use std::collections::HashMap;
use std::sync::Arc;
use crate::tools::Tool;
use async_trait::async_trait;
use std::fs;
use std::process::Command;

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
                entries.filter_map(|e| e.ok().map(|f| f.file_name().to_string_lossy().into_owned())).collect::<Vec<_>>().join("\n")
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
        params.insert("searchString", "string");
        params
    }

    async fn run(&self, args: HashMap<String, String>) -> String {
        let search_string = args.get("searchString").cloned().unwrap_or_default();
        if search_string.is_empty() {
            return "[Error] searchString parameter is required.".into();
        }

        let ag_check = Command::new("sh")
            .arg("-c")
            .arg("command -v ag")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);

        let command = if ag_check {
            format!("ag --ignore .git --ignore node_modules \"{}\"", search_string)
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
        params.insert("pathPattern", "string");
        params
    }

    async fn run(&self, args: HashMap<String, String>) -> String {
        let pattern = args.get("pathPattern").cloned().unwrap_or_default();
        if pattern.is_empty() {
            return "[Error] pathPattern parameter is required.".into();
        }

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

pub struct ReadFileTool;

#[async_trait]
impl Tool for ReadFileTool {
    fn name(&self) -> &'static str {
        "read_file"
    }

    fn description(&self) -> &'static str {
        "Reads the content of a file"
    }

    fn parameters(&self) -> HashMap<&'static str, &'static str> {
        let mut params = HashMap::new();
        params.insert("path", "string");
        params
    }

    async fn run(&self, args: HashMap<String, String>) -> String {
        let path = args.get("path").cloned().unwrap_or_default();
        match fs::read_to_string(&path) {
            Ok(content) => content,
            Err(e) => format!("[Error] Failed to read file: {}", e),
        }
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
        params.insert("dir", "string");
        params
    }

    async fn run(&self, args: HashMap<String, String>) -> String {
        let dir = args.get("dir").cloned().unwrap_or_else(|| ".".into());
        match fs::read_dir(&dir) {
            Ok(entries) => entries
                .filter_map(|e| e.ok().map(|f| f.file_name().to_string_lossy().into_owned()))
                .collect::<Vec<_>>()
                .join("\n"),
            Err(e) => format!("[Error] Failed to list files: {}", e),
        }
    }
}

pub struct EditFileTool;

#[async_trait]
impl Tool for EditFileTool {
    fn name(&self) -> &'static str {
        "edit_file"
    }

    fn description(&self) -> &'static str {
        "Edits a file by appending or prepending content"
    }

    fn parameters(&self) -> HashMap<&'static str, &'static str> {
        let mut params = HashMap::new();
        params.insert("filepath", "string");
        params.insert("mode", "string (append|prepend)");
        params.insert("content", "string");
        params
    }

    async fn run(&self, args: HashMap<String, String>) -> String {
        let path = args.get("filepath").cloned().unwrap_or_default();
        let mode = args.get("mode").cloned().unwrap_or_else(|| "append".into());
        let new_content = args.get("content").cloned().unwrap_or_default();

        if path.is_empty() || new_content.is_empty() {
            return "[Error] 'filepath' and 'content' must be provided.".into();
        }

        let existing = fs::read_to_string(&path).unwrap_or_default();

        let final_content = match mode.as_str() {
            "prepend" => format!("{}\n{}", new_content, existing),
            _ => format!("{}\n{}", existing, new_content),
        };

        match fs::write(&path, final_content) {
            Ok(_) => format!("✅ File edited successfully: {}", path),
            Err(e) => format!("[Error] Failed to edit file: {}", e),
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

pub struct ShowFileWithLineNumbers;

#[async_trait]
impl Tool for ShowFileWithLineNumbers {
    fn name(&self) -> &'static str {
        "show_file_with_line_numbers"
    }

    fn description(&self) -> &'static str {
        "Shows the content of a file with line numbers."
    }

    fn parameters(&self) -> HashMap<&'static str, &'static str> {
        let mut params = HashMap::new();
        params.insert("path", "string");
        params
    }

    async fn run(&self, args: HashMap<String, String>) -> String {
        let path = args.get("path").cloned().unwrap_or_default();
        match fs::read_to_string(&path) {
            Ok(content) => content.lines()
                .enumerate()
                .map(|(i, line)| format!("{:>4}: {}", i + 1, line))
                .collect::<Vec<_>>()
                .join("\n"),
            Err(e) => format!("[Error] Failed to read file: {}", e),
        }
    }
}

pub struct ShowFileRangeTool;

#[async_trait]
impl Tool for ShowFileRangeTool {
    fn name(&self) -> &'static str {
        "show_file_range"
    }

    fn description(&self) -> &'static str {
        "Shows a specific range of lines from a file with line numbers."
    }

    fn parameters(&self) -> HashMap<&'static str, &'static str> {
        let mut params = HashMap::new();
        params.insert("filepath", "string");
        params.insert("start_line", "integer");
        params.insert("end_line", "integer");
        params
    }

    async fn run(&self, args: HashMap<String, String>) -> String {
        let filepath = args.get("filepath").cloned().unwrap_or_default();
        let start_line: usize = args.get("start_line").and_then(|s| s.parse().ok()).unwrap_or(1);
        let end_line: usize = args.get("end_line").and_then(|s| s.parse().ok()).unwrap_or(10);

        if filepath.is_empty() {
            return "[Error] 'filepath' must be provided.".into();
        }

        match fs::read_to_string(&filepath) {
            Ok(content) => {
                let lines: Vec<&str> = content.lines().collect();
                let start_idx = start_line.saturating_sub(1);
                let end_idx = end_line.min(lines.len());

                if start_idx >= lines.len() {
                    return "[Error] Start line is beyond file length.".into();
                }

                lines[start_idx..end_idx]
                    .iter()
                    .enumerate()
                    .map(|(i, line)| format!("{:>4}: {}", start_idx + i + 1, line))
                    .collect::<Vec<_>>()
                    .join("\n")
            }
            Err(e) => format!("[Error] Failed to read file: {}", e),
        }
    }
}

pub struct ReplaceLineRangeTool;

#[async_trait]
impl Tool for ReplaceLineRangeTool {
    fn name(&self) -> &'static str {
        "replace_line_range"
    }

    fn description(&self) -> &'static str {
        "Replaces a range of lines in a file with new content. Use this for iterative editing by first viewing lines with show_file_range, then replacing specific ranges."
    }

    fn parameters(&self) -> HashMap<&'static str, &'static str> {
        let mut params = HashMap::new();
        params.insert("filepath", "string");
        params.insert("start_line", "integer");
        params.insert("end_line", "integer");
        params.insert("new_content", "string");
        params
    }

    async fn run(&self, args: HashMap<String, String>) -> String {
        let filepath = args.get("filepath").cloned().unwrap_or_default();
        let start_line: usize = args.get("start_line").and_then(|s| s.parse().ok()).unwrap_or(1);
        let end_line: usize = args.get("end_line").and_then(|s| s.parse().ok()).unwrap_or(1);
        let new_content = args.get("new_content").cloned().unwrap_or_default();

        if filepath.is_empty() {
            return "[Error] 'filepath' must be provided.".into();
        }

        if start_line == 0 || end_line == 0 || start_line > end_line {
            return "[Error] Invalid line range. Lines are 1-indexed and start_line must be <= end_line.".into();
        }

        match fs::read_to_string(&filepath) {
            Ok(content) => {
                let mut lines: Vec<String> = content.lines().map(|s| s.to_string()).collect();
                let start_idx = start_line - 1;
                let end_idx = end_line - 1;

                if start_idx >= lines.len() {
                    return "[Error] Start line is beyond file length.".into();
                }

                let end_idx = end_idx.min(lines.len() - 1);

                // Replace the range with new content lines
                let new_lines: Vec<String> = new_content.lines().map(|s| s.to_string()).collect();

                // Remove the old range
                lines.drain(start_idx..=end_idx);

                // Insert new content at the start position
                for (i, new_line) in new_lines.into_iter().enumerate() {
                    lines.insert(start_idx + i, new_line);
                }

                let final_content = lines.join("\n");

                match fs::write(&filepath, final_content) {
                    Ok(_) => format!("✅ Successfully replaced lines {}-{} in {}", start_line, end_line, filepath),
                    Err(e) => format!("[Error] Failed to write file: {}", e),
                }
            }
            Err(e) => format!("[Error] Failed to read file: {}", e),
        }
    }
}


pub fn get_tool_registry() -> HashMap<&'static str, Arc<dyn Tool>> {
    let mut map: HashMap<&'static str, Arc<dyn Tool>> = HashMap::new();

    map.insert("get_general_context", Arc::new(GetGeneralContext));
    map.insert("search_for_string", Arc::new(SearchForStringTool));
    map.insert("search_for_path_pattern", Arc::new(SearchForPathPatternTool));
    map.insert("read_file", Arc::new(ReadFileTool));
    map.insert("list_files", Arc::new(ListFilesTool));
    map.insert("git_status", Arc::new(GitStatusTool));
    map.insert("git_diff", Arc::new(GitDiffTool));
    map.insert("show_file_with_line_numbers", Arc::new(ShowFileWithLineNumbers));
    map.insert("show_file_range", Arc::new(ShowFileRangeTool));
    map.insert("replace_line_range", Arc::new(ReplaceLineRangeTool));

    map
}
