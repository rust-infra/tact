use crate::render::cells::tool::ToolCell;
use crate::render::util::wrap_line;
use crate::widgets::state::App;
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
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
    // Rebuild when message count or panel width changes.
    let cache_valid = app.log_scroll.visual_cache_ver == app.messages.len()
        && app.log_scroll.visual_cache_width == wrap_width as u16;

    if !cache_valid {
        app.log_scroll.visual_cache.clear();
        app.log_scroll.visual_start_cache.clear();
        app.log_scroll.visual_start_cache.push(0);

        for logical_i in 0..total_logical {
            let line = if let Some(&phys_idx) = app.log_scroll.visible_indices.get(logical_i) {
                let base = &app.messages[phys_idx];
                if base.spans.is_empty() {
                    Line::default()
                } else {
                    base.clone()
                }
            } else {
                // Last logical row: live stream text, styled with accent color.
                Line::from(Span::styled(app.stream.buffer.as_str(), app.theme.accent))
            };
            let wrapped = wrap_line(&line, wrap_width);
            app.log_scroll.visual_cache.extend(wrapped);
            app.log_scroll
                .visual_start_cache
                .push(app.log_scroll.visual_cache.len());
        }
        app.log_scroll.visual_cache_width = wrap_width as u16;
        app.log_scroll.visual_cache_ver = app.messages.len();
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
    let max_visual_scroll = total_visual.saturating_sub(visible_height);
    // Convert logical offset to the first visible visual line.
    let visual_scroll = if logical_scroll < vs_cache.len() {
        vs_cache[logical_scroll].min(max_visual_scroll)
    } else {
        max_visual_scroll
    };
    let end_visual = (visual_scroll + visible_height).min(total_visual);
    // Bottom snap: if viewport would extend past the last line, pin to bottom.
    //
    // ```text
    //  before snap          after snap (max_visual_scroll)
    //  ... [1198][1199]     ... [1180][1181]...[1199]
    //       └─ viewport ─┘        └─ viewport ─┘
    //  end >= total_visual  →  visual_scroll = total_visual - visible_height
    // ```
    let visual_scroll = if end_visual >= total_visual && total_visual > visible_height {
        max_visual_scroll
    } else {
        visual_scroll
    };
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
            let tool_match = app.tool_blocks.iter().find(|b| {
                phys >= b.phys_idx
                    && phys <= b.phys_idx + b.output.layout.placeholder_lines as usize
            });
            if let Some(tb) = tool_match {
                let rows_before = phys.saturating_sub(tb.phys_idx);
                // Push at the summary row's visual start so LogColumnRenderer
                // computes the correct skip_lines for clipping.
                let vis_start = if rows_before > 0 && rows_before <= logical_i {
                    vs_cache[logical_i - rows_before]
                } else {
                    vs_cache[logical_i]
                };
                let card_cell = ToolCell::from_output(
                    tb.output.clone(),
                    false, // render summary + card as one unit
                    app.theme.accent,
                    app.theme.bg,
                    app.theme.fg,
                    app.theme.success,
                    app.msgs().diff_card_bottom.to_string(),
                    app.msgs().diff_overflow_tmpl.to_string(),
                );
                renderer.push(vis_start, card_cell);
                // Skip the summary logical row + all placeholder rows.
                logical_i += 1 + tb.output.layout.placeholder_lines as usize - rows_before;
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

        let cell = super::cells::text::TextCell::new(
            cached_lines,
            raw_text,
            se_search_term,
            is_match,
            is_selected,
            word_sel,
            prefix,
            log_fg,
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
    frame.render_widget(Clear, inner);
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

    // Loading spinner overlay: when a task starts (PlanGenerated) but no content
    // has arrived yet, show an animated spinner line.
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
    let Some(logical_row) = app.log_scroll.phys_to_logical_cache.get(idx).and_then(|&v| v)
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

    // Pulse intensity based on spinner_frame (alternating dim/bright)
    let pulse_intensity = if app.spinner_frame % 2 == 0 {
        200
    } else {
        160
    };

    let spinner_style = Style::default()
        .fg(Color::Rgb(pulse_intensity, pulse_intensity, 80))
        .add_modifier(Modifier::BOLD);
    let text_style = Style::default()
        .fg(Color::Rgb(pulse_intensity, pulse_intensity, 80))
        .add_modifier(Modifier::ITALIC);

    let spinner_line = Line::from(vec![
        Span::styled(format!(" {} ", spinner_char), spinner_style),
        Span::styled("Thinking...", text_style),
    ]);

    let spinner_area = Rect::new(
        area.x + 2,
        area.y + 1 + y,
        area.width.saturating_sub(4),
        1,
    );
    if spinner_area.bottom() <= area.bottom() {
        frame.render_widget(Clear, spinner_area);
        frame.render_widget(Paragraph::new(spinner_line), spinner_area);
    }
}
