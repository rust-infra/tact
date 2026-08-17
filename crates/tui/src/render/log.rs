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
///  phys_to_logical   visual_start_cache   logical_start/end  thinking/diff/code
/// ```
///
/// # Three coordinate spaces
///
/// ```text
///  PHYSICAL (log_items[])     LOGICAL (scroll here)        VISUAL (draw here)
///  ┌───┬───┬───┬───┐         ┌───┬───┬───┐                ┌───┬───┬───┬───┬───┐
///  │ 0 │ 1 │ 2 │ 3 │  hide  │ 0 │ 1 │ 2 │  wrap long     │ 0 │ 1 │ 2 │ 3 │ 4 │
///  └───┴───┴───┴───┘  ──→    └───┴───┴───┘  ──→           └───┴───┴───┴───┴───┘
///   every stored msg          visible only              one screen line each
///                             + stream buffer           (may be many per logical)
/// ```
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
    // ① Phase 2 视口裁剪 —— 决定屏幕上能显示多少行
    // let visible_height = app.log_scroll.height as usize;
    // let end_visual = (visual_scroll + visible_height).min(total_visual);
    // // ② 覆盖层裁剪 —— thinking/diff/code cards 也用它
    // saturating_sub` 防的是极端情况：如果 `area.height < 2`（面板被缩到极小），不会 panic，直接归零。
    app.log_scroll.height = area.height.saturating_sub(top + bottom);
    let visible_height = app.log_scroll.height as usize;
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
    app.log_scroll.width = wrap_width as u16;

    // `visible_indices_ver` is the **dirty marker**. It stores the previous `log_items.len()`; a change invalidates the cache:
    // ```
    // 当前消息数量 ≠ 上次缓存时的消息数量  →  缓存过期，需要重建
    // ```
    // 这是 Phase 0 唯一的触发条件——因为只有消息增删才会改变可见索引（消息新增可能落在 thinking block 内部，需要重新判断是否可见）。消息内容变化不改变可见性，所以不用重建。
    let indices_stale = app.log_scroll.visible_indices_ver != app.log_items.len();
    if indices_stale {
        app.log_scroll.visible_indices.clear();
        app.log_scroll.phys_to_logical_cache.clear();
        app.log_scroll
            .phys_to_logical_cache
            .resize(app.log_items.len(), None);
        let mut total_logical = 0;
        // 遍历所有消息，将可见的物理索引添加到 visible_indices 中，并更新缓存
        for phys in 0..app.log_items.len() {
            if app.is_message_visible(phys) {
                app.log_scroll.visible_indices.push(phys);
                app.log_scroll.phys_to_logical_cache[phys] = Some(total_logical);
                total_logical += 1;
            }
        }
        // update visible_indices_ver to mark cache valid
        app.log_scroll.visible_indices_ver = app.log_items.len();
    }
    // total_logical: 可见的逻辑行数量
    let mut total_logical = app.log_scroll.visible_indices.len();
    // Stream buffer occupies the last logical row while tokens are arriving.
    if !app.stream.buffer.is_empty() {
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
    let cache_valid = app.log_scroll.visual_cache_ver == app.log_items.len()
        && app.log_scroll.visual_cache_width == wrap_width as u16
        && app.log_scroll.visual_cache_theme == app.theme.name;

    if !cache_valid {
        app.log_scroll.visual_cache.clear();
        app.log_scroll.visual_start_cache.clear();
        app.log_scroll.visual_start_cache.push(0);

        // Build once for the whole rebuild — not once per line.
        let skill_names = super::slash_style::skill_name_set(&app.skills_data);
        let msgs = app.msgs();
        let user_prefix_tmpl = msgs.user_msg_prefix;
        let user_cont_tmpl = msgs.user_msg_cont;

        for logical_i in 0..total_logical {
            // Whole-Markdown messages reserve one logical row; its visual
            // height comes from the cached MarkdownCell (parsed once per
            // width), and the rows are blank placeholders so the prefix-sum
            // cache stays consistent (same pattern as tool placeholder rows).
            if let Some(&phys_idx) = app.log_scroll.visible_indices.get(logical_i)
                && let Some(cell) = app
                    .log_items
                    .get(phys_idx)
                    .and_then(|item| item.markdown_cell.as_ref())
            {
                let rows = cell.height(wrap_width as u16) as usize;
                app.log_scroll
                    .visual_cache
                    .extend(std::iter::repeat_n(Line::default(), rows));
                app.log_scroll
                    .visual_start_cache
                    .push(app.log_scroll.visual_cache.len());
                continue;
            }

            let line = if let Some(&phys_idx) = app.log_scroll.visible_indices.get(logical_i) {
                let item = &app.log_items[phys_idx];
                if super::cells::separator::is_task_end_separator(&item.raw)
                    || item.line.spans.is_empty()
                {
                    Line::default()
                } else {
                    super::log_style::restyle_log_line_with_skills(
                        &item.line,
                        &item.raw,
                        &app.theme,
                        item.kind,
                        &skill_names,
                        user_prefix_tmpl,
                        user_cont_tmpl,
                    )
                }
            } else {
                // Last logical row: live stream text. Uses the same fg as the
                // final flushed rows so completing a reply does not recolor it.
                Line::from(Span::styled(app.stream.buffer.as_str(), app.theme.fg))
            };
            let wrapped = if let Some(&phys_idx) = app.log_scroll.visible_indices.get(logical_i) {
                if super::cells::separator::is_task_end_separator(&app.log_items[phys_idx].raw) {
                    vec![Line::default()]
                } else {
                    let indent = app.nested_log_indent(phys_idx) as usize;
                    wrap_line(&line, wrap_width.saturating_sub(indent).max(1))
                }
            } else {
                // The stream row uses the same reply indent as its TextCell.
                let indent = (super::util::LOG_THINKING_INDENT + 1) as usize;
                wrap_line(&line, wrap_width.saturating_sub(indent).max(1))
            };
            app.log_scroll.visual_cache.extend(wrapped);
            app.log_scroll
                .visual_start_cache
                .push(app.log_scroll.visual_cache.len());
        }
        app.log_scroll.visual_cache_width = wrap_width as u16;
        app.log_scroll.visual_cache_ver = app.log_items.len();
        app.log_scroll.visual_cache_theme = app.theme.name;
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
    //
    //  reverse lookup (binary_search on prefix sums):
    //    logical_start = row containing visual 180  →  15
    //    logical_end   = row containing visual 200  →  18
    // ```
    let total_visual = *app.log_scroll.visual_start_cache.last().unwrap_or(&0);
    let max_visual_scroll = total_visual.saturating_sub(visible_height);
    // The visual position is authoritative: clamp the bottom sentinel
    // (`usize::MAX`) and out-of-range values to the true visual bottom.
    let visual_scroll = if app.log_scroll.visual_top == usize::MAX {
        max_visual_scroll
    } else {
        app.log_scroll.visual_top.min(max_visual_scroll)
    };
    app.log_scroll.visual_top = visual_scroll;
    let vs_cache = &app.log_scroll.visual_start_cache;
    // Derive the logical offset mirror (row containing the viewport top) for
    // read-only consumers such as mouse hit-testing and the code-card popup.
    let logical_scroll = vs_cache
        .partition_point(|&start| start <= visual_scroll)
        .saturating_sub(1)
        .min(total_logical.saturating_sub(1));
    app.log_scroll.offset = logical_scroll.min(u16::MAX as usize) as u16;
    let end_visual = (visual_scroll + visible_height).min(total_visual);

    // Reverse-map visual viewport bounds back to logical row range for cell building.
    let logical_start = vs_cache
        .binary_search(&visual_scroll)
        .unwrap_or_else(|i| i.saturating_sub(1));
    let logical_end = match vs_cache.binary_search(&end_visual) {
        Ok(i) => i,
        Err(i) => i.min(total_logical),
    };

    // Phase 3: build TextCells for visible logical rows, then render.
    //
    // ```text
    //  wrap cache (plain text)          TextCell (on demand)
    //  ┌─────────────────────┐          ┌─────────────────────┐
    //  │ cached_lines        │  select │ REVERSED style      │
    //  │ no selection        │  ──→    │ overlay             │
    //  └─────────────────────┘          └─────────────────────┘
    //
    //  Viewport clipping happens twice:
    //    1. here — skip logical rows outside [logical_start, logical_end)
    //    2. LogColumnRenderer — skip_lines inside partially visible cells
    // ```

    let log_fg = app.theme.fg;

    let mut renderer =
        super::log_column::LogColumnRenderer::new().with_viewport(visual_scroll, visible_height);

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

        let phys_idx = app.log_scroll.visible_indices.get(logical_i).copied();

        // Compute the byte-range selection for this logical row, if any.
        let selection_range = app.mouse.log_selection.and_then(|sel| {
            let phys = phys_idx?;
            sel.byte_range_for(phys, app.log_items[phys].raw.len())
        });

        // ── Message category separator ──────────────────────────────
        // Between message groups of different types (user ↔ system ↔ assistant),
        // insert a thin decorative separator line.
        if let Some(phys) = phys_idx {
            let kind = app.log_items[phys].kind;
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
                    "user" => app.theme.accent,
                    "system" => app.theme.warning,
                    _ => app.theme.border,
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
        //
        // Check both exact and range match: when the viewport starts in the
        // middle of placeholder rows (user scrolled up), the summary phys_idx
        // is no longer in the loop range, but subsequent placeholder rows
        // still belong to the same ToolBlock.
        if let Some(phys) = phys_idx {
            if let Some((thinking_phys, _thinking_logical, thinking_rows)) =
                app.find_thinking_at_logical(logical_i)
            {
                let rows_before = phys.saturating_sub(thinking_phys);
                let vis_start = if rows_before > 0 && rows_before <= logical_i {
                    vs_cache[logical_i - rows_before]
                } else {
                    vs_cache[logical_i]
                };
                let msgs = app.msgs();
                let spinner = crate::widgets::tool_widget::TOOL_RUNNING_SPINNER[(app.spinner_frame
                    as usize)
                    % crate::widgets::tool_widget::TOOL_RUNNING_SPINNER.len()];
                if let Some(active) = app
                    .thinking
                    .active
                    .as_ref()
                    .filter(|active| active.phys_idx == thinking_phys)
                {
                    renderer.push(
                        vis_start,
                        ThinkingCell::active(active, spinner, &app.theme, &msgs),
                    );
                } else if let Some(block) = app
                    .thinking
                    .blocks
                    .iter()
                    .find(|block| block.phys_idx == thinking_phys)
                {
                    renderer.push(vis_start, ThinkingCell::completed(block, &app.theme, &msgs));
                }
                logical_i += thinking_rows - rows_before;
                continue;
            }

            let tool_match = app
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
                    app.tools.blocks.iter().find_map(|b| {
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
                let msgs = app.msgs();
                let spinner = crate::widgets::tool_widget::TOOL_RUNNING_SPINNER[(app.spinner_frame
                    as usize)
                    % crate::widgets::tool_widget::TOOL_RUNNING_SPINNER.len()];
                let card_cell = ToolCell::from_output(
                    output,
                    started_at,
                    spinner,
                    false,
                    app.theme.accent,
                    app.theme.bg,
                    app.theme.fg,
                    app.theme.success,
                    app.theme.warning,
                    app.theme.error,
                    app.theme.block_border_type(),
                    &msgs,
                );
                renderer.push(vis_start, card_cell);
                logical_i += visual_rows - rows_before;
                continue;
            }
        }

        // Whole-Markdown message: render the cached MarkdownCell at the
        // logical row's visual start. `vs_cache` already reserved its rows.
        if let Some(phys) = phys_idx
            && let Some(cell) = app
                .log_items
                .get(phys)
                .and_then(|item| item.markdown_cell.as_ref())
        {
            renderer.push(vs_cache[logical_i], cell);
            logical_i += 1;
            continue;
        }

        // Task-end rule: full-width line with centered elapsed label.
        if let Some(phys) = phys_idx
            && super::cells::separator::is_task_end_separator(&app.log_items[phys].raw)
        {
            let raw = &app.log_items[phys].raw;
            let msgs = app.msgs();
            let sep = match super::cells::separator::task_end_elapsed_secs(raw) {
                Some(secs) => super::cells::separator::TaskEndSeparator::with_elapsed(
                    app.theme.accent,
                    msgs.bottom_elapsed,
                    secs,
                ),
                None => super::cells::separator::TaskEndSeparator::new(app.theme.accent),
            };
            renderer.push(vs_cache[logical_i], sep);
            logical_i += 1;
            continue;
        }

        // Normal row: build TextCell
        let cached_lines: Vec<Line<'static>> =
            app.log_scroll.visual_cache[cache_start..cache_end].to_vec();
        let raw_text = phys_idx
            .map(|p| app.log_items[p].raw.clone())
            .unwrap_or_default();

        let indent_cols = phys_idx
            .map(|p| app.nested_log_indent(p))
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
            app.theme.bg,
        );

        // Push at this row's visual-line offset; LogColumnRenderer does a second
        // viewport clip and calls TextCell::render_partial for sub-line trimming.
        renderer.push(vs_cache[logical_i], cell);
        logical_i += 1;
    }

    let panel_title = app.msgs().log_title.to_string();

    // Render bordered log panel (bottom border may be omitted for sticky join).
    let log_block = Block::default()
        .borders(borders)
        .border_type(app.theme.block_border_type())
        .border_style(Style::default().fg(app.theme.border))
        .title(panel_title)
        .style(Style::default().bg(app.theme.bg));
    let inner = Rect::new(
        area.x + left,
        area.y + top,
        area.width.saturating_sub(left + right),
        area.height.saturating_sub(top + bottom),
    );
    frame.render_widget(log_block, area);
    frame.render_widget(
        Paragraph::new("").style(Style::default().bg(app.theme.bg)),
        inner,
    );
    frame.render_widget(renderer, inner);

    // Code cards remain viewport-clipped overlays. Thinking cards are direct
    // cells in the Phase 3 renderer above.
    super::cells::code::render_code_cards(frame, area, app, visual_scroll, visible_height);

    // Loading spinner overlay on the loading placeholder row (if present).
    render_loading_spinner(frame, area, app, visual_scroll, visible_height);

    // Scrollbar thumb follows visual lines, not logical offset:
    //
    // ```text
    //  logical offset 15  ──may map to──▶  visual line 180
    //  because one message can wrap to many visual lines after resize
    // ```
    let scrollbar = Scrollbar::default()
        .orientation(ratatui::widgets::ScrollbarOrientation::VerticalRight)
        .begin_symbol(Some("▲"))
        .end_symbol(Some("▼"))
        .track_symbol(Some("│"))
        // Half-block thumb: if a terminal briefly desyncs around wide emoji titles,
        // a full `█` ghost on the left chrome reads as a hard "shadow"; `▐` is quieter.
        .thumb_symbol("▐")
        .begin_style(Style::default().fg(app.theme.border))
        .end_style(Style::default().fg(app.theme.border))
        .track_style(Style::default().fg(app.theme.border))
        .thumb_style(Style::default().fg(app.theme.accent));
    let sb_position = if total_visual > visible_height {
        let range = total_visual - visible_height;
        (visual_scroll as u64 * (total_visual - 1) as u64 / range as u64) as usize
    } else {
        0
    };
    let sb_position = sb_position.min(total_visual.saturating_sub(1));
    let mut state = ScrollbarState::new(total_visual)
        .viewport_content_length(app.log_scroll.height as usize)
        .position(sb_position);
    frame.render_stateful_widget(scrollbar, area, &mut state);

    // Wide graphemes in card titles (e.g. 🧠) can desync some terminals' cursors while the
    // accent scrollbar thumb (`█`) is also being painted. Ghost thumb cells then stick on the
    // left chrome because unchanged border cells are skipped by Buffer::diff. Force-emit the
    // left border every frame so those residues cannot persist.
    restamp_log_left_border(frame.buffer_mut(), area, borders, &app.theme);

    // Persist prefix-sum cache for mouse hit-testing and scroll handlers outside render.
    app.log_scroll.visual_start = app.log_scroll.visual_start_cache.clone();
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
/// Uses `app.spinner_frame` (cycled 0-9) to pick a Braille spinner character,
/// and displays a "Thinking..." label with a subtle pulse.
fn render_loading_spinner(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    visual_scroll: usize,
    visible_height: usize,
) {
    let Some(idx) = app.loading_idx else { return };
    // Find logical row for this physical index
    let Some(logical_row) = app
        .log_scroll
        .phys_to_logical_cache
        .get(idx)
        .and_then(|&v| v)
    else {
        return;
    };
    let vs_cache = &app.log_scroll.visual_start_cache;
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
    let spinner_char = SPINNERS[(app.spinner_frame as usize) % SPINNERS.len()];

    let spinner_style = Style::default()
        .fg(app.theme.warning)
        .add_modifier(Modifier::BOLD);
    let text_style = Style::default()
        .fg(app.theme.accent)
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
