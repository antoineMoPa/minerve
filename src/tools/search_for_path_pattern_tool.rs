use super::utils::truncate;
use super::ExecuteCommandSettings;
use crate::tools::{ParamName, Tool, ToolParams};
use async_trait::async_trait;
use std::collections::HashMap;
use std::process::Command;

pub struct SearchForPathPatternTool;

#[async_trait]
impl Tool for SearchForPathPatternTool {
    fn name(&self) -> &'static str {
        "search_for_path_pattern"
    }

    fn description(&self) -> &'static str {
        "Searches for a path pattern in the current directory using ag or grep, excluding gitignored files."
    }

    fn parameters(&self) -> HashMap<&'static str, &'static str> {
        let mut params = HashMap::new();
        params.insert(ParamName::PathPattern.as_str(), "string");
        params
    }

    async fn run(
        &self,
        args: HashMap<String, String>,
        _settings: ExecuteCommandSettings,
    ) -> String {
        let params = ToolParams::new(args);
        let pattern = match params.get_string(ParamName::PathPattern.as_str()) {
            Ok(s) => s,
            Err(e) => return e,
        };

        let ag_check = Command::new("sh")
            .arg("-c")
            .arg("command -v ag")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);

        let command = if ag_check {
            format!("ag --ignore .git --ignore node_modules \"{}\"", pattern)
        } else {
            format!(
                "grep -r --exclude-dir={{.git,node_modules}} \"{}\" .",
                pattern
            )
        };

        let output = Command::new("sh")
            .arg("-c")
            .arg(&command)
            .output()
            .map(|out| String::from_utf8_lossy(&out.stdout).to_string())
            .unwrap_or_else(|e| format!("[Error] {}", e));

        truncate(output, 2000)
    }
}
