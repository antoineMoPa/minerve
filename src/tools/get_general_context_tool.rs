use crate::tools::{Tool};
use async_trait::async_trait;
use std::collections::HashMap;
use std::fs;
use std::process::Command;

use super::{utils::truncate, ExecuteCommandSettings};

pub struct GetGeneralContext;

#[async_trait]
impl Tool for GetGeneralContext {
    fn name(&self) -> &'static str {
        "get_general_context"
    }

    fn description(&self) -> &'static str {
        "Gets a snapshot of the current directory, git branch, staged files, and tree (excluding gitignored files)."
    }

    fn parameters(&self) -> HashMap<&'static str, &'static str> {
        HashMap::new()
    }

    async fn run(
        &self,
        _args: HashMap<String, String>,
        _settings: ExecuteCommandSettings,
    ) -> String {
        let exec = |cmd: &str| {
            Command::new("sh")
                .arg("-c")
                .arg(cmd)
                .output()
                .map(|out| String::from_utf8_lossy(&out.stdout).to_string())
                .unwrap_or_else(|e| format!("[Error] {}", e))
        };

        let dir = fs::read_dir(".")
            .map(|entries| {
                entries
                    .filter_map(|e| e.ok().map(|f| f.file_name().to_string_lossy().into_owned()))
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_else(|e| format!("[Error] Failed to list dir: {}", e));

        let result = serde_json::json!({
            "currentDirectory": truncate(dir, 2000),
            "gitBranch": truncate(exec("git rev-parse --abbrev-ref HEAD"), 200),
            "stagedFiles": truncate(exec("git diff --cached --name-only"), 1000),
            "workingTree": truncate(exec("git ls-files"), 2000)
        });

        serde_json::to_string_pretty(&result).unwrap()
    }
}
