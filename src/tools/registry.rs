use crate::tools::{ParamName, Tool, ToolParams};
use async_trait::async_trait;
use std::collections::HashMap;
use std::fs;
use std::io::{self, Write};
use std::process::Command;
use std::sync::Arc;

use super::get_general_context_tool::GetGeneralContext;
use super::replace_content_tool::ReplaceContentTool;
use super::search_for_path_pattern_tool::SearchForPathPatternTool;
use super::search_for_string_tool::SearchForStringTool;
use super::set_whole_file_contents_tool::SetWholeFileContentsTool;
use super::ExecuteCommandSettings;

pub struct ListFilesTool;

#[async_trait]
impl Tool for ListFilesTool {
    fn name(&self) -> &'static str {
        "list_files"
    }

    fn description(&self) -> &'static str {
        "Lists files in a directory"
    }

    fn parameters(&self) -> HashMap<&'static str, &'static str> {
        let mut params = HashMap::new();
        params.insert(ParamName::Dir.as_str(), "string");
        params
    }

    async fn run(
        &self,
        args: HashMap<String, String>,
        _settings: ExecuteCommandSettings,
    ) -> String {
        let params = ToolParams::new(args);
        let dir = params.get_string_optional(ParamName::Dir.as_str(), ".");
        match fs::read_dir(&dir) {
            Ok(entries) => entries
                .filter_map(|e| e.ok().map(|f| f.file_name().to_string_lossy().into_owned()))
                .collect::<Vec<_>>()
                .join("\n"),
            Err(e) => format!("[Error] Failed to list files: {}", e),
        }
    }
}

pub struct GitStatusTool;

#[async_trait]
impl Tool for GitStatusTool {
    fn name(&self) -> &'static str {
        "git_status"
    }

    fn description(&self) -> &'static str {
        "Gets the current git status of the repository."
    }

    fn parameters(&self) -> HashMap<&'static str, &'static str> {
        HashMap::new()
    }

    async fn run(
        &self,
        _args: HashMap<String, String>,
        _settings: ExecuteCommandSettings,
    ) -> String {
        let output = Command::new("git")
            .arg("status")
            .output()
            .map(|out| String::from_utf8_lossy(&out.stdout).to_string())
            .unwrap_or_else(|e| format!("[Error] {}", e));

        output
    }
}

pub struct GitDiffTool;

#[async_trait]
impl Tool for GitDiffTool {
    fn name(&self) -> &'static str {
        "git_diff"
    }

    fn description(&self) -> &'static str {
        "Gets the current git diff of the repository."
    }

    fn parameters(&self) -> HashMap<&'static str, &'static str> {
        HashMap::new()
    }

    async fn run(
        &self,
        _args: HashMap<String, String>,
        _settings: ExecuteCommandSettings,
    ) -> String {
        let output = Command::new("git")
            .arg("diff")
            .output()
            .map(|out| String::from_utf8_lossy(&out.stdout).to_string())
            .unwrap_or_else(|e| format!("[Error] {}", e));

        output
    }
}

pub struct GitDiffCachedTool;

#[async_trait]
impl Tool for GitDiffCachedTool {
    fn name(&self) -> &'static str {
        "git_diff_cached"
    }

    fn description(&self) -> &'static str {
        "Gets the current git diff of the repository."
    }

    fn parameters(&self) -> HashMap<&'static str, &'static str> {
        HashMap::new()
    }

    async fn run(
        &self,
        _args: HashMap<String, String>,
        _settings: ExecuteCommandSettings,
    ) -> String {
        let output = Command::new("git")
            .arg("diff")
            .arg("--cached")
            .output()
            .map(|out| String::from_utf8_lossy(&out.stdout).to_string())
            .unwrap_or_else(|e| format!("[Error] {}", e));

        output
    }
}

pub struct ShowFileTool;

#[async_trait]
impl Tool for ShowFileTool {
    fn name(&self) -> &'static str {
        "show_file"
    }

    fn description(&self) -> &'static str {
        "Shows the content of a file."
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

pub struct RunCargoCheckTool;

#[async_trait]
impl Tool for RunCargoCheckTool {
    fn name(&self) -> &'static str {
        "run_cargo_check"
    }

    fn description(&self) -> &'static str {
        "Runs `cargo check` in the current directory."
    }

    fn parameters(&self) -> HashMap<&'static str, &'static str> {
        HashMap::new()
    }

    async fn run(
        &self,
        _args: HashMap<String, String>,
        _settings: ExecuteCommandSettings,
    ) -> String {
        let output = Command::new("cargo")
            .arg("check")
            .output()
            .map(|out| {
                if out.status.success() {
                    let stdout = String::from_utf8_lossy(&out.stdout);
                    let stderr = String::from_utf8_lossy(&out.stderr);
                    let out = if !stderr.is_empty() {
                        format!("{}\n{}", stdout, stderr)
                    } else {
                        stdout.to_string()
                    };
                    out.to_string()
                } else {
                    format!("[Error] {}", String::from_utf8_lossy(&out.stderr))
                }
            })
            .unwrap_or_else(|e| format!("[Error] {}", e));

        output
    }
}

pub struct RunShellCommandTool;

impl Default for ExecuteCommandSettings {
    fn default() -> Self {
        Self { is_headless: false }
    }
}

impl RunShellCommandTool {
    pub fn execute_command(command: &str, settings: Option<ExecuteCommandSettings>) -> String {
        let settings = settings.unwrap_or_default();

        if settings.is_headless {
            // Prompt user for confirmation in headless mode
            print!("Do you want to run the command '{}'? (y/n): ", command);
            io::stdout().flush().unwrap();

            let mut input = String::new();
            if let Err(_) = io::stdin().read_line(&mut input) {
                return String::from("[Error] Failed to read user input");
            }

            let input = input.trim().to_lowercase();
            if input != "y" && input != "yes" {
                return String::from("Command execution cancelled by user.");
            }
        }

        let output = std::process::Command::new("sh")
            .arg("-c")
            .arg(command)
            .output()
            .map(|out| {
                if out.status.success() {
                    String::from_utf8_lossy(&out.stdout).to_string()
                } else {
                    format!("[Error] {}", String::from_utf8_lossy(&out.stderr))
                }
            })
            .unwrap_or_else(|e| format!("[Error] {}", e));
        output
    }
}

#[async_trait]
impl Tool for RunShellCommandTool {
    fn name(&self) -> &'static str {
        "run_shell_command"
    }

    fn description(&self) -> &'static str {
        "Runs a shell command. Use external UI for confirmation."
    }

    fn parameters(&self) -> HashMap<&'static str, &'static str> {
        let mut params = HashMap::new();
        params.insert("command", "string");
        params.insert("is_headless", "string"); // optional param
        params
    }

    async fn run(&self, args: HashMap<String, String>, settings: ExecuteCommandSettings) -> String {
        self.run_with_settings(args, settings).await
    }
}

impl RunShellCommandTool {
    pub async fn run_with_settings(
        &self,
        args: HashMap<String, String>,
        settings: ExecuteCommandSettings,
    ) -> String {
        let params = ToolParams::new(args);
        let command = match params.get_string("command") {
            Ok(cmd) => cmd,
            Err(e) => return e,
        };
        Self::execute_command(&command, Some(settings))
    }
}

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
            None => return String::from("[Error] Missing 'filepath' parameter."),
        };

        let content = match args.get("content") {
            Some(c) => c.clone(),
            None => String::new(),
        };

        match fs::write(filepath, content) {
            Ok(_) => format!("✅ Successfully created file {}", filepath),
            Err(e) => format!("[Error] Failed to create file {}: {}", filepath, e),
        }
    }
}

pub fn get_tool_registry() -> HashMap<&'static str, Arc<dyn Tool>> {
    let mut map: HashMap<&'static str, Arc<dyn Tool>> = HashMap::new();

    map.insert("get_general_context", Arc::new(GetGeneralContext));
    map.insert("search_for_string", Arc::new(SearchForStringTool));
    map.insert(
        "search_for_path_pattern",
        Arc::new(SearchForPathPatternTool),
    );
    map.insert("list_files", Arc::new(ListFilesTool));
    map.insert("git_status", Arc::new(GitStatusTool));
    map.insert("git_diff", Arc::new(GitDiffTool));
    map.insert("git_diff_cached", Arc::new(GitDiffCachedTool));
    map.insert("show_file", Arc::new(ShowFileTool));
    map.insert("replace_content", Arc::new(ReplaceContentTool));
    map.insert("run_cargo_check", Arc::new(RunCargoCheckTool));
    map.insert("run_shell_command", Arc::new(RunShellCommandTool));
    map.insert("set_whole_file_contents", Arc::new(SetWholeFileContentsTool));

    map
}
