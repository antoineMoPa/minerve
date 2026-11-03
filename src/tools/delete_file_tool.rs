use crate::tools::{ExecuteCommandSettings, ParamName, Tool, ToolParams};
use async_trait::async_trait;
use std::collections::HashMap;
use std::fs;
use std::io::ErrorKind;
use std::path::Path;

/// Deletes a single file. Never recursive: directories are refused, so a call can't wipe a tree.
pub struct DeleteFileTool;

#[async_trait]
impl Tool for DeleteFileTool {
    fn name(&self) -> &'static str {
        "delete_file"
    }

    fn description(&self) -> &'static str {
        "Delete a single file. Directories are refused: this never deletes recursively."
    }

    fn parameters(&self) -> HashMap<&'static str, &'static str> {
        let mut params = HashMap::new();
        params.insert(ParamName::Path.as_str(), "string");
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

        let metadata = match fs::symlink_metadata(Path::new(&path)) {
            Ok(m) => m,
            Err(e) if e.kind() == ErrorKind::NotFound => {
                return format!("[Error] No such file: {}", path);
            }
            Err(e) => return format!("[Error] Failed to inspect {}: {}", path, e),
        };

        if metadata.is_dir() {
            return format!(
                "[Error] {} is a directory; delete_file only removes single files.",
                path
            );
        }

        match fs::remove_file(&path) {
            Ok(_) => format!("Deleted {}", path),
            Err(e) => format!("[Error] Failed to delete {}: {}", path, e),
        }
    }
}
