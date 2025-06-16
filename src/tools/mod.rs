use async_trait::async_trait;
use std::collections::HashMap;

pub mod registry;
mod get_general_context;

#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    fn parameters(&self) -> HashMap<&'static str, &'static str>;

    async fn run(&self, args: HashMap<String, String>) -> String;
}
