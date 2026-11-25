//! Commands a tool runs while a turn can be stopped.
//!
//! `std::process::Command::output` holds the turn's task until the command ends, so a stop had
//! to wait for it. These commands are awaited instead: when the turn is cancelled the tool's
//! future is dropped, and the command dies with everything it started.

use std::ffi::OsStr;
use std::process::{Output, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt};

/// A command in its own process group, killed as a group when its run is dropped.
pub struct StoppableCommand {
    command: tokio::process::Command,
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
        Self { command }
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
    pub async fn output(&mut self) -> std::io::Result<Output> {
        let mut child = self.command.spawn()?;
        let mut group = ProcessGroupKilledOnDrop { id: child.id() };
        let stdout = spawn_pipe_reader(child.stdout.take());
        let stderr = spawn_pipe_reader(child.stderr.take());
        let status = child.wait().await?;
        // Ended by itself: whatever it left in the background stays, as with a shell.
        group.id = None;
        Ok(Output {
            status,
            stdout: collect_pipe_output(stdout).await,
            stderr: collect_pipe_output(stderr).await,
        })
    }
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
