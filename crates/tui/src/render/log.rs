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
        cells::{thinking::ThinkingCell, tool::ToolCell},
        renderable::Renderable,
        util::wrap_line,
    },
    theme::Theme,
    widgets::state::{App, LogItemKind},
};

use agent_tui_kit::{
    render::{ctx::RenderCtx, log_column::LogColumnRenderer},
    state::{LogCoordinator, LogScroll, SkillEntry, log_indent_at},
};

/// Render the Log panel: wrapping, scrolling, and mouse selection.
///
/// # Pipeline overview
///
/// ```text
///  Phase 0          Phase 1              Phase 2           Phase 3
///  physical ──→     logical ──→          visual viewport   TextCell + render
///  messages         wrap_line            scroll clip       + overlays
///       │                │                     │                  │
///       ▼                ▼                     ▼                  ▼
///  visible_indices   visual_cache         visual_scroll      LogColumnRenderer
/// ```
///
/// Phase 0-2 (cache rebuild) is the mutable `prepare_log_frame`; Phase 3 is the
/// pure `render_log_panel_pure` (reads the built caches).
pub(crate) fn render_log_panel(frame: &mut Frame, area: Rect, app: &mut App) {
    render_log_panel_with_borders(frame, area, app, Borders::ALL);
}

/// Like [`render_log_panel`], but allows omitting the bottom border so a sticky
/// task strip can continue the same chrome under the Log.
pub(crate) fn render_log_panel_with_borders(
    frame: &mut Frame,
    area: Rect,
    app: &mut App,
    borders: Borders,
) {
    let messages = app.msgs();
    prepare_log_frame(
        &mut app.log_scroll,
        &app.log,
        &app.stream.buffer,
        &app.skills_data,
        &app.theme,
        messages,
        area,
        borders,
    );
    let ctx = RenderCtx {
        theme: &app.theme,
        messages: app.msgs(),
        log_scroll: &app.log_scroll,
        log: &app.log,
        code_blocks: &app.code_blocks,
        mermaid_blocks: &app.mermaid_blocks,
        tools: &app.tools,
        thinking: &app.thinking,
        stream: &app.stream,
        mouse: &app.mouse,
        skills_data: &app.skills_data,
        loading_idx: app.loading_idx,
        spinner_frame: app.spinner_frame,
    };
    render_log_panel_pure(frame, area, &ctx, borders);
}

/// Phase 0-2: rebuild the scroll/layout caches (mutable). Runs once per frame
/// before the pure render pass.
///
/// Arguments mirror the disjoint `App` fields it reads; they will be grouped
/// into a prepare-input struct once the remaining panels migrate.
#[allow(clippy::too_many_arguments)]
fn prepare_log_frame(
    log_scroll: &mut LogScroll,
    log: &LogCoordinator,
    stream_buffer: &str,
    skills_data: &[SkillEntry],
    theme: &Theme,
    messages: agent_tui_kit::i18n::Messages,
    area: Rect,
    borders: Borders,
) {
    let top = u16::from(borders.contains(Borders::TOP));
    let bottom = u16::from(borders.contains(Borders::BOTTOM));
    let left = u16::from(borders.contains(Borders::LEFT));
    let right = u16::from(borders.contains(Borders::RIGHT));

    // 这行是算**面板内容区的实际可用高度**。
    // area.height = Border Block 的整个矩形高度
    // ┌─ Log ──────────────┐  ← area.y + 0  (上边框，占 1 行)
    // │                     │  ← area.y + 1  (内容区第 1 行)
    // │   actual content    │  ← ...
    // │                     │  ← area.y + area.height - 2 (内容区最后一行)
    // └─────────────────────┘  ← area.y + area.height - 1 (下边框，占 1 行)
    // area.height.saturating_sub(top+bottom) = 内容区可用行数 = visible_height
    log_scroll.height = area.height.saturating_sub(top + bottom);
    let visible_height = log_scroll.height as usize;
    // 两行做两件事：
    // 和 `height` 同样的 `saturating_sub`：
    // area.width = 整个 Block 的列宽
    // ┌─ Log ──────────────────┐
    // │                        │  ← area.width - left - right = 内容区可用列宽
    // └────────────────────────┘
    //     ↑ 左边框         ↑ 右边框
    let max_width = area.width.saturating_sub(left + right) as usize;
    // 防止 `wrap_line` 拿到 0 宽度：
    let wrap_width = if max_width > 0 { max_width } else { 1 };
    // Remember the content width: streamed tables are laid out against this
    // width at build time so they never need post-hoc char wrapping.
    log_scroll.width = wrap_width as u16;

    // `visible_indices_ver` is the **dirty marker**. It stores the previous `log_items.len()`; a change invalidates the cache:
    let indices_stale = log_scroll.visible_indices_ver != log.items.len();
    if indices_stale {
        log_scroll.visible_indices.clear();
        log_scroll.phys_to_logical_cache.clear();
        log_scroll
            .phys_to_logical_cache
            .resize(log.items.len(), None);
        let mut total_logical = 0;
        // 遍历所有消息，将可见的物理索引添加到 visible_indices 中，并更新缓存
        for phys in 0..log.items.len() {
            if phys < log.items.len() {
                log_scroll.visible_indices.push(phys);
                log_scroll.phys_to_logical_cache[phys] = Some(total_logical);
                total_logical += 1;
            }
        }
        // update visible_indices_ver to mark cache valid
        log_scroll.visible_indices_ver = log.items.len();
    }
    // total_logical: 可见的逻辑行数量
    let mut total_logical = log_scroll.visible_indices.len();
    // Stream buffer occupies the last logical row while tokens are arriving.
    if !stream_buffer.is_empty() {
        total_logical += 1;
    }

    // Every stored row carries its source kind, so the render path never has
    // to infer user/system ownership from raw prefixes or indentation.

    // Phase 1: logical → visual wrap cache.
    //
    // ```text
    //  logical 0: "hello world this is very long"
    //       │ wrap_line(width)
    //       ▼
    //  visual  [0]"hello world " [1]"this is " [2]"very long"
    //
    //  visual_start_cache = [0, 3, 5, ...]   ← prefix sum: logical i starts at visual[j]
    //  visual_cache       = [line0, line1, line2, ...]
    // ```
    //
    // Rebuild when message count, panel width, or theme changes.
    let cache_valid = log_scroll.visual_cache_ver == log.items.len()
        && log_scroll.visual_cache_width == wrap_width as u16
        && log_scroll.visual_cache_theme == theme.name;

    if !cache_valid {
        log_scroll.visual_cache.clear();
        log_scroll.visual_start_cache.clear();
        log_scroll.visual_start_cache.push(0);

        // Build once for the whole rebuild — not once per line.
        let skill_names = super::slash_style::skill_name_set(skills_data);
        let user_prefix_tmpl = messages.user_msg_prefix;
        let user_cont_tmpl = messages.user_msg_cont;

        for logical_i in 0..total_logical {
            // Whole-Markdown messages reserve one logical row; its visual
            // height comes from the cached MarkdownCell (parsed once per
            // width), and the rows are blank placeholders so the prefix-sum
            // cache stays consistent (same pattern as tool placeholder rows).
            if let Some(&phys_idx) = log_scroll.visible_indices.get(logical_i)
                && let Some(cell) = log
                    .items
                    .get(phys_idx)
                    .and_then(|item| item.markdown_cell.as_ref())
            {
                let rows = cell.height(wrap_width as u16) as usize;
                log_scroll
                    .visual_cache
                    .extend(std::iter::repeat_n(Line::default(), rows));
                log_scroll
                    .visual_start_cache
                    .push(log_scroll.visual_cache.len());
                continue;
            }

            let line = if let Some(&phys_idx) = log_scroll.visible_indices.get(logical_i) {
                let item = &log.items[phys_idx];
                if super::cells::separator::is_task_end_separator(&item.raw)
                    || item.line.spans.is_empty()
                {
                    Line::default()
                } else {
                    super::log_style::restyle_log_line_with_skills(
                        &item.line,
                        &item.raw,
                        theme,
                        item.kind,
                        &skill_names,
                        user_prefix_tmpl,
                        user_cont_tmpl,
                    )
                }
            } else {
                // Last logical row: live stream text. Uses the same fg as the
                // final flushed rows so completing a reply does not recolor it.
                Line::from(Span::styled(stream_buffer, theme.fg))
            };
            let wrapped = if let Some(&phys_idx) = log_scroll.visible_indices.get(logical_i) {
                if super::cells::separator::is_task_end_separator(&log.items[phys_idx].raw) {
                    vec![Line::default()]
                } else {
                    let indent = log_indent_at(log, phys_idx) as usize;
                    wrap_line(&line, wrap_width.saturating_sub(indent).max(1))
                }
            } else {
                // The stream row uses the same reply indent as its TextCell.
                let indent = (super::util::LOG_THINKING_INDENT + 1) as usize;
                wrap_line(&line, wrap_width.saturating_sub(indent).max(1))
            };
            log_scroll.visual_cache.extend(wrapped);
            log_scroll
                .visual_start_cache
                .push(log_scroll.visual_cache.len());
        }
        log_scroll.visual_cache_width = wrap_width as u16;
        log_scroll.visual_cache_ver = log.items.len();
        log_scroll.visual_cache_theme = theme.name;
    }

    // Phase 2: map logical scroll offset to a visual viewport.
    //
    // ```text
    //  total_visual = 1200 lines, visible_height = 20, offset = 15 (logical)
    //
    //  visual lines:  ... [178][179][180]...[199][200] ...
    //                              └──── viewport ────┘
    //  visual_scroll = visual_start_cache[15] = 180
    //  end_visual    = visual_scroll + visible_height = 200
    // ```
    let total_visual = *log_scroll.visual_start_cache.last().unwrap_or(&0);
    let max_visual_scroll = total_visual.saturating_sub(visible_height);
    // The visual position is authoritative: clamp the bottom sentinel
    // (`usize::MAX`) and out-of-range values to the true visual bottom.
    let visual_scroll = if log_scroll.visual_top == usize::MAX {
        max_visual_scroll
    } else {
        log_scroll.visual_top.min(max_visual_scroll)
    };
    log_scroll.visual_top = visual_scroll;
    let vs_cache = &log_scroll.visual_start_cache;
    // Derive the logical offset mirror (row containing the viewport top) for
    // read-only consumers such as mouse hit-testing and the code-card popup.
    let logical_scroll = vs_cache
        .partition_point(|&start| start <= visual_scroll)
        .saturating_sub(1)
        .min(total_logical.saturating_sub(1));
    log_scroll.offset = logical_scroll.min(u16::MAX as usize) as u16;

    // Persist prefix-sum cache for mouse hit-testing and scroll handlers outside render.
    log_scroll.visual_start = log_scroll.visual_start_cache.clone();
}

/// Phase 3: pure render — build cells from the caches and draw. Reads only.
fn render_log_panel_pure(frame: &mut Frame, area: Rect, ctx: &RenderCtx, borders: Borders) {
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
                let separator = super::cells::separator::MessageSeparator::new(
                    separator_label.to_string(),
                    separator_fg,
                );
                renderer.push(vs_cache[logical_i], separator);
            }
            prev_category = Some(category);
        }

        // Tool block: replace the summary TextCell + placeholder rows with a
        // single ToolCell that renders both summary and detail card.
        if let Some(phys) = phys_idx {
            if let Some((thinking_phys, _thinking_logical, thinking_rows)) =
                agent_tui_kit::state::find_thinking_at_logical(
                    ctx.log_scroll,
                    ctx.thinking,
                    logical_i,
                )
            {
                let rows_before = phys.saturating_sub(thinking_phys);
                let vis_start = if rows_before > 0 && rows_before <= logical_i {
                    vs_cache[logical_i - rows_before]
                } else {
                    vs_cache[logical_i]
                };
                let msgs = &ctx.messages;
                let spinner = crate::widgets::tool_widget::TOOL_RUNNING_SPINNER[(ctx.spinner_frame
                    as usize)
                    % crate::widgets::tool_widget::TOOL_RUNNING_SPINNER.len()];
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
                let spinner = crate::widgets::tool_widget::TOOL_RUNNING_SPINNER[(ctx.spinner_frame
                    as usize)
                    % crate::widgets::tool_widget::TOOL_RUNNING_SPINNER.len()];
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
            && super::cells::separator::is_task_end_separator(&ctx.log.items[phys].raw)
        {
            let raw = &ctx.log.items[phys].raw;
            let msgs = &ctx.messages;
            let sep = match super::cells::separator::task_end_elapsed_secs(raw) {
                Some(secs) => super::cells::separator::TaskEndSeparator::with_elapsed(
                    ctx.theme.accent,
                    msgs.bottom_elapsed,
                    secs,
                ),
                None => super::cells::separator::TaskEndSeparator::new(ctx.theme.accent),
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
            .unwrap_or(super::util::LOG_THINKING_INDENT + 1);

        let cell = super::cells::text::TextCell::new(
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
    super::cells::code::render_code_cards(frame, area, ctx, visual_scroll, visible_height);

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
