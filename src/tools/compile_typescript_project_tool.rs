use super::stoppable_command::StoppableCommand;
use crate::tools::{ExecuteCommandSettings, Tool};
use async_trait::async_trait;
use std::collections::HashMap;

pub struct CompileTypescriptProjectTool;

#[async_trait]
impl Tool for CompileTypescriptProjectTool {
    fn name(&self) -> &'static str {
        "compile_typescript_project"
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
        match StoppableCommand::shell(
            "NODE_OPTIONS=\"--max-old-space-size=8192\" yarn run tsc --incremental",
        )
        .output()
        .await
        {
            Ok(output) => String::from_utf8_lossy(&output.stdout).to_string(),
            Err(err) => err.to_string(),
        }
    }
}
