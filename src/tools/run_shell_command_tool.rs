use async_trait::async_trait;
use std::collections::HashMap;

use super::stoppable_command::{read_timeout_param, StoppableCommand, TIMEOUT_PARAM};
use super::{ExecuteCommandSettings, Tool, ToolParams};

pub struct RunShellCommandTool;

#[async_trait]
impl Tool for RunShellCommandTool {
    fn name(&self) -> &'static str {
        "run_shell_command"
    }

    fn description(&self) -> &'static str {
        "Runs a shell command and returns its output. It is killed after 16 seconds unless \
         timeout_seconds asks for more: set it only for a command you expect to run long \
         (a build, a test suite)."
    }

    fn parameters(&self) -> HashMap<&'static str, &'static str> {
        let mut params = HashMap::new();
        params.insert("command", "string");
        params.insert("is_headless", "string"); // optional param
        params.insert(TIMEOUT_PARAM, "optional integer");
        params
    }

    async fn run(&self, args: HashMap<String, String>, settings: ExecuteCommandSettings) -> String {
        self.run_with_settings(args, settings).await
    }
}

impl RunShellCommandTool {
    pub async fn run_with_settings(
        &self,
        args: HashMap<String, String>,
        settings: ExecuteCommandSettings,
    ) -> String {
        let params = ToolParams::new(args);
        let command = match params.get_string("command") {
            Ok(cmd) => cmd,
            Err(e) => return e,
        };
        let _ = settings;
        let timeout = match read_timeout_param(&params) {
            Ok(timeout) => timeout,
            Err(e) => return e,
        };
        let output = StoppableCommand::shell(&command)
            .timeout(timeout)
            .output()
            .await;
        format!("$ {}\n{}", command, describe_shell_output(output))
    }

    /// Runs the command and waits for it: for `!command`, which no turn is running for. It
    /// blocks the terminal's thread, so it is killed at `DEFAULT_TIMEOUT` like a tool's command.
    /// Every shell command is trusted and runs without approval.
    pub fn execute_command(command: &str, _settings: Option<ExecuteCommandSettings>) -> String {
        describe_shell_output(
            crate::get_global_runtime().block_on(StoppableCommand::shell(command).output()),
        )
    }
}

/// What the model reads: stdout of a command that succeeded, stderr of one that failed.
fn describe_shell_output(output: std::io::Result<std::process::Output>) -> String {
    match output {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout).to_string(),
        Ok(out) => format!("[Error] {}", String::from_utf8_lossy(&out.stderr)),
        Err(e) => format!("[Error] {}", e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bang_command_that_hangs_is_killed_instead_of_freezing_the_terminal() {
        let started = std::time::Instant::now();
        let output = RunShellCommandTool::execute_command("echo before; sleep 60", None);
        assert!(started.elapsed() < std::time::Duration::from_secs(20));
        assert!(output.contains("timed out after 16s"), "{output}");
        assert!(output.contains("before"), "{output}");
    }
}
