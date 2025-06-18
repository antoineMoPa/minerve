use async_trait::async_trait;
use std::collections::HashMap;
use serde_json::Value;

pub mod registry;

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
