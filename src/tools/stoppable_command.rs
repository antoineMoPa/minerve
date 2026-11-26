//! Commands a tool runs while a turn can be stopped.
//!
//! `std::process::Command::output` holds the turn's task until the command ends, so a stop had
//! to wait for it. These commands are awaited instead: when the turn is cancelled the tool's
//! future is dropped, and the command dies with everything it started.
//!
//! Every command also has a time limit, `DEFAULT_TIMEOUT` unless the tool call asks for more
//! with `timeout_seconds`: a command that hangs (a prompt, a server in the foreground) is
//! killed instead of holding the turn forever.

use std::ffi::OsStr;
use std::process::{Output, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt};

/// How long a command may run unless its tool call sets `timeout_seconds`.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(16);

/// The parameter a tool call sets to give its command more time than `DEFAULT_TIMEOUT`.
pub const TIMEOUT_PARAM: &str = "timeout_seconds";

/// The time limit a tool call asks for, `DEFAULT_TIMEOUT` when it sets none.
pub fn read_timeout_param(params: &super::ToolParams) -> Result<Duration, String> {
    Ok(params
        .get_usize_optional(TIMEOUT_PARAM)?
        .map(|seconds| Duration::from_secs(seconds as u64))
        .unwrap_or(DEFAULT_TIMEOUT))
}

/// A command in its own process group, killed as a group when its run is dropped or when it
/// runs past its time limit.
pub struct StoppableCommand {
    command: tokio::process::Command,
    timeout: Duration,
}

impl StoppableCommand {
    pub fn new(program: impl AsRef<OsStr>) -> Self {
        let mut command = tokio::process::Command::new(program);
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        // No prompt can wait for an answer nobody gives.
        command.env("GIT_TERMINAL_PROMPT", "0");
        // Its own session: its own group, so the children of a shell line can be killed with it,
        // and no controlling terminal. In a mere background group of minerve's terminal, a command
        // touching /dev/tty (a password prompt, `read < /dev/tty`) was stopped by SIGTTIN/SIGTTOU
        // and never ended; without a terminal, opening /dev/tty fails at once.
        #[cfg(unix)]
        // SAFETY: setsid is async-signal-safe and touches no memory of this process.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        Self {
            command,
            timeout: DEFAULT_TIMEOUT,
        }
    }

    /// Replaces `DEFAULT_TIMEOUT`.
    pub fn timeout(&mut self, timeout: Duration) -> &mut Self {
        self.timeout = timeout;
        self
    }

    /// `sh -c <line>`.
    pub fn shell(line: &str) -> Self {
        let mut command = Self::new("sh");
        command.arg("-c").arg(line);
        command
    }

    pub fn arg(&mut self, arg: impl AsRef<OsStr>) -> &mut Self {
        self.command.arg(arg);
        self
    }

    /// Runs the command to its end. Dropping this future kills the command's process group.
    /// Returns once the command exits, even if something it left in the background still holds
    /// its stdout or stderr open (`server &`): output written after `OUTPUT_GRACE` is not read.
    /// Fails with `ErrorKind::TimedOut`, naming the limit and holding the output so far, when
    /// the command runs past its time limit; it is then killed with its children.
    pub async fn output(&mut self) -> std::io::Result<Output> {
        let mut child = self.command.spawn()?;
        let mut group = ProcessGroupKilledOnDrop { id: child.id() };
        let stdout = spawn_pipe_reader(child.stdout.take());
        let stderr = spawn_pipe_reader(child.stderr.take());
        let status = match tokio::time::timeout(self.timeout, child.wait()).await {
            Ok(status) => status?,
            Err(_) => {
                drop(group);
                let _ = child.kill().await;
                let stdout = collect_pipe_output(stdout).await;
                let stderr = collect_pipe_output(stderr).await;
                return Err(describe_timeout(self.timeout, &stdout, &stderr));
            }
        };
        // Ended by itself: whatever it left in the background stays, as with a shell.
        group.id = None;
        Ok(Output {
            status,
            stdout: collect_pipe_output(stdout).await,
            stderr: collect_pipe_output(stderr).await,
        })
    }
}

/// What the model reads of a command killed at its time limit: what happened, how to give it
/// more time on purpose, and the output it wrote until then.
fn describe_timeout(timeout: Duration, stdout: &[u8], stderr: &[u8]) -> std::io::Error {
    let mut message = format!(
        "timed out after {}s and was killed. If it needs longer, run it again with {} set \
         (e.g. 120). Start a server in the background instead (`cmd > log 2>&1 &`).",
        timeout.as_secs(),
        TIMEOUT_PARAM
    );
    for (name, bytes) in [("stdout", stdout), ("stderr", stderr)] {
        if !bytes.is_empty() {
            message.push_str(&format!(
                "\n{} so far:\n{}",
                name,
                String::from_utf8_lossy(bytes)
            ));
        }
    }
    std::io::Error::new(std::io::ErrorKind::TimedOut, message)
}

/// How long the output of an exited command is still read, for pipes a background child keeps open.
const OUTPUT_GRACE: Duration = Duration::from_millis(300);

struct PipeReader {
    bytes: Arc<Mutex<Vec<u8>>>,
    task: tokio::task::JoinHandle<()>,
}

/// Reads a pipe to its end in the background, keeping what was read so far.
fn spawn_pipe_reader(pipe: Option<impl AsyncRead + Unpin + Send + 'static>) -> PipeReader {
    let bytes = Arc::new(Mutex::new(Vec::new()));
    let sink = bytes.clone();
    let task = tokio::spawn(async move {
        let Some(mut pipe) = pipe else { return };
        let mut chunk = [0u8; 8192];
        while let Ok(read) = pipe.read(&mut chunk).await {
            if read == 0 {
                break;
            }
            sink.lock().unwrap().extend_from_slice(&chunk[..read]);
        }
    });
    PipeReader { bytes, task }
}

/// What the pipe gave until it closed, or until `OUTPUT_GRACE` passed.
async fn collect_pipe_output(mut reader: PipeReader) -> Vec<u8> {
    if tokio::time::timeout(OUTPUT_GRACE, &mut reader.task)
        .await
        .is_err()
    {
        reader.task.abort();
    }
    let bytes = std::mem::take(&mut *reader.bytes.lock().unwrap());
    bytes
}

struct ProcessGroupKilledOnDrop {
    id: Option<u32>,
}

impl Drop for ProcessGroupKilledOnDrop {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Some(id) = self.id {
            // SAFETY: a signal to the group this command leads; no memory is involved.
            unsafe {
                libc::killpg(id as libc::pid_t, libc::SIGKILL);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[tokio::test]
    async fn returns_the_output_of_a_command_that_ends() {
        let output = StoppableCommand::shell("echo hi").output().await.unwrap();
        assert_eq!(String::from_utf8_lossy(&output.stdout), "hi\n");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn returns_when_the_shell_exits_even_if_a_background_child_holds_its_output() {
        let started = Instant::now();
        let output = StoppableCommand::shell("sleep 5 & echo hi")
            .output()
            .await
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&output.stdout), "hi\n");
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "{:?}",
            started.elapsed()
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_command_reading_the_terminal_ends_instead_of_being_stopped() {
        let started = Instant::now();
        let output = tokio::time::timeout(
            Duration::from_secs(3),
            StoppableCommand::shell("read x < /dev/tty && echo read").output(),
        )
        .await
        .expect("the command was stopped waiting for the terminal")
        .unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[tokio::test]
    async fn reads_output_larger_than_a_pipe_buffer() {
        let output = StoppableCommand::shell("head -c 200000 /dev/zero")
            .output()
            .await
            .unwrap();
        assert_eq!(output.stdout.len(), 200000);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_command_past_its_time_limit_is_killed_with_its_output_so_far() {
        let marker = std::env::temp_dir().join(format!("minerve-timeout-{}", std::process::id()));
        let _ = std::fs::remove_file(&marker);
        let line = format!(
            "echo started; (sleep 1; touch {}) & sleep 30",
            marker.display()
        );
        let started = Instant::now();
        let err = StoppableCommand::shell(&line)
            .timeout(Duration::from_millis(300))
            .output()
            .await
            .unwrap_err();
        assert!(started.elapsed() < Duration::from_secs(3));
        assert_eq!(err.kind(), std::io::ErrorKind::TimedOut);
        let message = err.to_string();
        assert!(message.contains("timeout_seconds"), "{message}");
        assert!(message.contains("stdout so far:\nstarted"), "{message}");
        tokio::time::sleep(Duration::from_millis(1500)).await;
        assert!(!marker.exists(), "the shell's child outlived the timeout");
    }

    #[test]
    fn the_time_limit_is_16_seconds_unless_the_call_sets_one() {
        let params = |pairs: &[(&str, &str)]| {
            super::super::ToolParams::new(
                pairs
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
            )
        };
        assert_eq!(
            read_timeout_param(&params(&[])).unwrap(),
            Duration::from_secs(16)
        );
        assert_eq!(
            read_timeout_param(&params(&[("timeout_seconds", "120")])).unwrap(),
            Duration::from_secs(120)
        );
        assert!(read_timeout_param(&params(&[("timeout_seconds", "soon")])).is_err());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_dropped_run_kills_the_command_and_its_children() {
        let marker = std::env::temp_dir().join(format!("minerve-stoppable-{}", std::process::id()));
        let _ = std::fs::remove_file(&marker);
        // The child of the shell would write the marker after the stop if it survived.
        let line = format!("(sleep 1; touch {}) & sleep 30", marker.display());
        let started = Instant::now();
        let stopped = tokio::time::timeout(
            Duration::from_millis(200),
            StoppableCommand::shell(&line).output(),
        )
        .await;
        assert!(
            stopped.is_err(),
            "the command was still running when dropped"
        );
        assert!(started.elapsed() < Duration::from_secs(5));
        tokio::time::sleep(Duration::from_millis(1500)).await;
        assert!(!marker.exists(), "the shell's child outlived the stop");
    }
}
