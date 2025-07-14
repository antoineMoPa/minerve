use async_trait::async_trait;
use std::collections::HashMap;
use std::fs;
use crate::tools::{ExecuteCommandSettings, Tool, ToolParams, ParamName};

pub struct ShowFileTool;

#[async_trait]
impl Tool for ShowFileTool {
    fn name(&self) -> &'static str {
        "show_file"
    }

    fn description(&self) -> &'static str {
        "Shows the content of a file. Prefer extract_structure when you just need an overview."
    }

    fn parameters(&self) -> HashMap<&'static str, &'static str> {
        let mut params = HashMap::new();
        params.insert(ParamName::FilePath.as_str(), "string");
        params
    }

    async fn run(
        &self,
        args: HashMap<String, String>,
        _settings: ExecuteCommandSettings,
    ) -> String {
        let params = ToolParams::new(args);
        let path = match params.get_string(ParamName::FilePath.as_str()) {
            Ok(s) => s,
            Err(e) => return e,
        };

        match fs::read_to_string(&path) {
            Ok(content) => content,
            Err(e) => {
                let error_message = e.to_string();
                if e.kind() == std::io::ErrorKind::NotFound
                    || error_message.contains("No such file or directory")
                {
                    "[file does not exist]".to_string()
                } else {
                    format!("[Error] Failed to read file: {}", e)
                }
            }
        }
    }
}
