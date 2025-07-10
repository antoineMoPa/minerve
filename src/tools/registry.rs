use crate::tools::{ParamName, Tool, ToolParams};
use async_trait::async_trait;
use std::collections::HashMap;
use std::fs;
use std::io::{self, Write};
use std::process::Command;
use std::sync::Arc;

use super::ExecuteCommandSettings;

pub struct GetGeneralContext;

fn truncate(s: String, limit: usize) -> String {
    if s.len() > limit {
        format!("{}\n...[truncated]", &s[..limit])
    } else {
        s
    }
}

#[async_trait]
impl Tool for GetGeneralContext {
    fn name(&self) -> &'static str {
        "get_general_context"
    }

    fn description(&self) -> &'static str {
        "Gets a snapshot of the current directory, git branch, staged files, and tree (excluding gitignored files)."
    }

    fn parameters(&self) -> HashMap<&'static str, &'static str> {
        HashMap::new()
    }

    async fn run(
        &self,
        _args: HashMap<String, String>,
        _settings: ExecuteCommandSettings,
    ) -> String {
        let exec = |cmd: &str| {
            Command::new("sh")
                .arg("-c")
                .arg(cmd)
                .output()
                .map(|out| String::from_utf8_lossy(&out.stdout).to_string())
                .unwrap_or_else(|e| format!("[Error] {}", e))
        };

        let dir = fs::read_dir(".")
            .map(|entries| {
                entries
                    .filter_map(|e| e.ok().map(|f| f.file_name().to_string_lossy().into_owned()))
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_else(|e| format!("[Error] Failed to list dir: {}", e));

        let result = serde_json::json!({
            "currentDirectory": truncate(dir, 2000),
            "gitBranch": truncate(exec("git rev-parse --abbrev-ref HEAD"), 200),
            "stagedFiles": truncate(exec("git diff --cached --name-only"), 1000),
            "workingTree": truncate(exec("git ls-files"), 2000)
        });

        serde_json::to_string_pretty(&result).unwrap()
    }
}

pub struct SearchForStringTool;

#[async_trait]
impl Tool for SearchForStringTool {
    fn name(&self) -> &'static str {
        "search_for_string"
    }

    fn description(&self) -> &'static str {
        "Searches for a string in the current directory using ag or grep, excluding gitignored files."
    }

    fn parameters(&self) -> HashMap<&'static str, &'static str> {
        let mut params = HashMap::new();
        params.insert(ParamName::SearchString.as_str(), "string");
        params
    }

    async fn run(
        &self,
        args: HashMap<String, String>,
        _settings: ExecuteCommandSettings,
    ) -> String {
        let params = ToolParams::new(args);
        let search_string = match params.get_string(ParamName::SearchString.as_str()) {
            Ok(s) => s,
            Err(e) => return e,
        };

        let ag_check = Command::new("sh")
            .arg("-c")
            .arg("command -v ag")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);

        let command = if ag_check {
            format!(
                "ag --ignore .git --ignore node_modules \"{}\"",
                search_string
            )
        } else {
            format!(
                "grep -r --exclude-dir={{.git,node_modules}} \"{}\" .",
                search_string
            )
        };

        let output = Command::new("sh")
            .arg("-c")
            .arg(&command)
            .output()
            .map(|out| String::from_utf8_lossy(&out.stdout).to_string())
            .unwrap_or_else(|e| format!("[Error] {}", e));

        truncate(output, 2000)
    }
}

pub struct SearchForPathPatternTool;

#[async_trait]
impl Tool for SearchForPathPatternTool {
    fn name(&self) -> &'static str {
        "search_for_path_pattern"
    }

    fn description(&self) -> &'static str {
        "Searches for a path pattern in the current directory using ag or grep, excluding gitignored files."
    }

    fn parameters(&self) -> HashMap<&'static str, &'static str> {
        let mut params = HashMap::new();
        params.insert(ParamName::PathPattern.as_str(), "string");
        params
    }

    async fn run(
        &self,
        args: HashMap<String, String>,
        _settings: ExecuteCommandSettings,
    ) -> String {
        let params = ToolParams::new(args);
        let pattern = match params.get_string(ParamName::PathPattern.as_str()) {
            Ok(s) => s,
            Err(e) => return e,
        };

        let ag_check = Command::new("sh")
            .arg("-c")
            .arg("command -v ag")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);

        let command = if ag_check {
            format!("ag --ignore .git --ignore node_modules \"{}\"", pattern)
        } else {
            format!(
                "grep -r --exclude-dir={{.git,node_modules}} \"{}\" .",
                pattern
            )
        };

        let output = Command::new("sh")
            .arg("-c")
            .arg(&command)
            .output()
            .map(|out| String::from_utf8_lossy(&out.stdout).to_string())
            .unwrap_or_else(|e| format!("[Error] {}", e));

        truncate(output, 2000)
    }
}

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

pub struct ReplaceContentTool;

#[async_trait]
impl Tool for ReplaceContentTool {
    fn name(&self) -> &'static str {
        "replace_content"
    }

    fn description(&self) -> &'static str {
        "Replaces existing content in a file with new content by searching for the old content. Use this for precise content-based editing. Please replace entire functions or code blocks at onces to avoid silly mistakes with closing parenthesis & brackets."
    }

    fn parameters(&self) -> HashMap<&'static str, &'static str> {
        let mut params = HashMap::new();
        params.insert(ParamName::FilePath.as_str(), "string");
        params.insert("old_content", "string");
        params.insert("new_content", "optional string");
        params
    }

    async fn run(
        &self,
        args: HashMap<String, String>,
        _settings: ExecuteCommandSettings,
    ) -> String {
        let params = ToolParams::new(args);
        let filepath = match params.get_string(ParamName::FilePath.as_str()) {
            Ok(s) => s,
            Err(e) => return e,
        };
        let old_content = match params.get_string("old_content") {
            Ok(s) => s,
            Err(e) => return e,
        };
        let new_content = params.get_string_optional("new_content", "");

        match fs::read_to_string(&filepath) {
            Ok(content) => {
                // First try exact match
                if content.contains(&old_content) {
                    let updated_content = content.replace(&old_content, &new_content);
                    match fs::write(&filepath, updated_content) {
                        Ok(_) => {
                            return format!("✅ Successfully replaced content in {}", filepath);
                        }
                        Err(e) => {
                            return format!("[Error] Failed to write file: {}", e);
                        }
                    }
                } else {
                    return format!("[Error] Old content not found in file: {} - make sure it's an exact match including whitespace. Show file again to know what to replace.", filepath);
                }
            }
            Err(e) => {
                // If file does not exist and old_content is empty, create new file with new_content
                if e.kind() == std::io::ErrorKind::NotFound {
                    if old_content.is_empty() {
                        match fs::write(&filepath, &new_content) {
                            Ok(_) => {
                                return format!("✅ Successfully created new file {}", filepath);
                            }
                            Err(e) => {
                                return format!("[Error] Failed to create file: {}", e);
                            }
                        }
                    } else {
                        return format!("[Error] File not found: {}", filepath);
                    }
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
                    String::from_utf8_lossy(&out.stdout).to_string()
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

pub struct SubMinerveTool;

#[async_trait]
impl Tool for SubMinerveTool {
    fn name(&self) -> &'static str {
        "subminerve"
    }

    fn description(&self) -> &'static str {
        "Runs a new instance of the minerve chat in a subprocess. Useful for creating nested minerve sessions."
    }

    fn parameters(&self) -> HashMap<&'static str, &'static str> {
        let mut params = HashMap::new();
        params.insert("prompt", "string");
        params
    }

    async fn run(
        &self,
        args: HashMap<String, String>,
        _settings: ExecuteCommandSettings,
    ) -> String {
        let params = ToolParams::new(args);
        let prompt = params.get_string("prompt");
        let prompt = match prompt {
            Ok(p) if !p.is_empty() => p,
            _ => return String::from("[Error] Missing or empty 'prompt' parameter."),
        };

        // Call run_headless_with_capture directly instead of subprocess
        let output = crate::run_headless_with_capture(prompt.clone(), true).await;

        if output.is_empty() {
            format!(
                "✅ Subminerve session completed successfully with prompt: '{}'",
                prompt
            )
        } else {
            format!(
                "✅ Subminerve session completed successfully.\nOutput:\n{}",
                output
            )
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
    map.insert("subminerve", Arc::new(SubMinerveTool));

    map
}
