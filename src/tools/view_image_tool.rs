use crate::images::image_to_send;
use crate::tools::{ExecuteCommandSettings, ParamName, Tool, ToolParams};
use async_trait::async_trait;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Shows an image file to the model. The image is sent after the tool results of the call
/// (see `chat::with_tool_images_in_user_messages`).
pub struct ViewImageTool;

fn open_image(args: HashMap<String, String>) -> Result<PathBuf, String> {
    let path = ToolParams::new(args).get_string(ParamName::Path.as_str())?;
    image_to_send(Path::new(&path)).map_err(|err| format!("[Error] {}", err))
}

fn attached_text(path: &Path) -> String {
    format!("Image {} is attached after the tool results.", path.display())
}

#[async_trait]
impl Tool for ViewImageTool {
    fn name(&self) -> &'static str {
        "view_image"
    }

    fn description(&self) -> &'static str {
        "Look at an image file (png, jpg, gif or webp), such as a screenshot. Large images are scaled down. Images pasted in earlier prompts are no longer attached; open them again with this tool when needed."
    }

    fn parameters(&self) -> HashMap<&'static str, &'static str> {
        let mut params = HashMap::new();
        params.insert(ParamName::Path.as_str(), "string");
        params
    }

    async fn run(&self, args: HashMap<String, String>, _settings: ExecuteCommandSettings) -> String {
        match open_image(args) {
            Ok(path) => attached_text(&path),
            Err(err) => err,
        }
    }

    async fn run_with_images(
        &self,
        args: HashMap<String, String>,
        _settings: ExecuteCommandSettings,
    ) -> (String, Vec<PathBuf>) {
        match open_image(args) {
            Ok(path) => (attached_text(&path), vec![path]),
            Err(err) => (err, Vec::new()),
        }
    }
}
