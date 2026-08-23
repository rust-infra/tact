//! Pure log-panel renderer (Phase 3).
//!
//! Reads the caches built by the app-side prepare phase and draws the bordered
//! log panel, its per-row cells, the code-card overlays, the loading spinner,
//! the scrollbar, and the left-border restamp. Takes `&RenderCtx` only — no
//! mutable app state — so it can be reused by any ratatui host that populates
//! the same context.

use ratatui::{
    Frame,
    buffer::{Buffer, CellDiffOption},
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Scrollbar, ScrollbarState},
};

use crate::{
    render::{
        cells::{
            code::render_code_cards,
            separator::{
                MessageSeparator, TaskEndSeparator, is_task_end_separator, task_end_elapsed_secs,
            },
            text::TextCell,
            thinking::ThinkingCell,
            tool::ToolCell,
        },
        ctx::RenderCtx,
        log_column::LogColumnRenderer,
        util::LOG_THINKING_INDENT,
    },
    state::{LogItemKind, find_thinking_at_logical, log_indent_at},
    theme::Theme,
    widgets::tool_widget::TOOL_RUNNING_SPINNER,
};

/// Phase 3: pure render — build cells from the caches and draw. Reads only.
pub fn render_log_panel_pure(frame: &mut Frame, area: Rect, ctx: &RenderCtx, borders: Borders) {
    let top = u16::from(borders.contains(Borders::TOP));
    let bottom = u16::from(borders.contains(Borders::BOTTOM));
    let left = u16::from(borders.contains(Borders::LEFT));
    let right = u16::from(borders.contains(Borders::RIGHT));

    let visual_scroll = ctx.log_scroll.visual_top;
    let visible_height = ctx.log_scroll.height as usize;
    let total_logical =
        ctx.log_scroll.visible_indices.len() + usize::from(!ctx.stream.buffer.is_empty());
    let total_visual = *ctx.log_scroll.visual_start_cache.last().unwrap_or(&0);
    let end_visual = (visual_scroll + visible_height).min(total_visual);

    // Reverse-map visual viewport bounds back to logical row range for cell building.
    let vs_cache = &ctx.log_scroll.visual_start_cache;
    let logical_start = vs_cache
        .binary_search(&visual_scroll)
        .unwrap_or_else(|i| i.saturating_sub(1));
    let logical_end = match vs_cache.binary_search(&end_visual) {
        Ok(i) => i,
        Err(i) => i.min(total_logical),
    };

    // Phase 3: build TextCells for visible logical rows, then render.
    let log_fg = ctx.theme.fg;

    let mut renderer = LogColumnRenderer::new().with_viewport(visual_scroll, visible_height);

    // Track message categories for separator insertion
    let mut prev_category: Option<&'static str> = None;

    let mut logical_i = logical_start;
    while logical_i < logical_end {
        let cache_start = vs_cache[logical_i];
        let cache_end = vs_cache[logical_i + 1];
        // Skip logical rows that fall entirely outside the visual viewport.
        if cache_end <= visual_scroll || cache_start >= end_visual {
            logical_i += 1;
            continue;
        }

        let phys_idx = ctx.log_scroll.visible_indices.get(logical_i).copied();

        // Compute the byte-range selection for this logical row, if any.
        let selection_range = ctx.mouse.log_selection.and_then(|sel| {
            let phys = phys_idx?;
            sel.byte_range_for(phys, ctx.log.items[phys].raw.len())
        });

        // ── Message category separator ──────────────────────────────
        // Between message groups of different types (user ↔ system ↔ assistant),
        // insert a thin decorative separator line.
        if let Some(phys) = phys_idx {
            let kind = ctx.log.items[phys].kind;
            let category = match kind {
                LogItemKind::User => "user",
                LogItemKind::AssistantMarkdown => "assistant",
                LogItemKind::SystemPlain(_)
                | LogItemKind::SystemMarkdown
                | LogItemKind::SystemTool
                | LogItemKind::Thinking => "system",
            };

            // Insert separator if category changed (and not first line)
            if let Some(prev) = prev_category
                && prev != category
            {
                let separator_fg = match category {
                    "user" => ctx.theme.accent,
                    "system" => ctx.theme.warning,
                    _ => ctx.theme.border,
                };
                let separator_label = match category {
                    "user" => "💬 user",
                    "system" => "⚙️ system",
                    _ => "🤖 assistant",
                };
                let separator = MessageSeparator::new(separator_label.to_string(), separator_fg);
                renderer.push(vs_cache[logical_i], separator);
            }
            prev_category = Some(category);
        }

        // Tool block: replace the summary TextCell + placeholder rows with a
        // single ToolCell that renders both summary and detail card.
        if let Some(phys) = phys_idx {
            if let Some((thinking_phys, _thinking_logical, thinking_rows)) =
                find_thinking_at_logical(ctx.log_scroll, ctx.thinking, logical_i)
            {
                let rows_before = phys.saturating_sub(thinking_phys);
                let vis_start = if rows_before > 0 && rows_before <= logical_i {
                    vs_cache[logical_i - rows_before]
                } else {
                    vs_cache[logical_i]
                };
                let msgs = &ctx.messages;
                let spinner =
                    TOOL_RUNNING_SPINNER[(ctx.spinner_frame as usize) % TOOL_RUNNING_SPINNER.len()];
                if let Some(active) = ctx
                    .thinking
                    .active
                    .as_ref()
                    .filter(|active| active.phys_idx == thinking_phys)
                {
                    renderer.push(
                        vis_start,
                        ThinkingCell::active(active, spinner, ctx.theme, msgs),
                    );
                } else if let Some(block) = ctx
                    .thinking
                    .blocks
                    .iter()
                    .find(|block| block.phys_idx == thinking_phys)
                {
                    renderer.push(vis_start, ThinkingCell::completed(block, ctx.theme, msgs));
                }
                logical_i += thinking_rows - rows_before;
                continue;
            }

            let tool_match = ctx
                .tools
                .active
                .iter()
                .find(|active| {
                    phys >= active.phys_idx
                        && phys <= active.phys_idx + active.output.message_placeholder_rows()
                })
                .map(|active| {
                    (
                        active.phys_idx,
                        active.output.clone(),
                        Some(active.started_at),
                    )
                })
                .or_else(|| {
                    ctx.tools.blocks.iter().find_map(|b| {
                        if phys >= b.phys_idx
                            && phys <= b.phys_idx + b.output.message_placeholder_rows()
                        {
                            Some((b.phys_idx, b.output.clone(), None))
                        } else {
                            None
                        }
                    })
                });
            if let Some((phys_idx, output, started_at)) = tool_match {
                let rows_before = phys.saturating_sub(phys_idx);
                let visual_rows = output.visual_rows(false);
                let vis_start = if rows_before > 0 && rows_before <= logical_i {
                    vs_cache[logical_i - rows_before]
                } else {
                    vs_cache[logical_i]
                };
                let msgs = &ctx.messages;
                let spinner =
                    TOOL_RUNNING_SPINNER[(ctx.spinner_frame as usize) % TOOL_RUNNING_SPINNER.len()];
                let card_cell = ToolCell::from_output(
                    output,
                    started_at,
                    spinner,
                    false,
                    ctx.theme.accent,
                    ctx.theme.bg,
                    ctx.theme.fg,
                    ctx.theme.success,
                    ctx.theme.warning,
                    ctx.theme.error,
                    ctx.theme.block_border_type(),
                    msgs,
                );
                renderer.push(vis_start, card_cell);
                logical_i += visual_rows - rows_before;
                continue;
            }
        }

        // Whole-Markdown message: render the cached MarkdownCell at the
        // logical row's visual start. `vs_cache` already reserved its rows.
        if let Some(phys) = phys_idx
            && let Some(cell) = ctx
                .log
                .items
                .get(phys)
                .and_then(|item| item.markdown_cell.as_ref())
        {
            renderer.push(vs_cache[logical_i], cell);
            logical_i += 1;
            continue;
        }

        // Task-end rule: full-width line with centered elapsed label.
        if let Some(phys) = phys_idx
            && is_task_end_separator(&ctx.log.items[phys].raw)
        {
            let raw = &ctx.log.items[phys].raw;
            let msgs = &ctx.messages;
            let sep = match task_end_elapsed_secs(raw) {
                Some(secs) => {
                    TaskEndSeparator::with_elapsed(ctx.theme.accent, msgs.bottom_elapsed, secs)
                }
                None => TaskEndSeparator::new(ctx.theme.accent),
            };
            renderer.push(vs_cache[logical_i], sep);
            logical_i += 1;
            continue;
        }

        // Normal row: build TextCell
        let cached_lines: Vec<Line<'static>> =
            ctx.log_scroll.visual_cache[cache_start..cache_end].to_vec();
        let raw_text = phys_idx
            .map(|p| ctx.log.items[p].raw.clone())
            .unwrap_or_default();

        let indent_cols = phys_idx
            .map(|p| log_indent_at(ctx.log, p))
            // The last row is an in-progress assistant response, not a stored
            // physical message, so apply the same reply indent directly.
            .unwrap_or(LOG_THINKING_INDENT + 1);

        let cell = TextCell::new(
            cached_lines,
            raw_text,
            selection_range,
            None,
            indent_cols,
            log_fg,
            ctx.theme.bg,
        );

        // Push at this row's visual-line offset; LogColumnRenderer does a second
        // viewport clip and calls TextCell::render_partial for sub-line trimming.
        renderer.push(vs_cache[logical_i], cell);
        logical_i += 1;
    }

    let panel_title = ctx.messages.log_title.to_string();

    // Render bordered log panel (bottom border may be omitted for sticky join).
    let log_block = Block::default()
        .borders(borders)
        .border_type(ctx.theme.block_border_type())
        .border_style(Style::default().fg(ctx.theme.border))
        .title(panel_title)
        .style(Style::default().bg(ctx.theme.bg));
    let inner = Rect::new(
        area.x + left,
        area.y + top,
        area.width.saturating_sub(left + right),
        area.height.saturating_sub(top + bottom),
    );
    frame.render_widget(log_block, area);
    frame.render_widget(
        Paragraph::new("").style(Style::default().bg(ctx.theme.bg)),
        inner,
    );
    frame.render_widget(renderer, inner);

    // Code cards remain viewport-clipped overlays. Thinking cards are direct
    // cells in the Phase 3 renderer above.
    render_code_cards(frame, area, ctx, visual_scroll, visible_height);

    // Loading spinner overlay on the loading placeholder row (if present).
    render_loading_spinner(frame, area, ctx, visual_scroll, visible_height);

    // Scrollbar thumb follows visual lines, not logical offset:
    let scrollbar = Scrollbar::default()
        .orientation(ratatui::widgets::ScrollbarOrientation::VerticalRight)
        .begin_symbol(Some("▲"))
        .end_symbol(Some("▼"))
        .track_symbol(Some("│"))
        // Half-block thumb: if a terminal briefly desyncs around wide emoji titles,
        // a full `█` ghost on the left chrome reads as a hard "shadow"; `▐` is quieter.
        .thumb_symbol("▐")
        .begin_style(Style::default().fg(ctx.theme.border))
        .end_style(Style::default().fg(ctx.theme.border))
        .track_style(Style::default().fg(ctx.theme.border))
        .thumb_style(Style::default().fg(ctx.theme.accent));
    let sb_position = if total_visual > visible_height {
        let range = total_visual - visible_height;
        (visual_scroll as u64 * (total_visual - 1) as u64 / range as u64) as usize
    } else {
        0
    };
    let sb_position = sb_position.min(total_visual.saturating_sub(1));
    let mut state = ScrollbarState::new(total_visual)
        .viewport_content_length(ctx.log_scroll.height as usize)
        .position(sb_position);
    frame.render_stateful_widget(scrollbar, area, &mut state);

    // Wide graphemes in card titles (e.g. 🧠) can desync some terminals' cursors while the
    // accent scrollbar thumb (`█`) is also being painted. Ghost thumb cells then stick on the
    // left chrome because unchanged border cells are skipped by Buffer::diff. Force-emit the
    // left border every frame so those residues cannot persist.
    restamp_log_left_border(frame.buffer_mut(), area, borders, ctx.theme);
}

/// Re-assert the log panel's left vertical border and mark it `AlwaysUpdate`.
///
/// Corners stay owned by the outer `Block`; this only refreshes the vertical span.
fn restamp_log_left_border(buf: &mut Buffer, area: Rect, borders: Borders, theme: &Theme) {
    if !borders.contains(Borders::LEFT) || area.width == 0 || area.height == 0 {
        return;
    }
    let top = u16::from(borders.contains(Borders::TOP));
    let bottom = u16::from(borders.contains(Borders::BOTTOM));
    let y0 = area.y.saturating_add(top);
    let y1 = area
        .y
        .saturating_add(area.height)
        .saturating_sub(bottom)
        .max(y0);
    let style = Style::default().fg(theme.border).bg(theme.bg);
    for y in y0..y1 {
        let cell = &mut buf[(area.x, y)];
        cell.set_symbol("│");
        cell.set_style(style);
        cell.set_diff_option(CellDiffOption::AlwaysUpdate);
    }
}

/// Render an animated loading spinner at the loading placeholder position.
/// Uses `ctx.spinner_frame` (cycled 0-9) to pick a Braille spinner character,
/// and displays a "Thinking..." label with a subtle pulse.
fn render_loading_spinner(
    frame: &mut Frame,
    area: Rect,
    ctx: &RenderCtx,
    visual_scroll: usize,
    visible_height: usize,
) {
    let Some(idx) = ctx.loading_idx else { return };
    // Find logical row for this physical index
    let Some(logical_row) = ctx
        .log_scroll
        .phys_to_logical_cache
        .get(idx)
        .and_then(|&v| v)
    else {
        return;
    };
    let vs_cache = &ctx.log_scroll.visual_start_cache;
    if logical_row >= vs_cache.len().saturating_sub(1) {
        return;
    }
    let vis_top = vs_cache[logical_row];
    let vis_bot = vs_cache[logical_row + 1];
    let range_end = visual_scroll + visible_height;
    if vis_bot <= visual_scroll || vis_top >= range_end {
        return;
    }
    let y = (vis_top.saturating_sub(visual_scroll)) as u16;

    // Spinner characters (10-frame cycle)
    const SPINNERS: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
    let spinner_char = SPINNERS[(ctx.spinner_frame as usize) % SPINNERS.len()];

    let spinner_style = Style::default()
        .fg(ctx.theme.warning)
        .add_modifier(Modifier::BOLD);
    let text_style = Style::default()
        .fg(ctx.theme.accent)
        .add_modifier(Modifier::ITALIC);

    let spinner_line = Line::from(vec![
        Span::styled(format!(" {} ", spinner_char), spinner_style),
        Span::styled("Thinking...", text_style),
    ]);

    let spinner_area = Rect::new(area.x + 2, area.y + 1 + y, area.width.saturating_sub(4), 1);
    if spinner_area.bottom() <= area.bottom() {
        frame.render_widget(Clear, spinner_area);
        frame.render_widget(Paragraph::new(spinner_line), spinner_area);
    }
}
