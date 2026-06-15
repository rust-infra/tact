use crate::render::util::wrap_line;
use crate::state::App;
use ratatui::{
    Frame,
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Scrollbar, ScrollbarState},
};

/// Render the Log panel: supports automatic line wrapping for long lines, scrolling, search highlighting, and mouse selection highlighting.
pub(crate) fn render_log_panel(frame: &mut Frame, area: Rect, app: &mut App) {
    app.log_scroll.height = area.height.saturating_sub(2);
    let visible_height = app.log_scroll.height as usize;
    let max_width = area.width.saturating_sub(2) as usize;
    let wrap_width = if max_width > 0 { max_width } else { 1 };

    // ---- visible_indices ----
    let indices_stale = app.log_scroll.visible_indices_ver != app.messages.len();
    if indices_stale {
        app.visible_indices.clear();
        app.log_scroll.phys_to_logical_cache.clear();
        app.log_scroll
            .phys_to_logical_cache
            .resize(app.messages.len(), None);
        let mut total_logical = 0;
        for phys in 0..app.messages.len() {
            if app.is_message_visible(phys) {
                app.visible_indices.push(phys);
                app.log_scroll.phys_to_logical_cache[phys] = Some(total_logical);
                total_logical += 1;
            }
        }
        app.log_scroll.visible_indices_ver = app.messages.len();
    }
    let mut total_logical = app.visible_indices.len();
    if !app.stream.buffer.is_empty() {
        total_logical += 1;
    }

    // ---- Phase 1: wrap cache ----
    let cache_valid = app.log_scroll.visual_cache_ver == app.messages.len()
        && app.log_scroll.visual_cache_width == wrap_width as u16;

    if !cache_valid {
        app.log_scroll.visual_cache.clear();
        app.log_scroll.visual_start_cache.clear();
        app.log_scroll.visual_start_cache.push(0);

        for logical_i in 0..total_logical {
            let line = if let Some(&phys_idx) = app.visible_indices.get(logical_i) {
                let base = &app.messages[phys_idx];
                if base.spans.is_empty() {
                    Line::default()
                } else {
                    base.clone()
                }
            } else {
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

    // ---- Phase 2: clip viewport ----
    let total_visual = *app.log_scroll.visual_start_cache.last().unwrap_or(&0);
    let effective_max_logical = if total_visual <= visible_height {
        0
    } else {
        let target = total_visual - visible_height;
        app.log_scroll.visual_start_cache.binary_search(&target).unwrap_or_else(|idx| idx.saturating_sub(1))
    };
    let max_scroll = effective_max_logical as u16;
    if app.log_scroll.offset > max_scroll {
        app.log_scroll.offset = max_scroll;
    }

    let logical_scroll = app.log_scroll.offset as usize;
    let vs_cache = &app.log_scroll.visual_start_cache;
    let max_visual_scroll = total_visual.saturating_sub(visible_height);
    let visual_scroll = if logical_scroll < vs_cache.len() {
        vs_cache[logical_scroll].min(max_visual_scroll)
    } else {
        max_visual_scroll
    };
    let end_visual = (visual_scroll + visible_height).min(total_visual);
    let visual_scroll = if end_visual >= total_visual && total_visual > visible_height {
        max_visual_scroll
    } else {
        visual_scroll
    };
    let end_visual = (visual_scroll + visible_height).min(total_visual);

    // ---- Phase 3: build cells and render ----
    let logical_start = vs_cache.binary_search(&visual_scroll).unwrap_or_else(|i| i.saturating_sub(1));
    let logical_end = match vs_cache.binary_search(&end_visual) {
        Ok(i) => i,
        Err(i) => i.min(total_logical),
    };

    let has_search = !app.search.term.is_empty();
    let has_selection = app.mouse.log_selection.is_some();
    let search_term = app.search.term.clone();
    let log_fg = app.theme.fg;

    let mut renderer = super::log_column::LogColumnRenderer::new()
        .with_viewport(visual_scroll, visible_height);

    for logical_i in logical_start..logical_end {
        let cache_start = vs_cache[logical_i];
        let cache_end = vs_cache[logical_i + 1];
        if cache_end <= visual_scroll || cache_start >= end_visual {
            continue;
        }

        let is_match = has_search && app.search.matches.contains(&logical_i);
        let is_selected = has_selection
            && app
                .mouse
                .log_selection
                .map(|(s, e)| logical_i >= s.min(e) && logical_i <= s.max(e))
                .unwrap_or(false);

        let phys_idx = app.visible_indices.get(logical_i).copied();
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

        // Build thinking collapse indicator prefix
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

        renderer.push(vs_cache[logical_i], cell);
    }

    // Render bordered log panel
    let log_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(app.theme.border))
        .title(app.msgs().log_title)
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

    // ---- Card overlay: thinking blocks ----
    super::cells::thinking::render_thinking_cards(
        frame, area, app, visual_scroll, visible_height);

    // ---- Diff block overlay ----
    super::cells::diff::render_diff_cards(
        frame, area, app, visual_scroll, visible_height);

    // ---- Code block overlay ----
    super::cells::code::render_code_cards(
        frame, area, app, visual_scroll, visible_height);

    // Scrollbar
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

    app.log_scroll.visual_start = app.log_scroll.visual_start_cache.clone();
}
