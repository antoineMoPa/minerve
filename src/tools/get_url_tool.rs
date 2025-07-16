use async_trait::async_trait;
use reqwest;
use std::collections::HashMap;

use super::{Tool, ToolParams, ExecuteCommandSettings};

pub struct GetUrlTool;

#[async_trait]
impl Tool for GetUrlTool {
    fn name(&self) -> &'static str {
        "get_url"
    }

    fn description(&self) -> &'static str {
        "Fetches the content of a URL as plaintext."
    }

    fn parameters(&self) -> HashMap<&'static str, &'static str> {
        let mut params = HashMap::new();
        params.insert("url", "string");
        params
    }

    async fn run(&self, args: HashMap<String, String>, _settings: ExecuteCommandSettings) -> String {
        let url = match args.get("url") {
            Some(u) => u,
            None => return "[Error] URL parameter is missing.".to_string(),
        };

        match reqwest::get(url).await {
            Ok(response) => match response.text().await {
                Ok(text) => text,
                Err(_) => "[Error] Failed to convert response to text.".to_string(),
            },
            Err(_) => "[Error] Failed to fetch the URL.".to_string(),
        }
    }
}