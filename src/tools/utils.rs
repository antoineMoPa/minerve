/// Every tool result sent back to the model is capped at this size, see cap_tool_output.
pub const MAX_TOOL_OUTPUT_BYTES: usize = 10 * 1024;

/// Largest index <= limit that does not split a UTF-8 character.
fn char_boundary_at_or_before(s: &str, limit: usize) -> usize {
    let mut end = limit.min(s.len());
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    end
}

pub fn truncate(s: String, limit: usize) -> String {
    if s.len() > limit {
        format!(
            "{}\n...[truncated]",
            &s[..char_boundary_at_or_before(&s, limit)]
        )
    } else {
        s
    }
}

/// Keeps the first MAX_TOOL_OUTPUT_BYTES of a tool result, cut after a whole line when possible,
/// and tells the model how much was left out.
pub fn cap_tool_output(output: String) -> String {
    if output.len() <= MAX_TOOL_OUTPUT_BYTES {
        return output;
    }
    let boundary = char_boundary_at_or_before(&output, MAX_TOOL_OUTPUT_BYTES);
    let end = match output[..boundary].rfind('\n') {
        Some(newline) if newline > 0 => newline + 1,
        _ => boundary,
    };
    format!(
        "{}\n[Output truncated: showing the first {} of {} bytes. Narrow the request (a path, a pattern, offset/limit) to see the rest.]",
        &output[..end],
        end,
        output.len()
    )
}

/// A shell command shown in the status line is cut at this many characters.
const MAX_STATUS_COMMAND_CHARS: usize = 120;

/// The status line while a tool call runs. A shell command is shown itself (its first line, cut
/// when long), so the user sees what is running; other tools are named.
pub fn describe_running_tool_call(tool_name: &str, arguments_json: &str) -> String {
    let command = (tool_name == "run_shell_command")
        .then(|| serde_json::from_str::<serde_json::Value>(arguments_json).ok())
        .flatten()
        .and_then(|arguments| Some(arguments.get("command")?.as_str()?.trim().to_string()))
        .filter(|command| !command.is_empty());
    let Some(command) = command else {
        return format!("Running tool: {}", tool_name);
    };
    let first_line = command.lines().next().unwrap_or_default();
    let shown: String = first_line.chars().take(MAX_STATUS_COMMAND_CHARS).collect();
    if shown.len() < command.len() {
        format!("Running `{}…`", shown)
    } else {
        format!("Running `{}`", shown)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leaves_small_output_alone() {
        assert_eq!(cap_tool_output(String::from("ok\n")), "ok\n");
    }

    #[test]
    fn cuts_after_a_whole_line() {
        let output = "x".repeat(100) + "\n" + &"y".repeat(MAX_TOOL_OUTPUT_BYTES);
        let capped = cap_tool_output(output);
        assert!(capped
            .starts_with(&("x".repeat(100) + "\n\n[Output truncated: showing the first 101 of")));
    }

    #[test]
    fn never_splits_a_multibyte_character() {
        let output = "a".repeat(MAX_TOOL_OUTPUT_BYTES - 1) + "éééé";
        assert!(cap_tool_output(output).contains("showing the first 10239 of"));
        let short = format!("{}é", "a".repeat(1999));
        assert_eq!(
            truncate(short, 2000),
            format!("{}\n...[truncated]", "a".repeat(1999))
        );
    }

    #[test]
    fn a_running_shell_command_is_shown_and_other_tools_are_named() {
        assert_eq!(
            describe_running_tool_call("run_shell_command", r#"{"command":"cargo test"}"#),
            "Running `cargo test`"
        );
        assert_eq!(
            describe_running_tool_call("run_shell_command", r#"{"command":"é\necho two"}"#),
            "Running `é…`"
        );
        let long = format!(r#"{{"command":"{}"}}"#, "x".repeat(200));
        assert_eq!(
            describe_running_tool_call("run_shell_command", &long),
            format!("Running `{}…`", "x".repeat(120))
        );
        assert_eq!(
            describe_running_tool_call("run_shell_command", "not json"),
            "Running tool: run_shell_command"
        );
        assert_eq!(
            describe_running_tool_call("read", r#"{"command":"x"}"#),
            "Running tool: read"
        );
    }
}
