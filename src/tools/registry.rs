use std::collections::HashMap;
use std::sync::Arc;
use crate::tools::Tool;
use async_trait::async_trait;
use std::fs;
use std::process::Command;

pub struct GetGeneralContext;

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
        let truncate = |s: String, limit: usize| {
            if s.len() > limit {
                format!("{}\n...[truncated]", &s[..limit])
            } else {
                s
            }
        };

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

        let truncate = |s: String, limit: usize| {
            if s.len() > limit {
                format!("{}\n...[truncated]", &s[..limit])
            } else {
                s
            }
        };

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

        let truncate = |s: String, limit: usize| {
            if s.len() > limit {
                format!("{}\n...[truncated]", &s[..limit])
            } else {
                s
            }
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

pub fn get_tool_registry() -> HashMap<&'static str, Arc<dyn Tool>> {
    let mut map: HashMap<&'static str, Arc<dyn Tool>> = HashMap::new();

    map.insert("get_general_context", Arc::new(GetGeneralContext));
    map.insert("search_for_string", Arc::new(SearchForStringTool));
    map.insert("search_for_path_pattern", Arc::new(SearchForPathPatternTool));
    map.insert("read_file", Arc::new(ReadFileTool));
    map.insert("list_files", Arc::new(ListFilesTool));
    map.insert("edit_file", Arc::new(EditFileTool));

    map
}
