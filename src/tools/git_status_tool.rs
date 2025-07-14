use async_trait::async_trait;
use std::collections::HashMap;
use std::process::Command;
use crate::tools::{ExecuteCommandSettings, Tool};

pub struct GitStatusTool;

#[async_trait]
impl Tool for GitStatusTool {
    fn name(&self) -> &'static str {
        "git_status"
    }

    fn description(&self) -> &'static str {
        "Gets the current git status of the repository."
    }

    fn parameters(&self) -> HashMap<&'static str, &'static str> {
        HashMap::new()
    }

    async fn run(
        &self,
        _args: HashMap<String, String>,
        _settings: ExecuteCommandSettings,
    ) -> String {
        let output = Command::new("git")
            .arg("status")
            .output()
            .map(|out| String::from_utf8_lossy(&out.stdout).to_string())
            .unwrap_or_else(|e| format!("[Error] {}", e));

        output
    }
}
