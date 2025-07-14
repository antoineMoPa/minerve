use crate::tools::{ParamName, Tool, ToolParams};
use async_trait::async_trait;
use regex::Regex;
use std::collections::HashMap;
use std::fs;

use super::ExecuteCommandSettings;

pub struct ReplaceContentTool;

fn check_string_balance(content: &str, open: char, close: char) -> Result<(), String> {
    let mut balance = 0;
    for c in content.chars() {
        if c == open {
            balance += 1;
        } else if c == close {
            balance -= 1;
        }
        if balance < 0 {
            return Err(format!(
                "[Error] Unbalanced {} in content",
                if open == '(' {
                    "parentheses"
                } else {
                    "brackets"
                }
            ));
        }
    }
    if balance != 0 {
        Err(format!(
            "[Error] Unbalanced {} in content",
            if open == '(' {
                "parentheses"
            } else {
                "brackets"
            }
        ))
    } else {
        Ok(())
    }
}

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

        let check_old = check_string_balance(&old_content, '(', ')')
            .and(check_string_balance(&old_content, '[', ']'))
            .and(check_string_balance(&old_content, '{', '}'));
        if let Err(e) = check_old {
            return format!("{} {} [in old content]- Please make sure to replace entire logical blocks of code.", e, old_content);
        }

        let check_new = check_string_balance(&new_content, '(', ')')
            .and(check_string_balance(&new_content, '[', ']'))
            .and(check_string_balance(&new_content, '{', '}'));
        if let Err(e) = check_new {
            return format!("{} {} [in new content] - Please make sure to replace entire logical blocks of code.", e, new_content);
        }

        match fs::read_to_string(&filepath) {
            Ok(content) => {
                // Try regex replacement with multi-line support
                match Regex::new(&format!("(?s){}", regex::escape(&old_content))) {
                    Ok(re) => {
                        if re.is_match(&content) {
                            let updated_content = re.replace_all(&content, &new_content);
                            match fs::write(&filepath, updated_content.as_ref()) {
                                Ok(_) => {
                                    return format!(
                                        "✅ Successfully replaced content in {}",
                                        filepath
                                    );
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
                        return format!("[Error] Failed to create regex pattern: {}", e);
                    }
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
