use async_trait::async_trait;
use std::collections::HashMap;
use serde_json::Value;

pub mod registry;

// Parameter name constants to prevent typos
pub const PARAM_FILEPATH: &str = "filepath";
pub const PARAM_DIR: &str = "dir";
pub const PARAM_SEARCH_STRING: &str = "search_string";
pub const PARAM_PATH_PATTERN: &str = "path_pattern";
pub const PARAM_MODE: &str = "mode";
pub const PARAM_CONTENT: &str = "content";
pub const PARAM_START_LINE: &str = "start_line";
pub const PARAM_END_LINE: &str = "end_line";
pub const PARAM_NEW_CONTENT: &str = "new_content";

// Centralized parameter validation
pub struct ToolParams {
    args: HashMap<String, String>,
}

impl ToolParams {
    pub fn new(args: HashMap<String, String>) -> Self {
        Self { args }
    }

    pub fn get_string(&self, param: &str) -> Result<String, String> {
        self.args.get(param)
            .cloned()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| format!("[Error] Parameter '{}' is required and must be a non-empty string.", param))
    }

    pub fn get_string_optional(&self, param: &str, default: &str) -> String {
        self.args.get(param).cloned().unwrap_or_else(|| default.to_string())
    }

    pub fn get_integer(&self, param: &str) -> Result<usize, String> {
        self.args.get(param)
            .ok_or_else(|| format!("[Error] Parameter '{}' is required.", param))?
            .parse::<usize>()
            .map_err(|_| format!("[Error] Parameter '{}' must be a valid integer.", param))
    }

}

#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    fn parameters(&self) -> HashMap<&'static str, &'static str>;

    async fn run(&self, args: HashMap<String, String>) -> String;

    fn function_definition(&self) -> Value {
        let mut properties = serde_json::Map::new();
        let mut required = Vec::new();

        for (param_name, param_type) in self.parameters() {
            if param_type == "string" {
                properties.insert(param_name.to_string(), serde_json::json!({"type": "string"}));
            }
            else if param_type == "integer" {
                properties.insert(param_name.to_string(), serde_json::json!({"type": "integer"}));
            }
            required.push(param_name.to_string());
        }

        serde_json::json!({
            "type": "object",
            "properties": properties,
            "required": required
        })
    }
}
