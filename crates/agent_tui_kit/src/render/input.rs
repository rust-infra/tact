//! Input box / command line / pending block rendering (pure `&RenderCtx`).
//!
//! Moved verbatim from `crates/tui/src/render/input.rs` in the Ctx migration
//! slice. The `&mut App` phase stays in `crates/tui` (scroll cache update,
//! cancel-button hit area, voice button area); this module only renders.

use std::collections::HashSet;

use ratatui::{
    Frame,
    layout::{Alignment, Rect},
    style::{Color, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Clear, Paragraph},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::{
    render::{ctx::RenderCtx, slash_style::style_input_skill_line},
    state::InputMode,
};

/// Soft-wrap a logical line into display-line slices no wider than
/// `max_width` columns, splitting at character boundaries.
///
/// The renderer draws exactly these rows (Paragraph stays unwrapped), so the
/// caret computed from the same function lands where the text is visible.
pub fn wrap_line(line: &str, max_width: usize) -> Vec<&str> {
    let mut rows = Vec::new();
    if max_width == 0 {
        rows.push(line);
        return rows;
    }
    let mut start = 0;
    let mut width = 0;
    for (byte_idx, ch) in line.char_indices() {
        let w = UnicodeWidthChar::width(ch).unwrap_or(0);
        if width + w > max_width && width > 0 {
            rows.push(&line[start..byte_idx]);
            start = byte_idx;
            width = 0;
        }
        width += w;
    }
    rows.push(&line[start..]);
    rows
}

/// Display (row, column) of a caret sitting at logical `cursor_col` within
/// `line`, after soft-wrapping at `max_width`. The caret is placed at the end
/// of the prefix's last display row.
pub fn caret_in_wrapped(line: &str, max_width: usize, cursor_col: usize) -> (usize, usize) {
    let mut acc = 0;
    let mut prefix_len = 0;
    for (byte_idx, ch) in line.char_indices() {
        if acc >= cursor_col {
            break;
        }
        let w = UnicodeWidthChar::width(ch).unwrap_or(0);
        if acc + w > cursor_col {
            break;
        }
        acc += w;
        prefix_len = byte_idx + ch.len_utf8();
    }
    let wrapped = wrap_line(&line[..prefix_len], max_width);
    let row = wrapped.len().saturating_sub(1);
    let col = UnicodeWidthStr::width(wrapped.last().copied().unwrap_or(""));
    (row, col)
}

/// Display row of the caret after soft-wrapping, plus its column within that
/// row. Pure (hosts use it in their prepare phase to keep the caret visible).
pub fn caret_display_line(input: &str, input_cursor: usize, inner_width: usize) -> (usize, usize) {
    // Logical rows are separated by explicit `\n`; the caret line/column is
    // computed on the raw input, then mapped through soft-wrapping below.
    let mut cursor_line = 0;
    let mut cursor_col = 0;
    for (i, c) in input.char_indices() {
        if i >= input_cursor {
            break;
        }
        if c == '\n' {
            cursor_line += 1;
            cursor_col = 0;
        } else {
            cursor_col += UnicodeWidthChar::width(c).unwrap_or(0);
        }
    }

    let lines: Vec<&str> = input.split('\n').collect();

    // Caret position after soft-wrapping: rows before the caret's logical
    // line plus the caret's row within that line.
    let cursor_logical = lines.get(cursor_line).copied().unwrap_or("");
    let (caret_row_in_line, caret_col_in_line) =
        caret_in_wrapped(cursor_logical, inner_width, cursor_col);
    let prior_display_rows: usize = lines[..cursor_line]
        .iter()
        .map(|line| wrap_line(line, inner_width).len())
        .sum();
    (prior_display_rows + caret_row_in_line, caret_col_in_line)
}

/// Render command-line input (Palette mode).
pub fn render_command_line(frame: &mut Frame, area: Rect, ctx: &RenderCtx) {
    let content = ctx.cmd_line.to_string();
    let para = Paragraph::new(Line::from(Span::styled(
        content,
        Style::default()
            .fg(ctx.theme.input_box_fg)
            .bg(ctx.theme.input_box_bg),
    )))
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(ctx.theme.border))
            .title(ctx.messages.command_title),
    )
    .style(Style::default().bg(ctx.theme.input_box_bg));
    frame.render_widget(Clear, area);
    frame.render_widget(para, area);
    let cmd_width = UnicodeWidthStr::width(ctx.cmd_line) as u16;
    frame.set_cursor_position((
        (area.x + 1 + cmd_width).min(area.x + area.width.saturating_sub(2)),
        area.y + 1,
    ));
}

/// Render the input box (insert mode) or command line (palette mode).
///
/// Returns the pending `[Cancel]` button hit area (empty when inactive) so the
/// host can record it for mouse handling — the kit render pass stays pure.
pub fn render_input_box(
    frame: &mut Frame,
    area: Rect,
    ctx: &RenderCtx,
    skill_names: &HashSet<&str>,
) -> Rect {
    if ctx.input_mode == InputMode::Palette {
        render_command_line(frame, area, ctx);
        return Rect::default();
    }

    // Codex-style queued messages: the hint + "↳ message" rows sit above the
    // bordered input box, which shrinks to the remaining height. The pending
    // block paints its own background (TUI invariant: no shadow residue).
    let pending_lines = (1 + ctx.pending_messages.len()).min(4) as u16;
    let (area, cancel_area) = if ctx.pending_messages.is_empty() {
        (area, Rect::default())
    } else {
        let pending_area = Rect::new(area.x, area.y, area.width, pending_lines);
        let cancel_area = render_pending_block(frame, pending_area, ctx);
        (
            Rect::new(
                area.x,
                area.y + pending_lines,
                area.width,
                area.height - pending_lines,
            ),
            cancel_area,
        )
    };

    let inner_width = area.width.saturating_sub(2).max(1) as usize;
    let (cursor_display_line, caret_col_in_line) =
        caret_display_line(ctx.input, ctx.input_cursor, inner_width);

    let lines: Vec<&str> = ctx.input.split('\n').collect();
    // Every display row the input occupies after soft-wrapping.
    let display_lines: Vec<&str> = lines
        .iter()
        .flat_map(|line| wrap_line(line, inner_width))
        .collect();

    let visible_lines = area.height.saturating_sub(2) as usize;

    let start = ctx.input_scroll as usize;
    let end = (start + visible_lines).min(display_lines.len());
    let placeholder_mode = ctx.input.is_empty();

    let display: Text<'static> = if placeholder_mode {
        Text::from(Span::styled(
            ctx.messages.input_box_placeholder.to_string(),
            Style::default()
                .fg(Color::Rgb(100, 100, 120))
                .bg(ctx.theme.input_box_bg),
        ))
    } else {
        let styled_lines: Vec<Line<'static>> = display_lines[start..end]
            .iter()
            .map(|line| {
                style_input_skill_line(line, skill_names, ctx.theme).unwrap_or_else(|| {
                    Line::from(Span::styled(
                        (*line).to_string(),
                        Style::default()
                            .fg(ctx.theme.input_box_fg)
                            .bg(ctx.theme.input_box_bg),
                    ))
                })
            })
            .collect();
        Text::from(styled_lines)
    };

    let line_stats = if ctx.input.is_empty() {
        None
    } else {
        Some((display_lines.len(), ctx.input.chars().count()))
    };
    let bottom_title = line_stats
        .map(|(total_lines, total_chars)| format!(" 📝 {total_lines}L · {total_chars}chars "))
        .unwrap_or_default();

    // Determine border color: accent when focused (insert mode), normal otherwise
    let border_color = if ctx.input_mode == InputMode::Insert {
        ctx.theme.accent
    } else {
        ctx.theme.border
    };

    // Left title + centered voice as separate Block titles so the top border
    // stays visible between them (space-padding with a bg would eat the line).
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_type(ctx.theme.block_border_type())
        .border_style(Style::default().fg(border_color))
        .title(Span::raw(ctx.messages.input_box_title))
        .title_bottom(bottom_title);
    if let Some((voice_label, voice_style)) = &ctx.input_voice_title {
        block = block.title(
            Line::from(Span::styled(voice_label.clone(), *voice_style))
                .alignment(Alignment::Center),
        );
    }

    let input_para = Paragraph::new(display)
        .style(Style::default().bg(ctx.theme.input_box_bg))
        .block(block);
    frame.render_widget(Clear, area);
    frame.render_widget(input_para, area);

    let cursor_x =
        (area.x + 1 + caret_col_in_line as u16).min(area.x + area.width.saturating_sub(2));
    let cursor_y = {
        if ctx.input_scroll as usize > cursor_display_line {
            eprintln!(
                "DEBUG input.rs: scroll={} > cursor_display_line={} area={:?} input={:?} cursor={}",
                ctx.input_scroll, cursor_display_line, area, ctx.input, ctx.input_cursor
            );
        }
        area.y + 1 + cursor_display_line.saturating_sub(ctx.input_scroll as usize) as u16
    };
    frame.set_cursor_position((cursor_x, cursor_y));

    cancel_area
}

/// Truncate a string to at most `max` display columns, appending `…` when cut.
pub fn truncate_to_width(s: &str, max: usize) -> String {
    if UnicodeWidthStr::width(s) <= max {
        return s.to_string();
    }
    let mut out = String::new();
    let mut width = 0;
    for c in s.chars() {
        let cw = UnicodeWidthChar::width(c).unwrap_or(0);
        if width + cw > max.saturating_sub(1) {
            break;
        }
        out.push(c);
        width += cw;
    }
    out.push('…');
    out
}

/// Codex-style pending block: a hint line plus one `↳ message` row per queued
/// message, drawn above the input box while the agent is busy. The hint row
/// carries a clickable `[Cancel]` button (mouse) — the only way to drop the
/// queued messages; they otherwise auto-submit when the current task finishes.
///
/// Returns the `[Cancel]` button hit area (empty when hidden).
fn render_pending_block(frame: &mut Frame, area: Rect, ctx: &RenderCtx) -> Rect {
    if area.height == 0 || area.width == 0 {
        return Rect::default();
    }
    let inner_width = area.width.saturating_sub(2).max(1) as usize;
    let cancel_label = ctx.messages.pending_cancel_btn;
    let cancel_width = UnicodeWidthStr::width(cancel_label) + 2; // "[label]"
    let can_show_cancel = !ctx.pending_messages.is_empty() && inner_width >= cancel_width + 30;
    let hint = ctx.messages.pending_submit_hint;
    let hint_max = if can_show_cancel {
        inner_width.saturating_sub(cancel_width).saturating_sub(1)
    } else {
        inner_width
    };
    let hint_text = truncate_to_width(hint, hint_max);
    let hint_width = UnicodeWidthStr::width(hint_text.as_str());
    let cancel_area = if can_show_cancel {
        Rect::new(
            area.x + hint_width as u16 + 1,
            area.y,
            cancel_width as u16,
            1,
        )
    } else {
        Rect::default()
    };

    let mut lines = vec![Line::from(Span::styled(
        hint_text,
        Style::default()
            .fg(ctx.theme.warning)
            .bg(ctx.theme.input_box_bg),
    ))];
    if !cancel_area.is_empty() {
        lines[0].spans.push(Span::styled(
            " ",
            Style::default().bg(ctx.theme.input_box_bg),
        ));
        lines[0].spans.push(Span::styled(
            format!("[{cancel_label}]"),
            Style::default()
                .fg(ctx.theme.warning)
                .bg(ctx.theme.input_box_bg),
        ));
    }
    for pending in ctx.pending_messages.iter().take(3) {
        let text = truncate_to_width(&format!("↳ {}", pending.display), inner_width);
        lines.push(Line::from(Span::styled(
            text,
            Style::default()
                .fg(ctx.theme.input_box_fg)
                .bg(ctx.theme.input_box_bg),
        )));
    }
    frame.render_widget(
        Paragraph::new(Text::from(lines)).style(Style::default().bg(ctx.theme.input_box_bg)),
        area,
    );
    cancel_area
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_line_splits_at_column_width() {
        assert_eq!(wrap_line("", 4), vec![""]);
        assert_eq!(wrap_line("abcd", 2), vec!["ab", "cd"]);
        assert_eq!(wrap_line("abcdef", 2), vec!["ab", "cd", "ef"]);
        // CJK wide characters count as two columns.
        assert_eq!(wrap_line("中文abc", 4), vec!["中文", "abc"]);
        assert_eq!(wrap_line("中文", 3), vec!["中", "文"]);
        // Oversized single character stays on its own row.
        assert_eq!(wrap_line("ab中", 3), vec!["ab", "中"]);
    }

    #[test]
    fn caret_in_wrapped_maps_logical_column_to_display_row() {
        assert_eq!(caret_in_wrapped("abcdef", 2, 0), (0, 0));
        assert_eq!(caret_in_wrapped("abcdef", 2, 2), (0, 2));
        assert_eq!(caret_in_wrapped("abcdef", 2, 4), (1, 2));
        assert_eq!(caret_in_wrapped("abcdef", 2, 6), (2, 2));
        // CJK caret: "中文ab" wraps as ["中文", "ab"]; caret after "中文" (col 4).
        assert_eq!(caret_in_wrapped("中文ab", 4, 4), (0, 4));
        assert_eq!(caret_in_wrapped("中文ab", 4, 6), (1, 2));
    }

    #[test]
    fn truncate_to_width_appends_ellipsis_for_cjk() {
        assert_eq!(truncate_to_width("abc", 5), "abc");
        assert_eq!(truncate_to_width("abcdef", 4), "abc…");
        // CJK wide chars count as two columns; truncation targets `max` cols.
        assert_eq!(truncate_to_width("中文abc", 5), "中文…");
        assert_eq!(truncate_to_width("中文abc", 6), "中文a…");
        assert_eq!(truncate_to_width("中文abc", 7), "中文abc");
    }
}
