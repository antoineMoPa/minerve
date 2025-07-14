use async_trait::async_trait;
use std::collections::HashMap;
use std::io::{self, Write};

use super::{ExecuteCommandSettings, Tool, ToolParams};

pub struct RunShellCommandTool;

#[async_trait]
impl Tool for RunShellCommandTool {
    fn name(&self) -> &'static str {
        "run_shell_command"
    }

    fn description(&self) -> &'static str {
        "Runs a shell command. Use external UI for confirmation."
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
        Self::execute_command(&command, Some(settings))
    }

    pub fn execute_command(command: &str, settings: Option<ExecuteCommandSettings>) -> String {
        let settings = settings.unwrap_or_default();

        if settings.is_headless {
            // Prompt user for confirmation in headless mode
            print!("Do you want to run the command '{}'? (y/n): ", command);
            io::stdout().flush().unwrap();

            let mut input = String::new();
            if let Err(_) = io::stdin().read_line(&mut input) {
                return String::from("[Error] Failed to read user input");
            }

            let input = input.trim().to_lowercase();
            if input != "y" && input != "yes" {
                return String::from("Command execution cancelled by user.");
            }
        }

        let output = std::process::Command::new("sh")
            .arg("-c")
            .arg(command)
            .output()
            .map(|out| {
                if out.status.success() {
                    String::from_utf8_lossy(&out.stdout).to_string()
                } else {
                    format!("[Error] {}", String::from_utf8_lossy(&out.stderr))
                }
            })
            .unwrap_or_else(|e| format!("[Error] {}", e));
        output
    }
}
