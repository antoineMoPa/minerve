//! Minimal markdown -> ratatui `Line` renderer for chat messages.
//!
//! Supports: `**bold**`, `*italic*`, `` `inline code` ``, `# headings`,
//! and `-`/`*` bullets. Anything else passes through as plain text.
//! Deliberately hand-rolled (no extra crate) and line-based, so each source
//! line maps to exactly one rendered line. That keeps layout predictable.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

/// Render markdown text into styled lines, one per source line.
/// Fenced code blocks (```) are shown verbatim in yellow, with no inline parsing.
pub fn render(text: &str) -> Vec<Line<'static>> {
    let lines: Vec<&str> = text.split('\n').collect();
    let mut out = Vec::new();
    let mut in_code = false;
    let mut i = 0;
    while i < lines.len() {
        let raw = lines[i];
        if !in_code && is_table_start(&lines, i) {
            let end = table_end(&lines, i);
            out.extend(render_table(&lines[i..end]));
            i = end;
            continue;
        }
        if raw.trim_start().starts_with("```") {
            in_code = !in_code;
            out.push(Line::styled(raw.to_string(), Style::default().fg(Color::Yellow)));
        } else if in_code {
            out.push(Line::styled(raw.to_string(), Style::default().fg(Color::Yellow)));
        } else {
            out.push(render_line(raw));
        }
        i += 1;
    }
    out
}

/// A table starts with a `|` row immediately followed by a separator row
/// (`|---|:---:|`) with the same number of cells.
fn is_table_start(lines: &[&str], i: usize) -> bool {
    if i + 1 >= lines.len() || !lines[i].contains('|') || !is_separator(lines[i + 1]) {
        return false;
    }
    parse_cells(lines[i]).len() == parse_cells(lines[i + 1]).len()
}

/// Index one past the last row of the table that starts at `i`.
fn table_end(lines: &[&str], i: usize) -> usize {
    let mut j = i + 2;
    while j < lines.len() && lines[j].contains('|') && !lines[j].trim().is_empty() {
        j += 1;
    }
    j
}

fn is_separator(line: &str) -> bool {
    let cells = parse_cells(line);
    !cells.is_empty()
        && cells.iter().all(|c| {
            let core = c.trim_matches(':');
            !core.is_empty() && core.chars().all(|ch| ch == '-')
        })
}

/// Split a `| a | b |` row into trimmed cell texts.
fn parse_cells(line: &str) -> Vec<String> {
    let t = line.trim();
    let t = t.strip_prefix('|').unwrap_or(t);
    let t = t.strip_suffix('|').unwrap_or(t);
    t.split('|').map(|c| c.trim().to_string()).collect()
}

/// Lay out a table as box-drawn lines with columns padded to the widest cell.
fn render_table(rows: &[&str]) -> Vec<Line<'static>> {
    let header = parse_cells(rows[0]);
    let ncols = header.len();
    let body: Vec<Vec<String>> = rows[2..]
        .iter()
        .map(|r| {
            let mut cells = parse_cells(r);
            cells.resize(ncols, String::new());
            cells
        })
        .collect();

    // Each cell as styled spans; the header is bold on top of inline styling.
    let header_cells: Vec<Vec<Span<'static>>> = header.iter().map(|c| inline(c)).collect();
    let body_cells: Vec<Vec<Vec<Span<'static>>>> = body
        .iter()
        .map(|r| r.iter().map(|c| inline(c)).collect())
        .collect();

    let width_of = |spans: &Vec<Span<'static>>| -> usize {
        spans.iter().map(|s| s.content.chars().count()).sum()
    };
    let mut widths = vec![3usize; ncols];
    for (c, spans) in header_cells.iter().enumerate() {
        widths[c] = widths[c].max(width_of(spans));
    }
    for row in &body_cells {
        for (c, spans) in row.iter().enumerate() {
            widths[c] = widths[c].max(width_of(spans));
        }
    }

    let border = |left: &str, mid: &str, right: &str| -> Line<'static> {
        let segs: Vec<String> = widths.iter().map(|w| "─".repeat(w + 2)).collect();
        Line::raw(format!("{left}{}{right}", segs.join(mid)))
    };

    let row_line = |cells: &Vec<Vec<Span<'static>>>, bold: bool| -> Line<'static> {
        let mut spans = vec![Span::raw("│".to_string())];
        for (c, cell) in cells.iter().enumerate() {
            spans.push(Span::raw(" ".to_string()));
            for s in cell {
                let style = if bold {
                    s.style.add_modifier(Modifier::BOLD).fg(Color::Cyan)
                } else {
                    s.style
                };
                spans.push(Span::styled(s.content.clone(), style));
            }
            let pad = widths[c] - width_of(cell);
            spans.push(Span::raw(" ".repeat(pad + 1)));
            spans.push(Span::raw("│".to_string()));
        }
        Line::from(spans)
    };

    let mut out = vec![border("┌", "┬", "┐"), row_line(&header_cells, true), border("├", "┼", "┤")];
    for row in &body_cells {
        out.push(row_line(row, false));
    }
    out.push(border("└", "┴", "┘"));
    out
}

fn render_line(raw: &str) -> Line<'static> {
    let trimmed = raw.trim_start();
    let indent = &raw[..raw.len() - trimmed.len()];

    if let Some(rest) = heading(trimmed) {
        let style = Style::default().add_modifier(Modifier::BOLD).fg(Color::Cyan);
        return Line::from(Span::styled(rest.to_string(), style));
    }

    if let Some(rest) = bullet(trimmed) {
        let mut spans = vec![Span::raw(format!("{indent}• "))];
        spans.extend(inline(rest));
        return Line::from(spans);
    }

    let mut spans = Vec::new();
    if !indent.is_empty() {
        spans.push(Span::raw(indent.to_string()));
    }
    spans.extend(inline(trimmed));
    Line::from(spans)
}

/// `# Title` / `## Title` ... -> the title text without the hashes.
fn heading(line: &str) -> Option<&str> {
    let hashes = line.chars().take_while(|c| *c == '#').count();
    if (1..=6).contains(&hashes) {
        let rest = &line[hashes..];
        if let Some(stripped) = rest.strip_prefix(' ') {
            return Some(stripped);
        }
    }
    None
}

/// `- item` or `* item` -> the item text.
fn bullet(line: &str) -> Option<&str> {
    line.strip_prefix("- ").or_else(|| line.strip_prefix("* "))
}

/// Parse inline emphasis and code spans into styled spans.
fn inline(text: &str) -> Vec<Span<'static>> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut plain = String::new();
    let mut bold = false;
    let mut italic = false;
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;

    let flush = |plain: &mut String, spans: &mut Vec<Span<'static>>, bold: bool, italic: bool| {
        if !plain.is_empty() {
            let mut style = Style::default();
            if bold {
                style = style.add_modifier(Modifier::BOLD);
            }
            if italic {
                style = style.add_modifier(Modifier::ITALIC);
            }
            spans.push(Span::styled(std::mem::take(plain), style));
        }
    };

    while i < chars.len() {
        // Inline code: take everything up to the closing backtick verbatim.
        if chars[i] == '`' {
            if let Some(end) = chars[i + 1..].iter().position(|c| *c == '`') {
                flush(&mut plain, &mut spans, bold, italic);
                let code: String = chars[i + 1..i + 1 + end].iter().collect();
                spans.push(Span::styled(
                    code,
                    Style::default().fg(Color::Yellow),
                ));
                i += end + 2;
                continue;
            }
        }
        // Bold toggle: **
        if chars[i] == '*' && chars.get(i + 1) == Some(&'*') {
            flush(&mut plain, &mut spans, bold, italic);
            bold = !bold;
            i += 2;
            continue;
        }
        // Italic toggle: single * opens only before a non-space char and
        // closes only after a non-space char, so `2 * 3 * 4` stays literal.
        if chars[i] == '*' {
            let next_ok = chars.get(i + 1).is_some_and(|c| !c.is_whitespace());
            let prev_ok = i > 0 && !chars[i - 1].is_whitespace();
            if (!italic && next_ok) || (italic && prev_ok) {
                flush(&mut plain, &mut spans, bold, italic);
                italic = !italic;
                i += 1;
                continue;
            }
        }
        plain.push(chars[i]);
        i += 1;
    }
    flush(&mut plain, &mut spans, bold, italic);

    if spans.is_empty() {
        spans.push(Span::raw(String::new()));
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_of(line: &Line) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn plain_text_passes_through() {
        let lines = render("hello world");
        assert_eq!(lines.len(), 1);
        assert_eq!(text_of(&lines[0]), "hello world");
    }

    #[test]
    fn bold_and_italic_strip_markers() {
        let lines = render("a **b** *c*");
        assert_eq!(text_of(&lines[0]), "a b c");
        let bold_span = lines[0].spans.iter().find(|s| s.content == "b").unwrap();
        assert!(bold_span.style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn inline_code_keeps_contents() {
        let lines = render("run `cargo check` now");
        assert_eq!(text_of(&lines[0]), "run cargo check now");
    }

    #[test]
    fn heading_drops_hashes() {
        let lines = render("## Title");
        assert_eq!(text_of(&lines[0]), "Title");
    }

    #[test]
    fn bullet_becomes_dot() {
        let lines = render("- item");
        assert_eq!(text_of(&lines[0]), "• item");
    }

    #[test]
    fn code_fence_contents_are_not_inline_parsed() {
        let lines = render("```\n**raw** `x`\n```\n**after**");
        assert_eq!(text_of(&lines[1]), "**raw** `x`");
        assert_eq!(text_of(&lines[0]), "```");
        assert!(lines[3].spans[0].style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn table_is_laid_out_with_padded_columns() {
        let lines = render("| Name | Qty |\n|---|:---:|\n| apple | 3 |\n| kiwi | 12 |");
        let rows: Vec<String> = lines.iter().map(text_of).collect();
        assert_eq!(rows.len(), 6);
        assert_eq!(rows[1], "│ Name  │ Qty │");
        assert_eq!(rows[3], "│ apple │ 3   │");
        assert_eq!(rows[4], "│ kiwi  │ 12  │");
        // All rendered rows share one width.
        let w = rows[0].chars().count();
        assert!(rows.iter().all(|r| r.chars().count() == w));
    }

    #[test]
    fn spaced_asterisks_are_literal() {
        let lines = render("2 * 3 * 4");
        assert_eq!(text_of(&lines[0]), "2 * 3 * 4");
        assert!(!lines[0].spans.iter().any(|s| s.style.add_modifier.contains(Modifier::ITALIC)));
    }

    #[test]
    fn triple_asterisks_are_bold_italic() {
        let lines = render("***triple***");
        assert_eq!(text_of(&lines[0]), "triple");
        let s = &lines[0].spans[0];
        assert!(s.style.add_modifier.contains(Modifier::BOLD | Modifier::ITALIC));
    }

    #[test]
    fn unclosed_code_backtick_is_literal() {
        let lines = render("unclosed ` tick");
        assert_eq!(text_of(&lines[0]), "unclosed ` tick");
    }

    #[test]
    fn lone_pipe_line_is_not_a_table() {
        let lines = render("a | b\nplain");
        assert_eq!(text_of(&lines[0]), "a | b");
    }

    #[test]
    fn one_line_per_source_line() {
        assert_eq!(render("a\nb\nc").len(), 3);
    }
}
