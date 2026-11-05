//! Drawing the chat UI: the boot overlay, chat, slash menu and input box.

use super::*;

impl App {
    /// Draws the boot overlay in `area`: a dotted ring around the centered video, and the
    /// current video frame. Only non-blank video characters are written, so the rest of the UI
    /// shows through the transparent background. Once the ring has eroded away, drops the boot
    /// state and schedules a screen clear.
    pub(super) fn draw_boot_overlay(&mut self, frame: &mut Frame, area: Rect) {
        let Some(boot) = self.boot.as_ref() else {
            return;
        };
        let erosion = boot.erosion();
        let sweep = boot.sweep_progress();
        let finished = boot.is_finished();
        let (width, height) = boot.size();
        let text = boot.current_frame();
        if finished {
            self.boot = None;
            return;
        }

        // Fit the video into the area, keeping its aspect ratio. Reserve 2 columns on each side
        // for the ring, and 4 rows for the ring (1 above and below) plus the caption block
        // (1 blank row, then the caption). Work in i32 so the ring can extend past the area.
        let (ax, ay) = (area.x as i32, area.y as i32);
        let (aw, ah) = (area.width as i32, area.height as i32);
        let (sw, sh) = (width as i32, height as i32);
        // 0.8 shrinks the fitted video by 20%, leaving more of the dot pattern visible around it.
        let scale = 0.8 * ((aw - 4).max(1) as f32 / sw as f32).min((ah - 4).max(1) as f32 / sh as f32);
        let w = ((sw as f32 * scale) as i32).max(1);
        let h = ((sh as f32 * scale) as i32).max(1);
        let vx = ax + (aw - w) / 2;
        let vy = ay + ((ah - h - 4).max(0)) / 2 + 1;
        let in_area = |x: i32, y: i32| x >= ax && x < ax + aw && y >= ay && y < ay + ah;
        let buf = frame.buffer_mut();

        // Dot pattern filling the whole chat area, except the video's own cells (drawn over it
        // below). Dots sit on every other cell. Erosion hides cells whose hashed threshold is
        // below the progress, so the pattern disappears unevenly.
        // Distance from the video border, in cells. Vertical distance is doubled because a
        // terminal cell is about twice as tall as it is wide. Cells closer than EDGE_BLEND to the
        // border are the blend band.
        const EDGE_BLEND: i32 = 7;
        let edge_distance = |c: i32, r: i32| (c.min(w - 1 - c)).min(2 * r.min(h - 1 - r));
        // Stable pseudo-random value in [0, 1) per cell.
        let cell_hash = |x: i32, y: i32| {
            let h = ((x as u32).wrapping_mul(73_856_093) ^ (y as u32).wrapping_mul(19_349_663)) % 1000;
            h as f32 / 1000.0
        };

        // Sparse dot grid: one dot every 3 columns and every 2 rows, across the whole area, including
        // behind the video. The video's blank cells show these dots, so the pattern runs seamlessly
        // into the video. Erosion hides dots unevenly.
        let dot_grid = |x: i32, y: i32| x % 3 == 0 && y % 2 == 0;
        for y in ay..ay + ah {
            for x in ax..ax + aw {
                if !dot_grid(x, y) {
                    continue;
                }
                let visible = erosion.is_none_or(|e| cell_hash(x, y) >= e);
                if visible {
                    buf[(x as u16, y as u16)].set_char('·').set_fg(ratatui::style::Color::DarkGray);
                }
            }
        }

        // Video frame, centered inside the ring. Blank characters are transparent.
        if let Some(text) = text {
            // Nearest-neighbour resample of the source characters to the fitted size.
            let src: Vec<Vec<char>> = text.lines().map(|l| l.chars().collect()).collect();
            for r in 0..h {
                let sr = (r * sh / h) as usize;
                for c in 0..w {
                    let sc = (c * sw / w) as usize;
                    let ch = src.get(sr).and_then(|row| row.get(sc)).copied().unwrap_or(' ');
                    let (x, y) = (vx + c, vy + r);
                    // Video cells in the blend band show with a chance rising from the border
                    // inward, so the edge dissolves into the dots.
                    let d = edge_distance(c, r);
                    let show = d >= EDGE_BLEND || cell_hash(x, y) < (d as f32 + 0.5) / EDGE_BLEND as f32;
                    if show && ch != ' ' && in_area(x, y) {
                        buf[(x as u16, y as u16)].set_char(ch);
                    }
                }
            }

        }

        // Caption centered under the video, one blank line below the ring. It stays for the
        // whole boot screen (also after the video stops) until the dash sweep erases it.
        const CAPTION: &str = "loading ancient knowledge";
        let caption_y = vy + h + 2;
        let caption_len = CAPTION.chars().count() as i32;
        let cap_l = vx + (w - caption_len) / 2;
        let cap_r = cap_l + caption_len - 1;
        for (col, ch) in CAPTION.chars().enumerate() {
            let x = cap_l + col as i32;
            if in_area(x, caption_y) {
                buf[(x as u16, caption_y as u16)].set_char(ch).set_fg(ratatui::style::Color::Gray);
            }
        }

        // Dashes frame the caption from the start. After the dots erode, they move in along the
        // caption row toward its center. Every caption cell they pass is blanked; they vanish at
        // the center.
        let dash_l = cap_l - 2;
        let dash_r = cap_r + 2;
        let mid = (cap_l + cap_r) / 2;
        let p = sweep.unwrap_or(0.0);
        let lf = dash_l + ((mid - dash_l) as f32 * p) as i32;
        let rf = dash_r - ((dash_r - mid) as f32 * p) as i32;
        if sweep.is_some() {
            for x in (dash_l..lf).chain(rf + 1..=dash_r) {
                if in_area(x, caption_y) {
                    buf[(x as u16, caption_y as u16)].set_char(' ');
                }
            }
        }
        for x in [lf, rf] {
            if in_area(x, caption_y) {
                buf[(x as u16, caption_y as u16)].set_char('-').set_fg(ratatui::style::Color::White);
            }
        }
    }

    pub(super) fn draw(&mut self, frame: &mut Frame) {
        if let Some(game) = self.tetris.as_ref() {
            game.draw(frame, frame.area());
            return;
        }

        if let Some(picker) = self.model_picker.as_ref() {
            let rows = picker.visible_rows(&self.recent_models);
            picker.draw(frame, frame.area(), &rows);
            return;
        }

        if let Some(picker) = self.session_picker.as_mut() {
            picker.draw(frame, frame.area(), SystemTime::now());
            return;
        }

        // Like the pickers, `/mcp` replaces the whole UI: no message box or input cursor behind it.
        if let Some(manager) = self.mcp_manager.as_ref() {
            manager.draw(frame, frame.area());
            return;
        }
        if let Some(apps) = self.message_apps.as_ref() {
            apps.draw(frame, frame.area());
            return;
        }

        // The rain replaces the whole UI until a key or paste ends it.
        if let Some(matrix) = self.matrix.as_mut() {
            if matrix.draw(frame, frame.area()) {
                return;
            }
            // The fade-out finished: fall through to the normal UI.
            self.matrix = None;
        }

        // Input grows with its wrapped text: 1 line minimum, at most 8 lines of text.
        let inner_width = frame.area().width.saturating_sub(2);
        let row_count = layout_input(&self.input, inner_width).rows.len() as u16;
        let input_height = (row_count + 2).clamp(3, 10);

        let [chat_area, token_area, status_area, input_area, hint_area] = Layout::vertical([
            Constraint::Min(3),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(input_height),
            Constraint::Length(1),
        ])
        .areas(frame.area());

        self.draw_chat(frame, chat_area);

        frame.render_widget(
            Paragraph::new(self.minerve.token_counter.format_usage_summary())
                .style(Style::default().fg(Color::Gray)),
            token_area,
        );
        frame.render_widget(
            Paragraph::new(self.status.as_str())
                .style(Style::default().fg(Color::Magenta)),
            status_area,
        );

        self.draw_input(frame, input_area);
        self.draw_slash_menu(frame, input_area);
        if let (Some(modal), None) = (self.setup_modal.as_ref(), self.boot.as_ref()) {
            modal.draw(frame, frame.area());
        }

        frame.render_widget(
            Paragraph::new(
                "Enter send | Esc clear selection / cancel turn | Shift+Enter newline | Up/Down history | PgUp/PgDn scroll | Ctrl+O tool output | Ctrl+C quit",
            )
            .style(Style::default().fg(Color::DarkGray)),
            hint_area,
        );

        if self.boot.is_some() {
            self.draw_boot_overlay(frame, chat_area);
        }
    }

    pub(super) fn draw_chat(&mut self, frame: &mut Frame, area: Rect) {
        self.chat_area = area;
        let mut lines = self.chat_lines();
        let width = area.width.max(1) as usize;
        let rows_per_line: Vec<usize> = lines
            .iter()
            .map(|line| line.width().div_ceil(width).max(1))
            .collect();
        let total: usize = rows_per_line.iter().sum();

        let max_scroll = total.saturating_sub(area.height as usize);
        self.scroll_up = self.scroll_up.min(max_scroll);
        let top = max_scroll - self.scroll_up;

        // Paragraph::scroll takes a u16 row offset, which long chats overflow. Drop the whole
        // lines above the offset first, then scroll the rest by what is left.
        let mut first_line = 0;
        let mut rows_before = 0;
        while first_line < rows_per_line.len() && rows_before + rows_per_line[first_line] <= top {
            rows_before += rows_per_line[first_line];
            first_line += 1;
        }
        lines.drain(..first_line);
        let offset_in_first_line = u16::try_from(top - rows_before)
            .expect("one chat line taller than u16::MAX rows");

        let paragraph = Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .scroll((offset_in_first_line, 0));
        frame.render_widget(paragraph, area);
        self.chat_top = top;
        self.draw_chat_selection(frame, area);
    }

    /// Highlights the chat selection, and copies it right after the drag ends. A selection made
    /// while another row was at the top is dropped: its cells now hold other text.
    pub(super) fn draw_chat_selection(&mut self, frame: &mut Frame, area: Rect) {
        let copy = std::mem::take(&mut self.copy_selection_on_draw);
        let Some(selection) = self.chat_selection else {
            return;
        };
        if selection.chat_top != self.chat_top || !contains_cell(area, selection.anchor.0, selection.anchor.1) {
            self.chat_selection = None;
            return;
        }
        if copy {
            let text = chat_selection::selected_text(frame.buffer_mut(), area, &selection);
            // Written before ratatui flushes this frame; terminals act on OSC 52 wherever it lands.
            let _ = io::stdout().write_all(chat_selection::osc52_copy(&text).as_bytes());
            self.status = format!("Copied {} chars", text.chars().count());
        }
        chat_selection::highlight(frame.buffer_mut(), area, &selection);
    }

    pub(super) fn chat_lines(&self) -> Vec<Line<'static>> {
        let shown: Vec<&(String, String)> = self
            .transcript
            .iter()
            .filter(|(role, _)| role != "system")
            .collect();
        let mut lines = Vec::new();
        for (role, content) in &shown {
            let (label, style) = match role.as_str() {
                "user" => (
                    "You",
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                ),
                "minerve" => (
                    "Minerve",
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
                GOAL_EVALUATOR_UI_ROLE => (
                    "Goal evaluator",
                    Style::default()
                        .fg(Color::Magenta)
                        .add_modifier(Modifier::BOLD),
                ),
                other => (other, Style::default().add_modifier(Modifier::BOLD)),
            };
            lines.push(Line::styled(format!("{}:", label), style));

            // Tool results are labeled with the function name; collapse them by default.
            let is_tool = !matches!(role.as_str(), "user" | "minerve" | "system" | GOAL_EVALUATOR_UI_ROLE);
            if is_tool && !self.tools_expanded {
                let first = content.lines().next().unwrap_or_default();
                let lines_count = content.lines().count();
                let summary = format!("  [{} line(s)] {}", lines_count, first);
                lines.push(Line::styled(
                    format!("{} ▸ collapsed (Ctrl+O to expand)", summary.trim_end()),
                    Style::default().fg(Color::DarkGray),
                ));
                lines.push(Line::default());
                continue;
            }

            let text = if content.chars().count() > MAX_OUTPUT_LEN {
                if role.as_str() == "minerve" {
                    content.trim().to_string()
                } else {
                    let head: String = content.chars().take(MAX_OUTPUT_LEN).collect();
                    format!("{}\n...[truncated]", head)
                }
            } else {
                content.clone()
            };
            lines.extend(crate::markdown::render(&text));
            lines.push(Line::default());
        }

        // Results of local slash commands. Not sent to the model.
        for notice in &self.notices {
            lines.push(Line::styled(
                "Command:",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ));
            lines.push(Line::styled(notice.clone(), Style::default().fg(Color::Gray)));
            lines.push(Line::default());
        }

        // Shell output queued with `!command`: shown here, sent to the model with the next message.
        for output in &self.pending_command_output {
            lines.push(Line::styled(
                "Shell (queued, sent with your next message):",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ));
            lines.extend(crate::markdown::render(output));
            lines.push(Line::default());
        }
        lines
    }

    /// Popup listing the slash commands that match the input, just above the input box.
    pub(super) fn draw_slash_menu(&self, frame: &mut Frame, input_area: Rect) {
        let matches = self.slash_matches();
        if matches.is_empty() {
            return;
        }
        let height = (matches.len() as u16 + 2).min(input_area.y);
        let area = Rect {
            x: input_area.x,
            y: input_area.y.saturating_sub(height),
            width: input_area.width,
            height,
        };
        // One column for every name, as wide as the longest command plus a space.
        let name_width = SLASH_COMMANDS.iter().map(|(name, _)| name.len()).max().unwrap_or(0) + 1;
        let lines: Vec<Line> = matches
            .iter()
            .map(|(name, description)| {
                Line::from(vec![
                    Span::styled(
                        format!("{:<width$}", name, width = name_width),
                        Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(description.to_string(), Style::default().fg(Color::Gray)),
                ])
            })
            .collect();
        frame.render_widget(Clear, area);
        frame.render_widget(
            Paragraph::new(lines).block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Commands (Tab complete, Enter run) ")
                    .border_style(Style::default().fg(Color::Cyan)),
            ),
            area,
        );
    }

    pub(super) fn draw_input(&mut self, frame: &mut Frame, area: Rect) {
        let title = " Message ";
        let border_color = Color::Cyan;

        // Inside the borders.
        let inner_width = area.width.saturating_sub(2);
        let inner_height = area.height.saturating_sub(2);
        let layout = layout_input(&self.input, inner_width);
        let (col, row) = layout.cursor_cell(self.cursor);
        // Keep the cursor row visible once the wrapped input is taller than the box.
        let scroll_row = row.saturating_add(1).saturating_sub(inner_height);
        self.input_area = area;
        self.input_scroll_row = scroll_row;

        // Rows are pre-wrapped by display width (no Paragraph wrap) so they match the cursor position.
        let text = if self.input.is_empty() {
            // Indented so the tip reads as a hint, apart from where typed text starts.
            let tip = format!(" {}", self.pro_tip.visible_text(Instant::now()));
            Text::from(Span::styled(tip, Style::default().fg(Color::DarkGray)))
        } else {
            Text::from(layout.rows.join("\n"))
        };
        let input = Paragraph::new(text)
            .scroll((scroll_row, 0))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(title)
                    .border_style(Style::default().fg(border_color)),
            );
        frame.render_widget(input, area);

        frame.set_cursor_position((area.x + 1 + col, area.y + 1 + row - scroll_row));
    }
}
