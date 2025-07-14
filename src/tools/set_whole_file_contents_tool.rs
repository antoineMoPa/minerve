use super::ExecuteCommandSettings;
use crate::tools::{ParamName, Tool, ToolParams};
use async_trait::async_trait;
use std::collections::HashMap;

pub struct SetWholeFileContentsTool;

#[async_trait]
impl Tool for SetWholeFileContentsTool {
    fn name(&self) -> &'static str {
        "set_whole_file_contents"
    }

    fn description(&self) -> &'static str {
        "Sets the entire contents of a file to the provided content. Only use this for files you fully understand."
    }

    fn parameters(&self) -> HashMap<&'static str, &'static str> {
        let mut params = HashMap::new();
        params.insert(ParamName::FilePath.as_str(), "string");
        params.insert(ParamName::Content.as_str(), "string");
        params
    }

    async fn run(
        &self,
        args: HashMap<String, String>,
        _settings: ExecuteCommandSettings,
    ) -> String {
        let params = ToolParams::new(args);
        let file_path = match params.get_string(ParamName::FilePath.as_str()) {
            Ok(s) => s,
            Err(e) => return e,
        };
        let content = match params.get_string(ParamName::Content.as_str()) {
            Ok(s) => s,
            Err(e) => return e,
        };

        match std::fs::write(&file_path, content) {
            Ok(_) => format!("Successfully set contents of file: {}", file_path),
            Err(e) => format!("[Error] Failed to write to file {}: {}", file_path, e),
        }
    }
}
