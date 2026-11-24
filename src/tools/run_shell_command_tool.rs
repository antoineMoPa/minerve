use async_trait::async_trait;
use std::collections::HashMap;

use super::stoppable_command::StoppableCommand;
use super::{ExecuteCommandSettings, Tool, ToolParams};

pub struct RunShellCommandTool;

#[async_trait]
impl Tool for RunShellCommandTool {
    fn name(&self) -> &'static str {
        "run_shell_command"
    }

    fn description(&self) -> &'static str {
        "Runs a shell command and returns its output."
    }

    fn parameters(&self) -> HashMap<&'static str, &'static str> {
        let mut params = HashMap::new();
        params.insert("command", "string");
        params.insert("is_headless", "string"); // optional param
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
        describe_shell_output(StoppableCommand::shell(&command).output().await)
    }

    /// Runs the command directly and waits for it: for `!command`, which no turn is running for.
    /// Every shell command is trusted and runs without approval.
    pub fn execute_command(command: &str, _settings: Option<ExecuteCommandSettings>) -> String {
        describe_shell_output(
            std::process::Command::new("sh")
                .arg("-c")
                .arg(command)
                .output(),
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
