use crate::tools::{ExecuteCommandSettings, ParamName, Tool, ToolParams};
use async_trait::async_trait;
use std::collections::HashMap;
use std::fs;
use std::path::Path;

pub struct WriteTool;

#[async_trait]
impl Tool for WriteTool {
    fn name(&self) -> &'static str {
        "write"
    }

    fn description(&self) -> &'static str {
        "Write content to a file, creating it or overwriting it if it exists. Missing parent directories are created. For partial changes, use the edit tool instead."
    }

    fn parameters(&self) -> HashMap<&'static str, &'static str> {
        let mut params = HashMap::new();
        params.insert(ParamName::Path.as_str(), "string");
        params.insert(ParamName::Content.as_str(), "string");
        params
    }

    async fn run(
        &self,
        args: HashMap<String, String>,
        _settings: ExecuteCommandSettings,
    ) -> String {
        let params = ToolParams::new(args);
        let path = match params.get_string(ParamName::Path.as_str()) {
            Ok(s) => s,
            Err(e) => return e,
        };
        // An empty file is a valid write, so content may be an empty string.
        let content = match params.get_present_string(ParamName::Content.as_str()) {
            Ok(s) => s,
            Err(e) => return e,
        };

        if let Some(parent) = Path::new(&path).parent() {
            if !parent.as_os_str().is_empty() {
                if let Err(e) = fs::create_dir_all(parent) {
                    return format!("[Error] Failed to create {}: {}", parent.display(), e);
                }
            }
        }

        match fs::write(&path, &content) {
            Ok(_) => format!("Successfully wrote {} bytes to {}", content.len(), path),
            Err(e) => format!("[Error] Failed to write {}: {}", path, e),
        }
    }
}
