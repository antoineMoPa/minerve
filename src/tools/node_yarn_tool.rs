use crate::tools::{Tool, ExecuteCommandSettings};
use std::collections::HashMap;
use std::process::Command;
use async_trait::async_trait;
use serde_json::Value;

pub struct NodeYarnTool;

#[async_trait]
impl Tool for NodeYarnTool {
    fn name(&self) -> &'static str {
        "NodeYarnTool"
    }

    fn description(&self) -> &'static str {
        "Runs `NODE_OPTIONS=\"--max-old-space-size=8192\" yarn run tsc --incremental`"
    }

    fn parameters(&self) -> HashMap<&'static str, &'static str> {
        HashMap::new()
    }

    async fn run(
        &self,
        _args: HashMap<String, String>,
        _settings: ExecuteCommandSettings,
    ) -> String {
        match Command::new("sh")
            .arg("-c")
            .arg("NODE_OPTIONS=\"--max-old-space-size=8192\" yarn run tsc --incremental")
            .output()
        {
            Ok(output) => String::from_utf8_lossy(&output.stdout).to_string(),
            Err(err) => err.to_string(),
        }
    }
}
