use crate::tools::{ExecuteCommandSettings, ParamName, Tool, ToolParams};
use async_trait::async_trait;
use std::collections::HashMap;
use std::fs;

use crate::tools::utils::MAX_TOOL_OUTPUT_BYTES;

const MAX_LINES: usize = 2000;
// Leaves room for the continuation note under the cap every tool result goes through.
const MAX_BYTES: usize = MAX_TOOL_OUTPUT_BYTES - 200;

pub struct ReadTool;

fn read_line_window(
    content: &str,
    offset: Option<usize>,
    limit: Option<usize>,
) -> Result<String, String> {
    let lines: Vec<&str> = content.lines().collect();
    let start = offset.map(|o| o.saturating_sub(1)).unwrap_or(0);
    if start > 0 && start >= lines.len() {
        return Err(format!(
            "[Error] Offset {} is beyond end of file ({} lines total)",
            start + 1,
            lines.len()
        ));
    }

    let line_limit = limit.unwrap_or(MAX_LINES).min(MAX_LINES);
    let mut output = String::new();
    let mut end = start;
    for line in lines.iter().skip(start).take(line_limit) {
        if output.len() + line.len() + 1 > MAX_BYTES && end > start {
            break;
        }
        output.push_str(line);
        output.push('\n');
        end += 1;
    }

    if end < lines.len() {
        output.push_str(&format!(
            "\n[Showing lines {}-{} of {}. Use offset={} to continue.]",
            start + 1,
            end,
            lines.len(),
            end + 1
        ));
    }
    Ok(output)
}

#[async_trait]
impl Tool for ReadTool {
    fn name(&self) -> &'static str {
        "read"
    }

    fn description(&self) -> &'static str {
        "Read the contents of a file. Output is truncated to 2000 lines or about 10KB, whichever is hit first. Use offset/limit for large files; when you need the full file, continue with offset until complete. Prefer extract_structure when you just need an overview."
    }

    fn parameters(&self) -> HashMap<&'static str, &'static str> {
        let mut params = HashMap::new();
        params.insert(ParamName::Path.as_str(), "string");
        params.insert("offset", "optional integer");
        params.insert("limit", "optional integer");
        params
    }

    async fn run(
        &self,
        args: HashMap<String, String>,
        _settings: ExecuteCommandSettings,
    ) -> String {
        let params = ToolParams::new(args);
        let path = match params.get_string(ParamName::Path.as_str()) {
            Ok(s) => s,
            Err(e) => return e,
        };
        let offset = match params.get_usize_optional("offset") {
            Ok(n) => n,
            Err(e) => return e,
        };
        let limit = match params.get_usize_optional("limit") {
            Ok(n) => n,
            Err(e) => return e,
        };

        let content = match fs::read_to_string(&path) {
            Ok(content) => content,
            Err(e) => return format!("[Error] Failed to read {}: {}", path, e),
        };

        match read_line_window(&content, offset, limit) {
            Ok(output) => output,
            Err(e) => e,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_window_and_points_to_next_offset() {
        let content = "a\nb\nc\nd\n";
        assert_eq!(
            read_line_window(content, Some(2), Some(2)).unwrap(),
            "b\nc\n\n[Showing lines 2-3 of 4. Use offset=4 to continue.]"
        );
        assert_eq!(
            read_line_window(content, None, None).unwrap(),
            "a\nb\nc\nd\n"
        );
        assert!(read_line_window(content, Some(9), None).is_err());
    }
}
