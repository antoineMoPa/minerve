//! Converts the markdown minerve writes into Slack's mrkdwn.

use std::sync::LazyLock;

use regex::Regex;

static HEADING: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^#{1,6}\s+(.+)$").unwrap());
static BOLD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\*\*(.+?)\*\*").unwrap());
static LINK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[([^\]]+)\]\((https?://[^)\s]+)\)").unwrap());

/// Slack's escaping applies everywhere, code included (Slack shows `&lt;` as `<` there too), so
/// `<!channel>` in a code block cannot ping anyone. Markdown syntax converts outside code only.
pub fn to_mrkdwn(markdown: &str) -> String {
    escape(markdown)
        .split("```")
        .enumerate()
        .map(|(i, part)| if i % 2 == 1 { format!("```{}```", part) } else { convert_outside_fences(part) })
        .collect::<Vec<_>>()
        .concat()
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn convert_outside_fences(text: &str) -> String {
    text.split('`')
        .enumerate()
        .map(|(i, part)| if i % 2 == 1 { format!("`{}`", part) } else { convert_plain(part) })
        .collect::<Vec<_>>()
        .concat()
}

fn convert_plain(text: &str) -> String {
    let step = HEADING.replace_all(text, "*$1*");
    let step = BOLD.replace_all(&step, "*$1*");
    LINK.replace_all(&step, "<$2|$1>").into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_bold_headings_and_links() {
        assert_eq!(to_mrkdwn("# Title\nsome **bold** and [x](https://a.b/c)"), "*Title*\nsome *bold* and <https://a.b/c|x>");
    }

    #[test]
    fn escapes_slack_control_chars() {
        assert_eq!(to_mrkdwn("a < b & c > d"), "a &lt; b &amp; c &gt; d");
    }

    #[test]
    fn leaves_code_untouched() {
        let text = "run `**x** < y` now\n```\n# not a heading **no** <!channel>\n```\n**yes**";
        assert_eq!(to_mrkdwn(text), "run `**x** &lt; y` now\n```\n# not a heading **no** &lt;!channel&gt;\n```\n*yes*");
    }
}
