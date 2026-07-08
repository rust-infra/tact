use crate::render::cells::tool::ToolCell;
use crate::render::util::wrap_line;
use crate::widgets::state::App;
use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Scrollbar, ScrollbarState},
};

/// Render the Log panel: wrapping, scrolling, search highlighting, and mouse selection.
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
///  PHYSICAL (messages[])     LOGICAL (scroll here)        VISUAL (draw here)
///  ┌───┬───┬───┬───┐         ┌───┬───┬───┐                ┌───┬───┬───┬───┬───┐
///  │ 0 │ 1 │ 2 │ 3 │  hide  │ 0 │ 1 │ 2 │  wrap long     │ 0 │ 1 │ 2 │ 3 │ 4 │
///  └───┴───┴───┴───┘  ──→    └───┴───┴───┘  ──→           └───┴───┴───┴───┴───┘
///   every stored msg          visible only              one screen line each
///                             + stream buffer           (may be many per logical)
/// ```
pub(crate) fn render_log_panel(frame: &mut Frame, area: Rect, app: &mut App) {
    // 这行是算**面板内容区的实际可用高度**。
    // area.height = Border Block 的整个矩形高度
    // ┌─ Log ──────────────┐  ← area.y + 0  (上边框，占 1 行)
    // │                     │  ← area.y + 1  (内容区第 1 行)
    // │   actual content    │  ← ...
    // │                     │  ← area.y + area.height - 2 (内容区最后一行)
    // └─────────────────────┘  ← area.y + area.height - 1 (下边框，占 1 行)
    // area.height.saturating_sub(2) = 内容区可用行数 = visible_height
    // ① Phase 2 视口裁剪 —— 决定屏幕上能显示多少行
    // let visible_height = app.log_scroll.height as usize;
    // let end_visual = (visual_scroll + visible_height).min(total_visual);
    // // ② 覆盖层裁剪 —— thinking/diff/code cards 也用它
    // render_thinking_cards(frame, area, app, visual_scroll, visible_height);
    // saturating_sub` 防的是极端情况：如果 `area.height < 2`（面板被缩到极小），不会 panic，直接归零。
    app.log_scroll.height = area.height.saturating_sub(2);
    let visible_height = app.log_scroll.height as usize;
    // 两行做两件事：
    // 和 `height` 同样的 `saturating_sub(2)`：
    // area.width = 整个 Block 的列宽
    // ┌─ Log ──────────────────┐
    // │                        │  ← area.width - 2 = 内容区可用列宽
    // └────────────────────────┘
    //     ↑ 左边框(1列)         ↑ 右边框(1列)
    let max_width = area.width.saturating_sub(2) as usize;
    // 防止 `wrap_line` 拿到 0 宽度：
    let wrap_width = if max_width > 0 { max_width } else { 1 };

    // `visible_indices_ver` 是**脏检测的版本号**。它存的是上次构建时 `messages.len()` 的值。这里的逻辑：
    // ```
    // 当前消息数量 ≠ 上次缓存时的消息数量  →  缓存过期，需要重建
    // ```
    // 这是 Phase 0 唯一的触发条件——因为只有消息增删才会改变可见索引（消息新增可能落在 thinking block 内部，需要重新判断是否可见）。消息内容变化不改变可见性，所以不用重建。
    let indices_stale = app.log_scroll.visible_indices_ver != app.messages.len();
    if indices_stale {
        app.log_scroll.visible_indices.clear();
        app.log_scroll.phys_to_logical_cache.clear();
        app.log_scroll
            .phys_to_logical_cache
            .resize(app.messages.len(), None);
        let mut total_logical = 0;
        // 遍历所有消息，将可见的物理索引添加到 visible_indices 中，并更新缓存
        for phys in 0..app.messages.len() {
            if app.is_message_visible(phys) {
                app.log_scroll.visible_indices.push(phys);
                app.log_scroll.phys_to_logical_cache[phys] = Some(total_logical);
                total_logical += 1;
            }
        }
        // update visible_indices_ver to mark cache valid
        app.log_scroll.visible_indices_ver = app.messages.len();
    }
    // total_logical: 可见的逻辑行数量
    let mut total_logical = app.log_scroll.visible_indices.len();
    // Stream buffer occupies the last logical row while tokens are arriving.
    if !app.stream.buffer.is_empty() {
        total_logical += 1;
    }

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
    let cache_valid = app.log_scroll.visual_cache_ver == app.messages.len()
        && app.log_scroll.visual_cache_width == wrap_width as u16
        && app.log_scroll.visual_cache_theme == app.theme.name;

    if !cache_valid {
        app.log_scroll.visual_cache.clear();
        app.log_scroll.visual_start_cache.clear();
        app.log_scroll.visual_start_cache.push(0);

        for logical_i in 0..total_logical {
            let line = if let Some(&phys_idx) = app.log_scroll.visible_indices.get(logical_i) {
                if super::cells::separator::is_task_end_separator(&app.raw_messages[phys_idx]) {
                    Line::default()
                } else if app.messages[phys_idx].spans.is_empty() {
                    Line::default()
                } else {
                    super::log_style::restyle_log_line(
                        &app.messages[phys_idx],
                        &app.raw_messages[phys_idx],
                        &app.theme,
                        app.raw_message_types[phys_idx],
                        super::log_style::is_user_message_line(&app.raw_messages, phys_idx),
                    )
                }
            } else {
                // Last logical row: live stream text, styled with accent color.
                Line::from(Span::styled(app.stream.buffer.as_str(), app.theme.accent))
            };
            let wrapped = if let Some(&phys_idx) = app.log_scroll.visible_indices.get(logical_i) {
                if super::cells::separator::is_task_end_separator(&app.raw_messages[phys_idx]) {
                    vec![Line::default()]
                } else {
                    wrap_line(&line, wrap_width)
                }
            } else {
                wrap_line(&line, wrap_width)
            };
            app.log_scroll.visual_cache.extend(wrapped);
            app.log_scroll
                .visual_start_cache
                .push(app.log_scroll.visual_cache.len());
        }
        app.log_scroll.visual_cache_width = wrap_width as u16;
        app.log_scroll.visual_cache_ver = app.messages.len();
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
    // Max logical offset: the last logical row whose start visual line still leaves
    // `visible_height` rows of content below it (binary search on prefix sums).
    let effective_max_logical = if total_visual <= visible_height {
        0
    } else {
        let target = total_visual - visible_height;
        app.log_scroll
            .visual_start_cache
            .binary_search(&target)
            .unwrap_or_else(|idx| idx.saturating_sub(1))
    };
    let max_scroll = effective_max_logical as u16;
    if app.log_scroll.offset > max_scroll {
        app.log_scroll.offset = max_scroll;
    }

    let logical_scroll = app.log_scroll.offset as usize;
    let vs_cache = &app.log_scroll.visual_start_cache;
    // Map the (already clamped) logical scroll offset to the first visible visual
    // line. See `resolve_visual_scroll` for the bottom-pinning rule that keeps a
    // tall last cell (e.g. a tool detail card) fully reachable.
    let visual_scroll = resolve_visual_scroll(
        vs_cache,
        total_visual,
        visible_height,
        logical_scroll,
        effective_max_logical,
    );
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
    //  │ cached_lines        │  search │ yellow highlight    │
    //  │ no search/selection │  ──→    │ or REVERSED select  │
    //  └─────────────────────┘          └─────────────────────┘
    //
    //  Viewport clipping happens twice:
    //    1. here — skip logical rows outside [logical_start, logical_end)
    //    2. LogColumnRenderer — skip_lines inside partially visible cells
    // ```

    let has_search = !app.search.term.is_empty();
    let has_selection = app.mouse.log_selection.is_some();
    let search_term = app.search.term.clone();
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

        let is_match = has_search && app.search.matches.contains(&logical_i);
        let is_selected = has_selection
            && app
                .mouse
                .log_selection
                .map(|(s, e)| logical_i >= s.min(e) && logical_i <= s.max(e))
                .unwrap_or(false);

        let phys_idx = app.log_scroll.visible_indices.get(logical_i).copied();

        // ── Message category separator ──────────────────────────────
        // Between message groups of different types (user ↔ system ↔ assistant),
        // insert a thin decorative separator line.
        if let Some(phys) = phys_idx {
            let raw = app.raw_messages[phys].as_str();
            // Determine category for this line
            let category = if raw.starts_with("💬") {
                "user"
            } else if raw.starts_with("  ") {
                // Continuation line: same as previous category
                prev_category.unwrap_or("assistant")
            } else if raw.starts_with("✓")
                || raw.starts_with("✗")
                || raw.starts_with("⚠")
                || raw.starts_with("📝")
                || raw.starts_with("❌")
                || raw.starts_with("✅")
                || raw.starts_with("▶")
                || raw.starts_with("🤖")
                || raw.starts_with("  ██")
            {
                "system"
            } else {
                "assistant"
            };

            // Insert separator if category changed (and not first line)
            if let Some(prev) = prev_category {
                if prev != category {
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

        // Task-end rule: full-width dashed line, width resolved at render time.
        if let Some(phys) = phys_idx {
            if super::cells::separator::is_task_end_separator(&app.raw_messages[phys]) {
                let sep = super::cells::separator::TaskEndSeparator::new(app.theme.border);
                renderer.push(vs_cache[logical_i], sep);
                logical_i += 1;
                continue;
            }
        }

        // Normal row: build TextCell
        let cached_lines: Vec<Line<'static>> =
            app.log_scroll.visual_cache[cache_start..cache_end].to_vec();
        let raw_text = phys_idx
            .map(|p| app.raw_messages[p].clone())
            .unwrap_or_default();
        let word_sel = app
            .mouse
            .log_word_selection
            .filter(|_| is_selected && phys_idx.is_some());
        let se_search_term = if is_match {
            search_term.clone()
        } else {
            String::new()
        };

        // Thinking blocks with >3 hidden lines get a "↑ N/M blocks hidden ↑" prefix.
        let prefix = phys_idx.and_then(|phys| {
            app.thinking.blocks.iter().find_map(|block| {
                if block.title_idx == phys {
                    let total = block.end_idx.saturating_sub(block.title_idx);
                    if total > 3 {
                        Some(
                            app.msgs()
                                .scroll_indicator_tmpl
                                .replacen("{}", &total.min(3).to_string(), 1)
                                .replacen("{}", &total.to_string(), 1),
                        )
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
        });

        let indent_cols = phys_idx.map(|p| app.nested_log_indent(p)).unwrap_or(0);

        let cell = super::cells::text::TextCell::new(
            cached_lines,
            raw_text,
            se_search_term,
            is_match,
            is_selected,
            word_sel,
            prefix,
            indent_cols,
            log_fg,
            app.theme.search_match_bg(),
            app.theme.search_match_fg(),
        );

        // Push at this row's visual-line offset; LogColumnRenderer does a second
        // viewport clip and calls TextCell::render_partial for sub-line trimming.
        renderer.push(vs_cache[logical_i], cell);
        logical_i += 1;
    }

    // Build panel title — show search count when search is active
    let panel_title: String = if has_search {
        let total = app.search.matches.len();
        let current = if total > 0 {
            app.search.current_match + 1
        } else {
            0
        };
        app.msgs()
            .log_search_count_tmpl
            .replacen("{}", &current.to_string(), 1)
            .replacen("{}", &total.to_string(), 1)
    } else {
        app.msgs().log_title.to_string()
    };

    // Render bordered log panel
    let log_block = Block::default()
        .borders(Borders::ALL)
        .border_type(app.theme.block_border_type())
        .border_style(Style::default().fg(app.theme.border))
        .title(panel_title)
        .style(Style::default().bg(app.theme.bg));
    let inner = Rect::new(
        area.x + 1,
        area.y + 1,
        area.width.saturating_sub(2),
        area.height.saturating_sub(2),
    );
    frame.render_widget(log_block, area);
    frame.render_widget(
        Paragraph::new("").style(Style::default().bg(app.theme.bg)),
        inner,
    );
    frame.render_widget(renderer, inner);

    // Overlays (thinking / code cards) share the same visual viewport.
    // Diff cards are now handled as ToolCells in LogColumnRenderer (Phase 3).
    //
    // ```text
    //  ┌─ Log panel ─────────────────┐
    //  │  TextCell rows (base layer) │
    //  │  + thinking card overlay    │  each layer clips with
    //  │  + code card overlay        │  (visual_scroll, visible_height)
    //  └─────────────────────────────┘
    super::cells::thinking::render_thinking_cards(frame, area, app, visual_scroll, visible_height);

    super::cells::code::render_code_cards(frame, area, app, visual_scroll, visible_height);

    // Loading spinner overlay: legacy PlanGenerated path only. Current agent
    // updates usually drive the plan/tool UI through StepAdded / StepStarted.
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
        .thumb_symbol("█")
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

    // Persist prefix-sum cache for mouse hit-testing and scroll handlers outside render.
    app.log_scroll.visual_start = app.log_scroll.visual_start_cache.clone();
}

/// Map a clamped logical scroll offset to the first visible visual line.
///
/// The log scrolls in *logical-row* units but renders in *visual lines*. A single
/// logical row may wrap to many visual lines (long paragraph) or be one of several
/// 1-line rows (a tool card's reserved placeholder rows).
///
/// `effective_max_logical` is the largest logical offset the user can reach. When
/// the offset is at (or beyond) that maximum we must pin the viewport to the true
/// visual bottom (`total_visual - visible_height`). Otherwise, when the max logical
/// row *begins above* `max_visual_scroll` — e.g. a long wrapped paragraph sitting
/// directly above a tall tool detail card — using `vs_cache[logical_scroll]` would
/// leave the card's final rows below the viewport, unreachable: the card shows only
/// its top border (an empty box) and `G` / wheel-down can't scroll into it.
fn resolve_visual_scroll(
    vs_cache: &[usize],
    total_visual: usize,
    visible_height: usize,
    logical_scroll: usize,
    effective_max_logical: usize,
) -> usize {
    let max_visual_scroll = total_visual.saturating_sub(visible_height);
    if logical_scroll >= effective_max_logical {
        return max_visual_scroll;
    }
    vs_cache
        .get(logical_scroll)
        .copied()
        .unwrap_or(max_visual_scroll)
        .min(max_visual_scroll)
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

#[cfg(test)]
mod tests {
    use super::resolve_visual_scroll;

    /// The max logical row starts *above* `max_visual_scroll` because the row just
    /// above the last (tall) cell is a long wrapped paragraph. At the max offset the
    /// viewport must pin to the true bottom so the tall cell's last rows are visible.
    ///
    /// Layout: 3 logical rows
    ///   row0: visual [0..68)   (68-line wrapped paragraph)
    ///   row1: visual [68..72)  (paragraph just above the card)
    ///   row2: visual [72..100) (28-row tool detail card)
    /// total_visual = 100, visible_height = 30 → max_visual_scroll = 70.
    /// target = 70 falls between vs_cache[1]=68 and vs_cache[2]=72, so
    /// effective_max_logical = 1, which begins at 68 (< 70).
    #[test]
    fn pins_to_bottom_when_max_row_starts_above_max_visual_scroll() {
        let vs_cache = [0usize, 68, 72, 100];
        let total_visual = 100;
        let visible_height = 30;
        let effective_max_logical = 1;

        // At the max offset, must reach the true bottom (70), not 68.
        let vs = resolve_visual_scroll(
            &vs_cache,
            total_visual,
            visible_height,
            effective_max_logical,
            effective_max_logical,
        );
        assert_eq!(vs, 70, "should pin to max_visual_scroll at the bottom");
        assert_eq!(
            vs + visible_height,
            total_visual,
            "viewport reaches the last line"
        );
    }

    #[test]
    fn uses_row_start_when_above_max_offset() {
        let vs_cache = [0usize, 68, 72, 100];
        // Mid-scroll (offset 1, max is 2) uses the row's visual start, clamped.
        let vs = resolve_visual_scroll(&vs_cache, 100, 30, 1, 2);
        assert_eq!(vs, 68);
    }

    #[test]
    fn content_shorter_than_viewport_stays_at_top() {
        let vs_cache = [0usize, 1, 2, 3];
        // total_visual (3) <= visible_height (30): everything fits, no scroll.
        let vs = resolve_visual_scroll(&vs_cache, 3, 30, 0, 0);
        assert_eq!(vs, 0);
    }
}
