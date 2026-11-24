//! Commands a tool runs while a turn can be stopped.
//!
//! `std::process::Command::output` holds the turn's task until the command ends, so a stop had
//! to wait for it. These commands are awaited instead: when the turn is cancelled the tool's
//! future is dropped, and the command dies with everything it started.

use std::ffi::OsStr;
use std::process::{Output, Stdio};

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
        // Its own group, so the children of a shell line can be killed with it.
        #[cfg(unix)]
        command.process_group(0);
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
    pub async fn output(&mut self) -> std::io::Result<Output> {
        let child = self.command.spawn()?;
        let mut group = ProcessGroupKilledOnDrop { id: child.id() };
        let output = child.wait_with_output().await;
        // Ended by itself: whatever it left in the background stays, as with a shell.
        group.id = None;
        output
    }
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
