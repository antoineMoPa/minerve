use crate::tools::{ExecuteCommandSettings, Tool};
use async_trait::async_trait;
use std::collections::HashMap;
use std::fs;
use std::fs::OpenOptions;
use std::io::Write;
use std::os::unix::fs::PermissionsExt; // Required for setting permissions on Unix platforms.
use std::path::Path;



pub struct SetUserCommand;

#[async_trait]
impl Tool for SetUserCommand {
    fn name(&self) -> &'static str {
        "set_user_command"
    }

    fn description(&self) -> &'static str {
        "Manage user script commands."
    }

    fn parameters(&self) -> HashMap<&'static str, &'static str> {
        let mut params = HashMap::new();
        params.insert("script_name", "string");
        params.insert("content", "string");
        params
    }

    async fn run(
        &self,
        args: HashMap<String, String>,
        _settings: ExecuteCommandSettings,
    ) -> String {
        let script_name = match args.get("script_name") {
            Some(name) => name,
            None => return String::from("[Error] Missing 'script_name' parameter."),
        };

        let content = match args.get("content") {
            Some(content) => content,
            None => return String::from("[Error] Missing 'content' parameter."),
        };

        let dir = Path::new(".minerve/commands");
        if !dir.exists() {
            if let Err(e) = fs::create_dir_all(dir) {
                return format!("[Error] Failed to create directory: {}", e);
            }
        }

        let file_path = dir.join(format!("{}.sh", script_name));
        match OpenOptions::new().write(true).create(true).open(&file_path) {
            Ok(mut file) => {
                if let Err(e) = file.write_all(content.as_bytes()) {
                    return format!("[Error] Failed to write content: {}", e);
                }
            }
            Err(e) => return format!("[Error] Failed to create script file: {}", e),
        }

        if let Err(e) = fs::set_permissions(&file_path, fs::Permissions::from_mode(0o755)) {
            return format!("[Error] Failed to set permissions: {}", e);
        }

        format!("✅ User command '{}' created successfully.", script_name)
    }
}
