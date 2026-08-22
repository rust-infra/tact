use ratatui::{
    Frame,
    layout::Rect,
    text::{Line, Span},
    widgets::Borders,
};

use crate::{
    render::{renderable::Renderable, util::wrap_line},
    theme::Theme,
    widgets::state::App,
};

use agent_tui_kit::{
    render::log::render_log_panel_pure,
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
/// pure `render_log_panel_pure` (reads the built caches), now hosted in
/// `agent_tui_kit::render::log`.
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
    let ctx = app.render_ctx();
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
