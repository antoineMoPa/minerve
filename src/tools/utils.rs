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
}
