use async_trait::async_trait;
use std::collections::HashMap;
use std::fs;
use crate::tools::{ExecuteCommandSettings, Tool};

pub struct CreateFileTool;

#[async_trait]
impl Tool for CreateFileTool {
    fn name(&self) -> &'static str {
        "create_file"
    }

    fn description(&self) -> &'static str {
        "Creates a new file with specified content."
    }

    fn parameters(&self) -> HashMap<&'static str, &'static str> {
        let mut params = HashMap::new();
        params.insert("filepath", "string");
        params.insert("content", "optional string");
        params
    }

    async fn run(
        &self,
        args: HashMap<String, String>,
        _settings: ExecuteCommandSettings,
    ) -> String {
        let filepath = match args.get("filepath") {
            Some(f) => f,
            None => return String::from(["Error"] Missing 'filepath' parameter.),
        };

        let content = match args.get("content") {
            Some(c) => c.clone(),
            None => String::new(),
        };

        match fs::write(filepath, content) {
            Ok(_) => format!(✅ Successfully created file {}, filepath),
            Err(e) => format!(["Error"] Failed to create file {}: {}, filepath, e),
        }
    }
}