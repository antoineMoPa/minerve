use super::stoppable_command::{read_timeout_param, StoppableCommand, TIMEOUT_PARAM};
use crate::tools::{ExecuteCommandSettings, Tool, ToolParams};
use async_trait::async_trait;
use std::collections::HashMap;

pub struct CompileTypescriptProjectTool;

#[async_trait]
impl Tool for CompileTypescriptProjectTool {
    fn name(&self) -> &'static str {
        "compile_typescript_project"
    }

    fn description(&self) -> &'static str {
        "Runs `NODE_OPTIONS=\"--max-old-space-size=8192\" yarn run tsc --incremental`. Killed after 16 seconds unless timeout_seconds asks for more."
    }

    fn parameters(&self) -> HashMap<&'static str, &'static str> {
        HashMap::from([(TIMEOUT_PARAM, "optional integer")])
    }

    async fn run(
        &self,
        args: HashMap<String, String>,
        _settings: ExecuteCommandSettings,
    ) -> String {
        let timeout = match read_timeout_param(&ToolParams::new(args)) {
            Ok(timeout) => timeout,
            Err(e) => return e,
        };
        match StoppableCommand::shell(
            "NODE_OPTIONS=\"--max-old-space-size=8192\" yarn run tsc --incremental",
        )
        .timeout(timeout)
        .output()
        .await
        {
            Ok(output) => String::from_utf8_lossy(&output.stdout).to_string(),
            Err(err) => format!("[Error] {}", err),
        }
    }
}
