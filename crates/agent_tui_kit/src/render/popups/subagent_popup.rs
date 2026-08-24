//! Subagent popup: persistent live-transcript viewer with block-model layout.
//!
//! The popup stays open while the subagent streams and after it ends (Esc/✕
//! close it). Content comes from the active tool block's structured
//! `live_output` (`ToolOutputLine` spans carry a `ChunkKind`). Committed lines
//! are folded into the layout cache's `lines` (incremental append-only), then
//! grouped into rows mirroring the main area:
//! - Thinking kind runs → a purple thinking card (title + ≤3 tail rows + footer),
//! - ToolCall(+ToolResult/ToolError) → an accent tool card,
//! - user / system / assistant text → plain linear rows.
//!
//! Performance: a full row rebuild happens only when a committed line is
//! appended (once per newline) or the collapse set changes; the per-frame hot
//! path (the in-progress tail growing) only replaces the final tail row in
//! O(1). `render` paints viewport rows and reuses the previous frame's hit
//! table when the (content watermark, width, scroll) stamp is unchanged.

use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Scrollbar, ScrollbarState},
};

use super::{FooterHint, PopupMouseSurface};
use crate::{
    i18n::Messages,
    protocol::{ChunkKind, ToolOutputLine},
    render::{ctx::RenderCtx, popups::render_popup_chrome},
    state::{
        PopupHitRow, RowRole, SubagentLabels, SubagentLayoutCache, SubagentPopup, SubagentRow,
        SubagentSourceLine,
    },
    theme::Theme,
};

/// Max body rows shown for a collapsed thinking card (mirrors ThinkingCell).
const THINKING_COLLAPSED_BODY: usize = 3;

fn footer(m: &Messages) -> Vec<FooterHint> {
    vec![
        FooterHint {
            key: "y",
            label: m.subagent_popup_hint_copy,
        },
        FooterHint {
            key: "j/k",
            label: m.subagent_popup_hint_scroll,
        },
        FooterHint {
            key: "g/G",
            label: m.subagent_popup_hint_top_bottom,
        },
        FooterHint {
            key: "f",
            label: m.subagent_popup_hint_follow,
        },
        FooterHint {
            key: "⏎",
            label: m.subagent_popup_hint_collapse,
        },
        FooterHint {
            key: "Esc",
            label: m.subagent_popup_hint_close,
        },
    ]
}

fn line_text(line: &ToolOutputLine) -> String {
    line.spans.iter().map(|span| span.text.as_str()).collect()
}

/// Seed the layout cache from a completed transcript (plain text, no kinds).
/// Used only when the popup opens after the subagent already ended.
fn fold_completed(detail: &str, cache: &mut SubagentLayoutCache) {
    let mut offset = 0usize;
    for line in detail.lines() {
        let start = cache.raw_text.len();
        if start > 0 {
            cache.raw_text.push('\n');
        }
        cache.raw_text.push_str(line);
        cache.lines.push(SubagentSourceLine {
            text: line.to_string(),
            kind: None,
            source_start: start,
        });
        offset += 1;
    }
    cache.laid_out_committed = offset;
    cache.tail = None;
    cache.rows_built_for = usize::MAX; // force a full rebuild
}

fn truncate(text: &str, width: u16) -> String {
    let max = width as usize;
    if max == 0 || text.chars().count() <= max {
        text.to_string()
    } else {
        let end = text.floor_char_boundary(max.saturating_sub(1));
        format!("{}…", &text[..end])
    }
}

/// Prepare (side-effect phase): fold new committed lines + the tail line, then
/// rebuild `rows` (full on append, tail-only on steady-state streaming).
pub fn prepare_subagent_popup(
    popup: &mut SubagentPopup,
    tools: &crate::state::ToolState,
    _theme: &Theme,
    body_width: u16,
    messages: Messages,
) {
    let labels = SubagentLabels::from_messages(&messages);
    // Ensure a layout cache exists (first prepare after open); keep the chrome
    // labels in sync with the current language every frame (all `&'static str`).
    if popup.layout_cache.is_none() {
        popup.layout_cache = Some(SubagentLayoutCache {
            width: body_width,
            labels,
            laid_out_committed: 0,
            raw_text: String::new(),
            lines: Vec::new(),
            tail: None,
            rows_built_for: usize::MAX,
            rows: Vec::new(),
            line_count: 0,
        });
    } else if let Some(cache) = popup.layout_cache.as_mut() {
        cache.labels = labels;
    }

    let is_live = tools.active.iter().any(|a| a.tool_id == *popup.tool_id);
    popup.live = is_live;

    // Width change resets the layout (both live and completed paths).
    if let Some(cache) = popup.layout_cache.as_mut()
        && cache.width != body_width
    {
        cache.width = body_width;
        cache.laid_out_committed = 0;
        cache.raw_text.clear();
        cache.lines.clear();
        cache.tail = None;
        cache.rows_built_for = usize::MAX;
    }

    // Live source: the active tool block's structured buffer.
    if let Some(output) = tools.active.iter().find(|a| a.tool_id == *popup.tool_id) {
        let buffer = &output.live_output;
        if let Some(cache) = popup.layout_cache.as_mut() {
            let mut idx = cache.laid_out_committed;
            while let Some(line) = buffer.structured_line_at(idx) {
                let text = line_text(line);
                let start = cache.raw_text.len();
                if !cache.raw_text.is_empty() {
                    cache.raw_text.push('\n');
                }
                cache.raw_text.push_str(&text);
                cache.lines.push(SubagentSourceLine {
                    text,
                    kind: line.kind(),
                    source_start: start,
                });
                cache.laid_out_committed = idx + 1;
                idx += 1;
            }
            cache.tail = buffer
                .current_structured_line()
                .map(|l| SubagentSourceLine {
                    text: line_text(l),
                    kind: l.kind(),
                    source_start: cache.raw_text.len(),
                });
        }
        if let Some(cache) = popup.layout_cache.as_mut() {
            rebuild_rows(cache, &popup.collapsed_blocks, body_width);
            cache.line_count = buffer.logical_line_count();
        }
        return;
    }

    // Completed source: fold the final transcript once, then keep it.
    popup.live = false;
    let Some(block) = tools.blocks.iter().find(|b| b.tool_id == *popup.tool_id) else {
        return;
    };
    let Some(detail) = block.output.detail_full.as_deref() else {
        return;
    };
    if let Some(cache) = popup.layout_cache.as_mut() {
        if cache.laid_out_committed == 0 && cache.lines.is_empty() {
            fold_completed(detail, cache);
        }
        rebuild_rows(cache, &popup.collapsed_blocks, body_width);
        cache.line_count = detail.lines().count();
    }
}

/// Rebuild the flat `rows` list.
///
/// Full rebuild only when a committed line was appended (or a collapse toggle
/// forced `rows_built_for = usize::MAX`); otherwise the tail-only fast path
/// replaces the final in-progress row in O(1).
fn rebuild_rows(
    cache: &mut SubagentLayoutCache,
    collapsed: &std::collections::HashSet<usize>,
    width: u16,
) {
    let committed = cache.lines.len();
    if cache.rows_built_for != committed {
        cache.rows = build_rows_full(cache, collapsed, width);
        cache.rows_built_for = committed;
        return;
    }
    // Steady state: only the tail may have changed. Drop the previous tail row
    // (final plain row with no committed line index) and re-append.
    if cache
        .rows
        .last()
        .is_some_and(|r| r.line_idx.is_none() && r.role == RowRole::Plain)
    {
        cache.rows.pop();
    }
    if let Some(tail) = &cache.tail {
        cache.rows.push(make_tail_row(tail, committed, width));
    }
}

/// Build the complete flat row list: block chrome interleaved with content,
/// plus the in-progress tail row.
fn build_rows_full(
    cache: &SubagentLayoutCache,
    collapsed: &std::collections::HashSet<usize>,
    width: u16,
) -> Vec<SubagentRow> {
    let mut rows: Vec<SubagentRow> = Vec::new();
    let n = cache.lines.len();
    let mut i = 0;
    while i < n {
        let kind = cache.lines[i].kind;
        // Run boundary: ToolResult/ToolError lines fold into the *preceding*
        // ToolCall run (one tool card), not an orphan block.
        let mut j = i + 1;
        if kind == Some(ChunkKind::ToolCall) {
            while j < n
                && matches!(
                    cache.lines[j].kind,
                    Some(ChunkKind::ToolResult) | Some(ChunkKind::ToolError)
                )
            {
                j += 1;
            }
        } else {
            while j < n && cache.lines[j].kind == kind {
                j += 1;
            }
        }
        let block_id = i; // stable: starting `lines` index of this run
        let block_collapsed = collapsed.contains(&block_id);

        match kind {
            Some(ChunkKind::Thinking) => {
                rows.push(SubagentRow {
                    text: cache.labels.thinking_title.to_string(),
                    kind,
                    role: RowRole::Title,
                    line_idx: None,
                    source_start: i,
                    source_end: i,
                    block_id,
                });
                let body = if block_collapsed {
                    let skip = (j - i).saturating_sub(THINKING_COLLAPSED_BODY);
                    i + skip..j
                } else {
                    i..j
                };
                for li in body {
                    let line = &cache.lines[li];
                    rows.push(SubagentRow {
                        text: truncate(&line.text, width),
                        kind,
                        role: RowRole::Plain,
                        line_idx: Some(li),
                        source_start: line.source_start,
                        source_end: line.source_start + line.text.len(),
                        block_id,
                    });
                }
                rows.push(SubagentRow {
                    text: cache
                        .labels
                        .lines_footer
                        .replacen("{}", &(j - i).to_string(), 1),
                    kind,
                    role: RowRole::Footer,
                    line_idx: None,
                    source_start: j,
                    source_end: j,
                    block_id,
                });
            }
            Some(ChunkKind::ToolCall) => {
                // Tool card: title row = first line, result rows = the rest.
                let first = &cache.lines[i];
                rows.push(SubagentRow {
                    text: truncate(&first.text, width),
                    kind: Some(ChunkKind::ToolCall),
                    role: RowRole::Title,
                    line_idx: Some(i),
                    source_start: first.source_start,
                    source_end: first.source_start + first.text.len(),
                    block_id,
                });
                let result_kind =
                    if (j - i) > 1 && cache.lines[j - 1].kind == Some(ChunkKind::ToolError) {
                        Some(ChunkKind::ToolError)
                    } else {
                        Some(ChunkKind::ToolResult)
                    };
                for li in i + 1..j {
                    let line = &cache.lines[li];
                    rows.push(SubagentRow {
                        text: truncate(&line.text, width),
                        kind: result_kind,
                        role: RowRole::Plain,
                        line_idx: Some(li),
                        source_start: line.source_start,
                        source_end: line.source_start + line.text.len(),
                        block_id,
                    });
                }
                rows.push(SubagentRow {
                    text: cache.labels.tool_footer.to_string(),
                    kind,
                    role: RowRole::Footer,
                    line_idx: None,
                    source_start: j,
                    source_end: j,
                    block_id,
                });
            }
            _ => {
                // user / system / assistant text / orphan results → linear rows.
                for li in i..j {
                    let line = &cache.lines[li];
                    rows.push(SubagentRow {
                        text: truncate(&line.text, width),
                        kind,
                        role: RowRole::Plain,
                        line_idx: Some(li),
                        source_start: line.source_start,
                        source_end: line.source_start + line.text.len(),
                        block_id,
                    });
                }
            }
        }
        i = j;
    }

    // In-progress tail line as the final plain row.
    if let Some(tail) = &cache.tail {
        rows.push(make_tail_row(tail, n, width));
    }
    rows
}

/// Build the single in-progress tail row.
fn make_tail_row(tail: &SubagentSourceLine, committed: usize, width: u16) -> SubagentRow {
    SubagentRow {
        text: truncate(&tail.text, width),
        kind: tail.kind,
        role: RowRole::Plain,
        line_idx: None,
        source_start: tail.source_start,
        source_end: tail.source_start + tail.text.len(),
        block_id: committed,
    }
}

/// Build the flat visible row list (test / completed-path entry point).
#[cfg(test)]
fn build_rows(
    cache: &SubagentLayoutCache,
    collapsed: &std::collections::HashSet<usize>,
    width: u16,
) -> Vec<SubagentRow> {
    build_rows_full(cache, collapsed, width)
}

/// Render (pure read): paint the viewport rows from the layout cache.
pub fn render_subagent_popup(frame: &mut Frame, area: Rect, ctx: &RenderCtx) -> PopupMouseSurface {
    let mut surface = PopupMouseSurface::default();
    let Some(popup) = ctx.subagent_popup else {
        return surface;
    };
    let popup_area = super::centered_popup_area(area);
    let Some(cache) = popup.layout_cache.as_ref() else {
        render_popup_chrome(frame, popup_area, ctx.theme, "", None);
        return surface;
    };

    let header = header_text(popup, cache);
    let body_area = render_popup_chrome(
        frame,
        popup_area,
        ctx.theme,
        &header,
        Some(&footer(&ctx.messages)),
    );

    // Establish the body background explicitly (render invariants: no residue,
    // no flicker) before painting rows.
    frame
        .buffer_mut()
        .set_style(body_area, Style::default().bg(ctx.theme.bg));

    let viewport = body_area.height as usize;
    let total = cache.rows.len();
    let max_scroll = total.saturating_sub(viewport);
    let scroll = if popup.follow_bottom {
        max_scroll
    } else {
        (popup.scroll as usize).min(max_scroll)
    };

    // Paint the visible window. Each row paints its own background so no cell
    // retains a stale background when the transcript scrolls or resizes.
    let row_style = Style::default().bg(ctx.theme.bg);
    for (visible_row, row) in cache.rows.iter().skip(scroll).take(viewport).enumerate() {
        let styled = style_row(row, ctx.theme);
        let screen_y = body_area.y.saturating_add(visible_row as u16);
        frame.render_widget(
            Paragraph::new(Line::from(styled)).style(row_style),
            Rect::new(body_area.x, screen_y, body_area.width, 1),
        );
    }

    let scrollbar =
        Scrollbar::default().orientation(ratatui::widgets::ScrollbarOrientation::VerticalRight);
    let mut state = ScrollbarState::new(total)
        .viewport_content_length(viewport)
        .position(scroll);
    frame.render_stateful_widget(scrollbar, popup_area, &mut state);

    // Lazy hit table: rebuild only when content/width/scroll changed. The
    // app layer caches `hit_rows` keyed by `stamp` on the popup state; we
    // report the stamp here via the surface write-back.
    let stamp = build_stamp(popup, cache, body_area.width, scroll);
    let hit_rows = match popup.hit_cache.as_ref() {
        Some((s, rows)) if *s == stamp => rows.clone(),
        _ => build_hit_rows(cache, scroll, viewport, body_area),
    };
    surface.hit_rows = hit_rows;
    surface.subagent_hit_stamp = Some(stamp);
    if !popup.follow_bottom {
        surface.subagent_scroll = Some(scroll as u16);
    }

    surface.subagent_popup_area = popup_area;
    surface.body_area = body_area;
    surface
}

fn build_stamp(
    popup: &SubagentPopup,
    cache: &SubagentLayoutCache,
    width: u16,
    scroll: usize,
) -> u64 {
    // FNV-ish combine of the view geometry + content watermark.
    let mut h: u64 = 0xcbf29ce484222325;
    for v in [
        cache.laid_out_committed as u64,
        width as u64,
        scroll as u64,
        cache
            .tail
            .as_ref()
            .map(|t| t.text.len() as u64)
            .unwrap_or(0),
        popup.follow_bottom as u64,
    ] {
        h ^= v;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// Build per-cell hit data for the visible window (byte ranges into raw_text).
fn build_hit_rows(
    cache: &SubagentLayoutCache,
    scroll: usize,
    viewport: usize,
    body_area: Rect,
) -> Vec<PopupHitRow> {
    use unicode_width::UnicodeWidthChar;
    let mut rows = Vec::with_capacity(viewport);
    for (visible_row, row) in cache.rows.iter().skip(scroll).take(viewport).enumerate() {
        let screen_y = body_area.y.saturating_add(visible_row as u16);
        // Column-accurate cells: each grapheme maps cells to its byte range
        // within `source_start..source_end`. Rows that are block chrome
        // (title/footer) have equal start/end → empty hits clamp to the row.
        let text = &row.text;
        let span = row.source_start..row.source_end;
        let mut cells = Vec::new();
        let mut byte = 0usize;
        for ch in text.chars() {
            let w = UnicodeWidthChar::width(ch).unwrap_or(0);
            let start = span
                .start
                .saturating_add(byte.min(span.len().saturating_sub(1)));
            let end = span
                .start
                .saturating_add((byte + ch.len_utf8()).min(span.len()));
            let hit = crate::state::PopupTextHit::new(start, end);
            for _ in 0..w.max(1) {
                cells.push(hit);
            }
            byte += ch.len_utf8();
        }
        rows.push(PopupHitRow {
            screen_y,
            text_x: body_area.x,
            line_start: span.start,
            line_end: span.end,
            cells,
        });
    }
    rows
}

fn header_text(popup: &SubagentPopup, cache: &SubagentLayoutCache) -> String {
    let lines = cache.line_count;
    let tmpl = if popup.live {
        cache.labels.live_header
    } else {
        cache.labels.done_header
    };
    format!(
        " {}{} ",
        popup.title,
        tmpl.replacen("{}", &lines.to_string(), 1)
    )
}

fn style_row(row: &SubagentRow, theme: &Theme) -> Vec<Span<'static>> {
    let fg = row_fg(row, theme);
    let mut style = Style::default().fg(fg);
    if matches!(row.role, RowRole::Title) {
        style = style.add_modifier(Modifier::BOLD);
    }
    if row.kind == Some(ChunkKind::System) {
        style = style.italic();
    }
    vec![Span::styled(row.text.clone(), style)]
}

fn row_fg(row: &SubagentRow, theme: &Theme) -> ratatui::style::Color {
    match row.kind {
        Some(ChunkKind::Thinking) => match row.role {
            RowRole::Title => theme.thinking_card_border(),
            RowRole::Footer => theme.muted,
            _ => theme.thinking_preview_fg(),
        },
        Some(ChunkKind::ToolCall) => theme.accent,
        Some(ChunkKind::ToolResult) => theme.success,
        Some(ChunkKind::ToolError) => theme.error,
        Some(ChunkKind::User) => theme.success,
        Some(ChunkKind::System) => theme.muted,
        Some(ChunkKind::AssistantText) | None => theme.fg,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{ToolOutputBuffer, ToolOutputChunk};
    use crate::state::{SubagentLayoutCache, SubagentPopup, SubagentSourceLine};
    use std::collections::HashSet;

    fn cache_with_width(width: u16) -> SubagentLayoutCache {
        SubagentLayoutCache {
            width,
            labels: SubagentLabels::from_messages(&crate::i18n::Messages::by_language(
                crate::i18n::Language::English,
            )),
            laid_out_committed: 0,
            raw_text: String::new(),
            lines: Vec::new(),
            tail: None,
            rows_built_for: usize::MAX,
            rows: Vec::new(),
            line_count: 0,
        }
    }

    fn prepare_with_buffer(popup: &mut SubagentPopup, buffer: ToolOutputBuffer, width: u16) {
        let mut tools = crate::state::ToolState::default();
        let output = crate::widgets::tool_widget::ToolWidget::new(
            &crate::theme::Theme::from(crate::theme::ThemeName::Ink),
            &crate::i18n::Messages::by_language(crate::i18n::Language::English),
        )
        .with_tool("spawn_subagent")
        .build();
        use crate::state::ActiveToolBlock;
        tools.active.push(ActiveToolBlock {
            phys_idx: 0,
            tool_id: popup.tool_id.clone(),
            output,
            live_output: buffer,
            started_at: std::time::Instant::now(),
        });
        prepare_subagent_popup(
            popup,
            &tools,
            &crate::theme::Theme::from(crate::theme::ThemeName::Ink),
            width,
            crate::i18n::Messages::by_language(crate::i18n::Language::English),
        );
    }

    fn buffer_with(chunks: &[(Option<ChunkKind>, &str)]) -> ToolOutputBuffer {
        let mut b = ToolOutputBuffer::new_full(50_000);
        let mapped: Vec<ToolOutputChunk> = chunks
            .iter()
            .map(|(kind, text)| {
                let mut c = ToolOutputChunk::other(*text);
                if let Some(k) = kind {
                    c = c.with_kind(*k);
                }
                c
            })
            .collect();
        b.push_chunks(&mapped);
        b
    }

    fn popup(tool_id: &str) -> SubagentPopup {
        SubagentPopup {
            title: "Subagent".into(),
            scroll: 0,
            tool_id: tool_id.into(),
            selection: None,
            follow_bottom: true,
            live: true,
            collapsed_blocks: HashSet::new(),
            layout_cache: Some(cache_with_width(80)),
            hit_cache: None,
        }
    }

    #[test]
    fn build_rows_groups_thinking_into_chrome_titled_block() {
        let mut cache = cache_with_width(80);
        cache.raw_text = "t1\nt2\nt3\nt4".to_string();
        cache.lines = vec![
            SubagentSourceLine {
                text: "t1".into(),
                kind: Some(ChunkKind::Thinking),
                source_start: 0,
            },
            SubagentSourceLine {
                text: "t2".into(),
                kind: Some(ChunkKind::Thinking),
                source_start: 3,
            },
            SubagentSourceLine {
                text: "t3".into(),
                kind: Some(ChunkKind::Thinking),
                source_start: 6,
            },
            SubagentSourceLine {
                text: "t4".into(),
                kind: Some(ChunkKind::Thinking),
                source_start: 9,
            },
        ];
        let collapsed: HashSet<usize> = HashSet::new();
        let rows = build_rows(&cache, &collapsed, 80);

        // Title + 4 body + footer
        assert_eq!(rows.len(), 6);
        assert_eq!(rows[0].role, RowRole::Title);
        assert!(rows[0].text.contains("Thinking"));
        assert_eq!(rows[1].role, RowRole::Plain);
        assert_eq!(rows[5].role, RowRole::Footer);
        assert!(rows[5].text.contains("4"));
        assert!(rows.iter().all(|r| r.block_id == 0), "one block → id 0");
    }

    #[test]
    fn collapsed_thinking_block_keeps_only_last_three_body_rows() {
        let mut cache = cache_with_width(80);
        cache.lines = vec![
            SubagentSourceLine {
                text: "a".into(),
                kind: Some(ChunkKind::Thinking),
                source_start: 0,
            },
            SubagentSourceLine {
                text: "b".into(),
                kind: Some(ChunkKind::Thinking),
                source_start: 2,
            },
            SubagentSourceLine {
                text: "c".into(),
                kind: Some(ChunkKind::Thinking),
                source_start: 4,
            },
            SubagentSourceLine {
                text: "d".into(),
                kind: Some(ChunkKind::Thinking),
                source_start: 6,
            },
        ];
        let collapsed = HashSet::from([0usize]);
        let rows = build_rows(&cache, &collapsed, 80);
        assert_eq!(rows.len(), 5);
        let body: Vec<&str> = rows
            .iter()
            .filter(|r| r.role == RowRole::Plain)
            .map(|r| r.text.as_str())
            .collect();
        assert_eq!(body, vec!["b", "c", "d"]);
    }

    #[test]
    fn tool_run_gets_title_result_and_footer() {
        let mut cache = cache_with_width(80);
        cache.lines = vec![
            SubagentSourceLine {
                text: "→ bash ls".into(),
                kind: Some(ChunkKind::ToolCall),
                source_start: 0,
            },
            SubagentSourceLine {
                text: "✓ file.rs".into(),
                kind: Some(ChunkKind::ToolResult),
                source_start: 9,
            },
        ];
        let rows = build_rows(&cache, &HashSet::new(), 80);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].role, RowRole::Title);
        assert!(rows[0].text.starts_with("→ bash"));
        assert_eq!(rows[1].kind, Some(ChunkKind::ToolResult));
        assert_eq!(rows[2].role, RowRole::Footer);
    }

    #[test]
    fn mixed_kinds_split_into_separate_blocks_with_stable_ids() {
        let mut cache = cache_with_width(80);
        cache.lines = vec![
            SubagentSourceLine {
                text: "prompt".into(),
                kind: Some(ChunkKind::User),
                source_start: 0,
            },
            SubagentSourceLine {
                text: "think".into(),
                kind: Some(ChunkKind::Thinking),
                source_start: 7,
            },
            SubagentSourceLine {
                text: "answered".into(),
                kind: Some(ChunkKind::AssistantText),
                source_start: 13,
            },
        ];
        let rows = build_rows(&cache, &HashSet::new(), 80);
        assert_eq!(rows.len(), 5);
        assert_eq!(rows[0].kind, Some(ChunkKind::User));
        assert_eq!(rows[0].block_id, 0);
        assert_eq!(rows[1].role, RowRole::Title);
        assert_eq!(rows[1].kind, Some(ChunkKind::Thinking));
        assert_eq!(rows[1].block_id, 1, "thinking block id = its starting line");
        assert_eq!(rows[4].kind, Some(ChunkKind::AssistantText));
        assert_eq!(rows[4].block_id, 2);
    }

    #[test]
    fn prepare_appends_incrementally_and_rebuilds_only_on_commit() {
        let mut p = popup("t9");
        prepare_with_buffer(&mut p, buffer_with(&[(Some(ChunkKind::User), "hi\n")]), 80);
        let cache = p.layout_cache.as_ref().unwrap();
        assert_eq!(cache.laid_out_committed, 1);
        assert_eq!(cache.lines.len(), 1);
        assert_eq!(cache.rows.len(), 1);
        assert_eq!(cache.rows_built_for, 1);
        let first = cache.raw_text.clone();

        prepare_with_buffer(
            &mut p,
            buffer_with(&[
                (Some(ChunkKind::User), "hi\n"),
                (Some(ChunkKind::Thinking), "reason\n"),
            ]),
            80,
        );
        let cache = p.layout_cache.as_ref().unwrap();
        assert_eq!(cache.laid_out_committed, 2, "watermark advanced");
        assert_eq!(cache.lines.len(), 2);
        assert!(
            cache.raw_text.starts_with(&first),
            "raw text appends, never rewrites"
        );
        // thinking block: title + 1 body + footer appended after the user row.
        assert_eq!(cache.rows.len(), 4);
        assert_eq!(cache.rows[1].role, RowRole::Title);
        assert_eq!(cache.rows[1].kind, Some(ChunkKind::Thinking));
    }

    #[test]
    fn tail_growth_replaces_only_the_last_row() {
        let mut p = popup("t10");
        prepare_with_buffer(
            &mut p,
            buffer_with(&[(Some(ChunkKind::Thinking), "reason")]),
            80,
        );
        let cache = p.layout_cache.as_ref().unwrap();
        assert_eq!(
            cache.laid_out_committed, 0,
            "no newline → nothing committed"
        );
        assert!(cache.tail.is_some());
        assert_eq!(cache.rows.len(), 1, "only the tail row");
        assert_eq!(cache.rows_built_for, 0);

        // Steady-state growth: same committed count, longer tail → same row count.
        prepare_with_buffer(
            &mut p,
            buffer_with(&[(Some(ChunkKind::Thinking), "reasoning")]),
            80,
        );
        let cache = p.layout_cache.as_ref().unwrap();
        assert_eq!(cache.laid_out_committed, 0);
        assert_eq!(cache.rows_built_for, 0);
        assert_eq!(cache.rows.len(), 1);
        assert_eq!(cache.rows[0].text, "reasoning");
    }

    #[test]
    fn ten_k_line_transcript_lays_out_within_budget() {
        let mut chunks: Vec<(Option<ChunkKind>, String)> = Vec::with_capacity(10_000);
        for i in 0..10_000 {
            let kind = match i % 4 {
                0 => Some(ChunkKind::User),
                1 => Some(ChunkKind::Thinking),
                2 => Some(ChunkKind::ToolCall),
                _ => Some(ChunkKind::AssistantText),
            };
            chunks.push((kind, format!("line-{i:05} with some padded text\n")));
        }
        let chunk_refs: Vec<(Option<ChunkKind>, &str)> =
            chunks.iter().map(|(k, t)| (*k, t.as_str())).collect();
        let full_buffer = buffer_with(&chunk_refs);

        // Lay out the first 9999 lines (untimed).
        let mut p = popup("perf-10k");
        {
            let mut prefix = chunk_refs.clone();
            prefix.truncate(9_999);
            let prefix_buffer = buffer_with(&prefix);
            prepare_with_buffer(&mut p, prefix_buffer, 80);
        }
        let cache = p.layout_cache.as_ref().unwrap();
        assert_eq!(cache.laid_out_committed, 9_999);
        assert_eq!(cache.lines.len(), 9_999);

        // Time the incremental append of the final committed line.
        let start = std::time::Instant::now();
        prepare_with_buffer(&mut p, full_buffer, 80);
        let elapsed = start.elapsed();
        let cache = p.layout_cache.as_ref().unwrap();
        assert_eq!(cache.laid_out_committed, 10_000);
        assert_eq!(cache.lines.len(), 10_000);
        assert!(cache.rows.len() > 9_999);
        assert!(
            elapsed.as_millis() < 16,
            "incremental tail append onto 10k lines took {}ms (> 16ms budget)",
            elapsed.as_millis()
        );
    }

    #[test]
    fn hit_stamp_changes_only_when_view_moves() {
        let mut cache = cache_with_width(80);
        cache.lines = vec![SubagentSourceLine {
            text: "alpha beta".into(),
            kind: Some(ChunkKind::User),
            source_start: 0,
        }];
        cache.rows = build_rows(&cache, &HashSet::new(), 80);
        let p = SubagentPopup {
            title: "t".into(),
            scroll: 0,
            tool_id: "t1".into(),
            selection: None,
            follow_bottom: true,
            live: true,
            collapsed_blocks: HashSet::new(),
            layout_cache: Some(cache),
            hit_cache: None,
        };
        let s1 = build_stamp(&p, p.layout_cache.as_ref().unwrap(), 80, 0);
        let s2 = build_stamp(&p, p.layout_cache.as_ref().unwrap(), 80, 0);
        assert_eq!(s1, s2, "unchanged view → same stamp");
        let s3 = build_stamp(&p, p.layout_cache.as_ref().unwrap(), 80, 1);
        assert_ne!(s1, s3, "scroll change → new stamp");
    }
}
