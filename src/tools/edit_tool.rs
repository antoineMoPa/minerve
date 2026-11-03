use crate::tools::{ExecuteCommandSettings, ParamName, Tool, ToolParams};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;
use std::fs;

pub struct EditTool;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TextReplacement {
    #[serde(rename = "oldText")]
    pub old_text: String,
    #[serde(rename = "newText")]
    pub new_text: String,
}

/// Every replacement is matched against the original content, must match exactly once
/// and must not overlap another replacement.
pub fn apply_replacements(content: &str, edits: &[TextReplacement]) -> Result<String, String> {
    if edits.is_empty() {
        return Err(String::from(
            "[Error] edits must contain at least one replacement.",
        ));
    }

    let mut ranges: Vec<(usize, usize, &str)> = Vec::new();
    for (i, edit) in edits.iter().enumerate() {
        if edit.old_text.is_empty() {
            return Err(format!("[Error] edits[{}].oldText is empty.", i));
        }
        let matches: Vec<usize> = content
            .match_indices(edit.old_text.as_str())
            .map(|(start, _)| start)
            .collect();
        match matches.len() {
            0 => {
                return Err(format!(
                    "[Error] edits[{}].oldText not found. It must match the file exactly, including whitespace. Read the file again.",
                    i
                ))
            }
            1 => ranges.push((matches[0], matches[0] + edit.old_text.len(), &edit.new_text)),
            count => {
                return Err(format!(
                    "[Error] edits[{}].oldText matches {} places. Add surrounding lines so it is unique.",
                    i, count
                ))
            }
        }
    }

    ranges.sort_by_key(|(start, _, _)| *start);
    for pair in ranges.windows(2) {
        if pair[0].1 > pair[1].0 {
            return Err(String::from(
                "[Error] Two edits overlap. Merge nearby changes into one edit.",
            ));
        }
    }

    let mut updated = String::with_capacity(content.len());
    let mut cursor = 0;
    for (start, end, new_text) in ranges {
        updated.push_str(&content[cursor..start]);
        updated.push_str(new_text);
        cursor = end;
    }
    updated.push_str(&content[cursor..]);
    Ok(updated)
}

#[async_trait]
impl Tool for EditTool {
    fn name(&self) -> &'static str {
        "edit"
    }

    fn description(&self) -> &'static str {
        "Edit a single file using exact text replacement. Every edits[].oldText must match a unique, non-overlapping region of the original file. If two changes affect the same block or nearby lines, merge them into one edit instead of emitting overlapping edits. Do not include large unchanged regions just to connect distant changes."
    }

    fn parameters(&self) -> HashMap<&'static str, &'static str> {
        let mut params = HashMap::new();
        params.insert(ParamName::Path.as_str(), "string");
        params.insert("edits", "array");
        params
    }

    fn function_definition(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file to edit (relative or absolute)"
                },
                "edits": {
                    "type": "array",
                    "description": "One or more targeted replacements. Each edit is matched against the original file, not incrementally. Do not include overlapping or nested edits.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "oldText": {
                                "type": "string",
                                "description": "Exact text for one targeted replacement. It must be unique in the original file."
                            },
                            "newText": {
                                "type": "string",
                                "description": "Replacement text for this targeted edit."
                            }
                        },
                        "required": ["oldText", "newText"]
                    }
                }
            },
            "required": ["path", "edits"]
        })
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
        // Non-string arguments arrive JSON-encoded, see handle_tool_call.
        let edits_json = match params.get_string("edits") {
            Ok(s) => s,
            Err(e) => return e,
        };
        let edits: Vec<TextReplacement> = match serde_json::from_str(&edits_json) {
            Ok(edits) => edits,
            Err(e) => {
                return format!(
                    "[Error] edits must be an array of {{oldText, newText}} objects: {}",
                    e
                )
            }
        };

        let content = match fs::read_to_string(&path) {
            Ok(content) => content,
            Err(e) => return format!("[Error] Failed to read {}: {}", path, e),
        };

        let updated = match apply_replacements(&content, &edits) {
            Ok(updated) => updated,
            Err(e) => return e,
        };

        match fs::write(&path, updated) {
            Ok(_) => format!("Successfully applied {} edit(s) to {}", edits.len(), path),
            Err(e) => format!("[Error] Failed to write {}: {}", path, e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn replacement(old_text: &str, new_text: &str) -> TextReplacement {
        TextReplacement {
            old_text: old_text.to_string(),
            new_text: new_text.to_string(),
        }
    }

    #[test]
    fn applies_several_edits_against_original() {
        let content = "let a = 1;\nlet b = 2;\nlet c = 3;\n";
        let updated = apply_replacements(
            content,
            &[
                replacement("let c = 3;", "let c = $HOME;"),
                replacement("let a = 1;", "let a = ${x};"),
            ],
        )
        .unwrap();
        assert_eq!(updated, "let a = ${x};\nlet b = 2;\nlet c = $HOME;\n");
    }

    #[test]
    fn rejects_ambiguous_missing_and_overlapping_edits() {
        let content = "foo foo bar";
        assert!(apply_replacements(content, &[replacement("foo", "x")]).is_err());
        assert!(apply_replacements(content, &[replacement("baz", "x")]).is_err());
        assert!(apply_replacements(
            content,
            &[replacement("foo bar", "x"), replacement("o bar", "y")]
        )
        .is_err());
        assert!(apply_replacements(content, &[]).is_err());
    }
}
