use crate::tools::{ParamName, Tool, ToolParams};
use async_trait::async_trait;
use chrono::Utc;
use std::collections::HashMap;
use std::fs;
use std::fs::OpenOptions;
use std::io::Write;
use std::process::Command;
use std::sync::Arc;

pub struct GetGeneralContext;

fn truncate(s: String, limit: usize) -> String {
    if s.len() > limit {
        format!("{}\n...[truncated]", &s[..limit])
    } else {
        s
    }
}

fn log_search_replace(filepath: &str, old_content: &str, new_content: &str, success: bool) {
    let timestamp = Utc::now().format("%Y-%m-%d %H:%M:%S UTC");
    let log_entry = format!(
        "\n=== SEARCH/REPLACE LOG ===\n\
        Timestamp: {}\n\
        File: {}\n\
        Success: {}\n\
        \n--- OLD CONTENT ---\n\
        {}\n\
        \n--- NEW CONTENT ---\n\
        {}\n\
        \n=== END LOG ENTRY ===\n\n",
        timestamp, filepath, success, old_content, new_content
    );

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open("search_replace.log")
        .unwrap_or_else(|e| panic!("Failed to open log file: {}", e));

    file.write_all(log_entry.as_bytes())
        .unwrap_or_else(|e| panic!("Failed to write to log file: {}", e));
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

    async fn run(&self, _args: HashMap<String, String>) -> String {
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

    async fn run(&self, args: HashMap<String, String>) -> String {
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

    async fn run(&self, args: HashMap<String, String>) -> String {
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

    async fn run(&self, args: HashMap<String, String>) -> String {
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

    async fn run(&self, _args: HashMap<String, String>) -> String {
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

    async fn run(&self, _args: HashMap<String, String>) -> String {
        let output = Command::new("git")
            .arg("diff")
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
        "Shows the content of a file without line numbers for easier content-based editing."
    }

    fn parameters(&self) -> HashMap<&'static str, &'static str> {
        let mut params = HashMap::new();
        params.insert(ParamName::FilePath.as_str(), "string");
        params
    }

    async fn run(&self, args: HashMap<String, String>) -> String {
        let params = ToolParams::new(args);
        let path = match params.get_string(ParamName::FilePath.as_str()) {
            Ok(s) => s,
            Err(e) => return e,
        };
        match fs::read_to_string(&path) {
            Ok(content) => content,
            Err(e) => format!("[Error] Failed to read file: {}", e),
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
        "Replaces existing content in a file with new content by searching for the old content. Use this for precise content-based editing."
    }

    fn parameters(&self) -> HashMap<&'static str, &'static str> {
        let mut params = HashMap::new();
        params.insert(ParamName::FilePath.as_str(), "string");
        params.insert("old_content", "string");
        params.insert("new_content", "string");
        params
    }

    async fn run(&self, args: HashMap<String, String>) -> String {
        let params = ToolParams::new(args);
        let filepath = match params.get_string(ParamName::FilePath.as_str()) {
            Ok(s) => s,
            Err(e) => return e,
        };
        let old_content = match params.get_string("old_content") {
            Ok(s) => s,
            Err(e) => return e,
        };
        let new_content = match params.get_string("new_content") {
            Ok(s) => s,
            Err(e) => return e,
        };

        // Helper function to normalize whitespace for comparison
        let normalize_whitespace =
            |s: &str| -> String { s.split_whitespace().collect::<Vec<_>>().join(" ") };

        match fs::read_to_string(&filepath) {
            Ok(content) => {
                // First try exact match
                if content.contains(&old_content) {
                    let updated_content = content.replace(&old_content, &new_content);
                    match fs::write(&filepath, updated_content) {
                        Ok(_) => {
                            log_search_replace(&filepath, &old_content, &new_content, true);
                            return format!("✅ Successfully replaced content in {}", filepath);
                        }
                        Err(e) => {
                            log_search_replace(&filepath, &old_content, &new_content, false);
                            return format!("[Error] Failed to write file: {}", e);
                        }
                    }
                }

                // If exact match fails, try whitespace-normalized matching
                let normalized_old = normalize_whitespace(&old_content);
                let normalized_content = normalize_whitespace(&content);

                if !normalized_content.contains(&normalized_old) {
                    return format!("[Error] Old content not found in file: {}", filepath);
                }

                // Find the matching substring in the original content using sliding window
                let old_words: Vec<&str> = old_content.split_whitespace().collect();
                let content_chars: Vec<char> = content.chars().collect();

                let mut start_idx = None;
                let mut end_idx = None;

                // Scan through the content to find matching word sequence
                let mut word_idx = 0;
                let mut char_idx = 0;
                let mut current_word = String::new();

                while char_idx < content_chars.len() {
                    let ch = content_chars[char_idx];

                    if ch.is_whitespace() {
                        if !current_word.is_empty() {
                            if word_idx < old_words.len() && current_word == old_words[word_idx] {
                                if word_idx == 0 {
                                    // Mark start of first word
                                    start_idx = Some(char_idx - current_word.len());
                                }
                                word_idx += 1;
                                if word_idx == old_words.len() {
                                    // Found complete match, mark end
                                    end_idx = Some(char_idx);
                                    break;
                                }
                            } else {
                                // Reset if word doesn't match
                                word_idx = 0;
                                if current_word == old_words[0] {
                                    start_idx = Some(char_idx - current_word.len());
                                    word_idx = 1;
                                }
                            }
                            current_word.clear();
                        }
                    } else {
                        current_word.push(ch);
                    }
                    char_idx += 1;
                }

                // Handle case where match ends at end of file
                if !current_word.is_empty()
                    && word_idx < old_words.len()
                    && current_word == old_words[word_idx]
                {
                    if word_idx == 0 {
                        start_idx = Some(char_idx - current_word.len());
                    }
                    word_idx += 1;
                    if word_idx == old_words.len() {
                        end_idx = Some(char_idx);
                    }
                }

                match (start_idx, end_idx) {
                    (Some(start), Some(end)) => {
                        let mut updated_content = String::new();
                        updated_content.push_str(&content[..start]);
                        updated_content.push_str(&new_content);
                        updated_content.push_str(&content[end..]);

                        match fs::write(&filepath, updated_content) {
                            Ok(_) => {
                                log_search_replace(&filepath, &old_content, &new_content, true);
                                format!("✅ Successfully replaced content in {}", filepath)
                            }
                            Err(e) => {
                                log_search_replace(&filepath, &old_content, &new_content, false);
                                format!("[Error] Failed to write file: {}", e)
                            }
                        }
                    }
                    _ => {
                        log_search_replace(&filepath, &old_content, &new_content, false);
                        format!(
                            "[Error] Could not locate exact position of old content in file: {}",
                            filepath
                        )
                    }
                }
            }
            Err(e) => format!("[Error] Failed to read file: {}", e),
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

    async fn run(&self, _args: HashMap<String, String>) -> String {
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
    map.insert("show_file", Arc::new(ShowFileTool));
    map.insert("replace_content", Arc::new(ReplaceContentTool));
    map.insert("run_cargo_check", Arc::new(RunCargoCheckTool));

    map
}
