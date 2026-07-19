//! Mouse handling extracted from the main event loop for testability.

use crate::widgets::state::{
    App, FocusedPanel, LogSelection, PopupTextHit, PopupTextSelection, TextPosition,
};
use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct MousePanelHit {
    pub in_log: bool,
    pub in_plan: bool,
    pub in_divider: bool,
}

fn panel_hit(app: &App, column: u16, row: u16) -> MousePanelHit {
    MousePanelHit {
        in_log: point_in_rect(column, row, app.mouse.log_area),
        in_plan: point_in_rect(column, row, app.mouse.plan_area),
        in_divider: point_in_rect(column, row, app.mouse.divider_area),
    }
}

fn point_in_rect(column: u16, row: u16, area: ratatui::layout::Rect) -> bool {
    column >= area.x && column < area.x + area.width && row >= area.y && row < area.y + area.height
}

/// Dispatch a mouse event (scroll, click, drag, resize).
pub(crate) fn handle_mouse_event(app: &mut App, mouse: MouseEvent) {
    match mouse.kind {
        MouseEventKind::ScrollUp => {
            let hit = panel_hit(app, mouse.column, mouse.row);
            handle_mouse_scroll_up(app, hit);
        }
        MouseEventKind::ScrollDown => {
            let hit = panel_hit(app, mouse.column, mouse.row);
            handle_mouse_scroll_down(app, hit);
        }
        MouseEventKind::Down(MouseButton::Left) if app.tools.popup.is_some() => {
            handle_diff_popup_mouse_down(app, mouse);
        }
        MouseEventKind::Down(MouseButton::Left) => {
            let hit = panel_hit(app, mouse.column, mouse.row);
            handle_mouse_down(app, mouse, hit);
        }
        MouseEventKind::Drag(MouseButton::Left) if app.tools.popup.is_some() => {
            handle_diff_popup_mouse_drag(app, mouse);
        }
        MouseEventKind::Drag(MouseButton::Left) => {
            let hit = panel_hit(app, mouse.column, mouse.row);
            handle_mouse_drag(app, mouse, hit);
        }
        MouseEventKind::Up(MouseButton::Left) => {
            app.mouse.diff_popup_drag_origin = None;
            app.mouse.dragging_log = false;
            app.mouse.dragging_plan = false;
            end_panel_resize(app);
        }
        _ => {}
    }
}

/// Handle mouse wheel up.
pub(crate) fn handle_mouse_scroll_up(app: &mut App, hit: MousePanelHit) {
    if app.has_overlay_popup() {
        app.overlay_popup_scroll_up();
    } else if hit.in_log && app.log_scroll.offset > 0 {
        app.log_scroll.offset -= 1;
    } else if hit.in_plan && app.plan.selected > 0 {
        app.plan.selected -= 1;
        app.plan.list_state.select(Some(app.plan.selected));
    }
}

/// Handle mouse wheel down.
pub(crate) fn handle_mouse_scroll_down(app: &mut App, hit: MousePanelHit) {
    if app.has_overlay_popup() {
        app.overlay_popup_scroll_down();
    } else if hit.in_log {
        app.log_scroll.offset = app.log_scroll.offset.saturating_add(1);
    } else if hit.in_plan
        && !app.plan.steps.is_empty()
        && app.plan.selected + 1 < app.plan.steps.len()
    {
        app.plan.selected += 1;
        app.plan.list_state.select(Some(app.plan.selected));
    }
}

fn handle_mouse_down(app: &mut App, mouse: MouseEvent, hit: MousePanelHit) {
    if app.close_overlay_on_outside_click(mouse.column, mouse.row) {
        return;
    }
    if hit.in_divider {
        begin_panel_resize(app);
        return;
    }
    if hit.in_log {
        handle_log_click(app, mouse);
        return;
    }
    if hit.in_plan {
        app.focused_panel = FocusedPanel::Plan;
        let item_idx = (mouse.row.saturating_sub(app.mouse.plan_area.y + 1)) as usize;
        if item_idx < app.plan.steps.len() {
            app.plan.selected = item_idx;
            app.plan.list_state.select(Some(app.plan.selected));
            app.mouse.plan_selection = Some((item_idx, item_idx));
            app.mouse.dragging_plan = true;
        }
    }
}

fn handle_diff_popup_mouse_down(app: &mut App, mouse: MouseEvent) {
    let inside_popup = point_in_rect(mouse.column, mouse.row, app.mouse.diff_popup_area);
    app.close_overlay_on_outside_click(mouse.column, mouse.row);
    if !inside_popup || !point_in_rect(mouse.column, mouse.row, app.mouse.diff_popup_body_area) {
        return;
    }

    let Some(origin) = diff_popup_text_hit(app, mouse.column, mouse.row, false) else {
        return;
    };
    if let Some(popup) = app.tools.popup.as_mut() {
        popup.selection = Some(PopupTextSelection::new(origin.start, origin.start));
        app.mouse.diff_popup_drag_origin = Some(origin);
    }
}

fn diff_popup_text_hit(
    app: &App,
    column: u16,
    row: u16,
    clamp_vertical: bool,
) -> Option<PopupTextHit> {
    let first = app.mouse.diff_popup_hit_rows.first()?;
    let last = app.mouse.diff_popup_hit_rows.last()?;
    let body = app.mouse.diff_popup_body_area;

    if row < body.y {
        return clamp_vertical.then(|| PopupTextHit::empty(first.line_start));
    }
    if row >= body.y.saturating_add(body.height) {
        return clamp_vertical.then(|| PopupTextHit::empty(last.line_end));
    }
    app.mouse
        .diff_popup_hit_rows
        .iter()
        .find(|hit_row| hit_row.screen_y == row)
        .map(|hit_row| hit_row.hit(column))
}

fn handle_log_click(app: &mut App, mouse: MouseEvent) {
    app.focused_panel = FocusedPanel::Log;
    let visual_base = app
        .log_scroll
        .visual_start
        .get(app.log_scroll.offset as usize)
        .copied()
        .unwrap_or(0);
    let visual_row = visual_base + mouse.row.saturating_sub(app.mouse.log_area.y + 1) as usize;
    let line_idx = app.logical_from_visual(visual_row);
    let col = mouse.column.saturating_sub(app.mouse.log_area.x + 1) as usize;

    let now = std::time::Instant::now();
    let pos = (mouse.column, mouse.row);
    let is_same_click = app.mouse.last_click_pos == Some(pos)
        && app
            .mouse
            .last_click_time
            .is_some_and(|t| now.duration_since(t).as_millis() < 500);
    if is_same_click {
        app.mouse.click_count = (app.mouse.click_count + 1).min(3);
    } else {
        app.mouse.click_count = 1;
    }
    app.mouse.last_click_time = Some(now);
    app.mouse.last_click_pos = Some(pos);

    let Some(phys_idx) = app.visible_message_index(line_idx) else {
        return;
    };

    let card_hit = app.thinking.blocks.iter().position(|b| {
        app.phys_to_logical_fast(b.title_idx)
            .zip(app.phys_to_logical_fast(b.end_idx + 1))
            .is_some_and(|(tl, bl)| line_idx >= tl && line_idx < bl)
    });
    if let Some(card_idx) = card_hit {
        if app.mouse.click_count == 1 {
            app.mouse.last_click_card = Some(card_idx);
            app.mouse.log_selection = None;
            app.mouse.dragging_log = false;
        } else if app.mouse.click_count == 2 && app.mouse.last_click_card == Some(card_idx) {
            let block = &app.thinking.blocks[card_idx];
            app.open_thinking_popup(block.title_idx);
        } else if app.mouse.click_count >= 3 {
            handle_log_triple_click(app, line_idx, false);
        }
    } else {
        app.mouse.last_click_card = None;
    }

    if let Some((tool_idx, tool_phys, logical_start, _)) = app.find_tool_at_logical(line_idx) {
        let relative_row = line_idx - logical_start;
        handle_tool_block_click(app, tool_idx, tool_phys, relative_row);
        if app.mouse.click_count >= 3 {
            handle_log_triple_click(app, line_idx, false);
        }
        return;
    }

    app.mouse.last_click_tool = None;
    let code_hit = app.code_blocks.iter().enumerate().find(|(_, b)| {
        app.phys_to_logical_fast(b.start_idx)
            .is_some_and(|si| line_idx >= si)
            && app
                .phys_to_logical_fast(b.end_idx)
                .is_some_and(|ei| line_idx < ei)
    });
    if let Some((code_idx, _block)) = code_hit {
        if app.mouse.click_count == 1 {
            app.mouse.last_click_code = Some(code_idx);
            app.mouse.log_selection = None;
            app.mouse.dragging_log = false;
        } else if app.mouse.click_count == 2 && app.mouse.last_click_code == Some(code_idx) {
            app.open_code_popup(code_idx);
        } else if app.mouse.click_count >= 3 {
            handle_log_triple_click(app, line_idx, false);
        }
        return;
    }

    app.mouse.last_click_code = None;
    let thinking_title = app.thinking.blocks.iter().any(|b| b.title_idx == phys_idx);
    if thinking_title {
        app.open_thinking_popup(phys_idx);
        return;
    }

    if app.mouse.click_count == 2 {
        if let Some((phys, byte)) = app.byte_offset_from_log_position(line_idx, visual_row, col)
            && let Some((ws, we)) = app.find_word_bounds(line_idx, byte)
        {
            app.mouse.log_selection = Some(LogSelection::span(phys, ws, we));
        }
        app.mouse.dragging_log = true;
    } else if app.mouse.click_count >= 3 {
        handle_log_triple_click(app, line_idx, true);
    } else if let Some((phys, byte)) = app.byte_offset_from_log_position(line_idx, visual_row, col)
    {
        app.mouse.log_selection = Some(LogSelection::span(phys, byte, byte));
        app.mouse.dragging_log = true;
    }
}

fn handle_mouse_drag(app: &mut App, mouse: MouseEvent, hit: MousePanelHit) {
    if app.mouse.is_resizing_panel {
        let total_width =
            app.mouse.plan_area.width + app.mouse.divider_area.width + app.mouse.log_area.width;
        update_panel_resize(app, mouse.column, app.mouse.plan_area.x, total_width);
    } else if app.mouse.dragging_log && hit.in_log {
        let visual_base = app
            .log_scroll
            .visual_start
            .get(app.log_scroll.offset as usize)
            .copied()
            .unwrap_or(0);
        let visual_row = visual_base + mouse.row.saturating_sub(app.mouse.log_area.y + 1) as usize;
        let line_idx = app.logical_from_visual(visual_row);
        let col = mouse.column.saturating_sub(app.mouse.log_area.x + 1) as usize;
        if line_idx < app.total_log_lines()
            && let Some((phys, byte)) = app.byte_offset_from_log_position(line_idx, visual_row, col)
            && let Some(ref mut sel) = app.mouse.log_selection
        {
            sel.end = TextPosition::new(phys, byte);
        }
    } else if app.mouse.dragging_plan && hit.in_plan {
        let item_idx = (mouse.row.saturating_sub(app.mouse.plan_area.y + 1)) as usize;
        if item_idx < app.plan.steps.len()
            && let Some((start, _)) = app.mouse.plan_selection
        {
            app.mouse.plan_selection = Some((start, item_idx));
        }
    }
}

fn handle_diff_popup_mouse_drag(app: &mut App, mouse: MouseEvent) {
    let Some(origin) = app.mouse.diff_popup_drag_origin else {
        return;
    };
    let Some(current) = diff_popup_text_hit(app, mouse.column, mouse.row, true) else {
        return;
    };
    let selection = if current.end > origin.start {
        PopupTextSelection::new(origin.start, current.end)
    } else {
        PopupTextSelection::new(origin.end, current.start)
    };
    if let Some(popup) = app.tools.popup.as_mut() {
        popup.selection = Some(selection);
    }
}

/// Begin dragging the Plan/Log divider to resize panels.
pub(crate) fn begin_panel_resize(app: &mut App) {
    app.mouse.is_resizing_panel = true;
}

/// Update `panel_split_ratio` while the divider is being dragged.
pub(crate) fn update_panel_resize(
    app: &mut App,
    mouse_column: u16,
    plan_area_x: u16,
    total_width: u16,
) {
    if !app.mouse.is_resizing_panel || total_width == 0 {
        return;
    }
    let mouse_x = mouse_column.saturating_sub(plan_area_x);
    let new_ratio = mouse_x as f64 / total_width as f64;
    app.panel_split_ratio = new_ratio.clamp(0.10, 0.70);
}

/// End panel divider drag resize.
pub(crate) fn end_panel_resize(app: &mut App) {
    app.mouse.is_resizing_panel = false;
}

/// Triple-click on a log line selects the line (or whole code block when enabled).
pub(crate) fn handle_log_triple_click(app: &mut App, line_idx: usize, expand_code_blocks: bool) {
    if expand_code_blocks
        && let Some((cb_start, cb_end)) = app.find_code_block_containing_logical(line_idx)
    {
        if let Some(start_phys) = app.visible_message_index(cb_start) {
            let end_phys = app.visible_message_index(cb_end).unwrap_or(start_phys);
            let end_len = app.raw_messages[end_phys].len();
            app.mouse.log_selection = Some(LogSelection::new(
                TextPosition::new(start_phys, 0),
                TextPosition::new(end_phys, end_len),
            ));
        }
        app.mouse.dragging_log = true;
        return;
    }
    if let Some(phys) = app.visible_message_index(line_idx) {
        let len = app.raw_messages[phys].len();
        app.mouse.log_selection = Some(LogSelection::full_message(phys, len));
    }
    app.mouse.dragging_log = true;
}

/// Double-click on a tool detail card opens its detail popup.
pub(crate) fn handle_tool_block_click(
    app: &mut App,
    tool_idx: usize,
    phys_idx: usize,
    relative_row: usize,
) {
    if app.mouse.click_count == 2 && app.mouse.last_click_tool == Some(tool_idx) {
        app.open_diff_popup_at_row(phys_idx, relative_row);
        return;
    }
    if app.mouse.click_count == 1 {
        app.mouse.last_click_tool = Some(tool_idx);
        app.mouse.log_selection = None;
        app.mouse.dragging_log = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::test_harness::make_app;
    use crate::widgets::state::{DiffPopup, PopupHitRow, PopupTextHit, PopupTextSelection};
    use crate::widgets::tool_widget::TOOL_HEADER_ROWS;
    use crossterm::event::KeyModifiers;
    use ratatui::layout::Rect;
    use std::collections::HashMap;
    use tact_protocol::{AgentUpdate, PlanStep, StepResult, StepStatus};

    fn mouse_event(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    fn mouse_down(column: u16, row: u16) -> MouseEvent {
        mouse_event(MouseEventKind::Down(MouseButton::Left), column, row)
    }

    fn mouse_drag(column: u16, row: u16) -> MouseEvent {
        mouse_event(MouseEventKind::Drag(MouseButton::Left), column, row)
    }

    fn mouse_up(column: u16, row: u16) -> MouseEvent {
        mouse_event(MouseEventKind::Up(MouseButton::Left), column, row)
    }

    fn popup_hit_row(screen_y: u16, text_x: u16, line_start: usize, text: &str) -> PopupHitRow {
        let cells = text
            .char_indices()
            .map(|(offset, ch)| {
                PopupTextHit::new(line_start + offset, line_start + offset + ch.len_utf8())
            })
            .collect();
        PopupHitRow {
            screen_y,
            text_x,
            line_start,
            line_end: line_start + text.len(),
            cells,
        }
    }

    fn app_with_selectable_tool_popup() -> App {
        let mut app = make_app();
        app.add_system_message("under the popup".into());
        app.mouse.log_area = Rect::new(0, 0, 40, 20);
        app.mouse.diff_popup_area = Rect::new(5, 5, 24, 8);
        app.mouse.diff_popup_body_area = Rect::new(6, 6, 22, 5);
        app.mouse.diff_popup_hit_rows = vec![
            popup_hit_row(6, 10, 0, "alpha"),
            popup_hit_row(7, 10, 6, "omega"),
        ];
        app.tools.popup = Some(DiffPopup {
            title: "tool output".into(),
            file_path: None,
            git_diff_path: None,
            workspace_dir: None,
            inline_content: Some("alpha\nomega".into()),
            lang: String::new(),
            use_diff_gutter: false,
            is_diff: false,
            scroll: 0,
            selection: None,
            cached_content: Some("alpha\nomega".into()),
            highlighted_lines: Vec::new(),
        });
        app
    }

    #[test]
    fn popup_mouse_down_starts_empty_selection_without_selecting_log() {
        let mut app = app_with_selectable_tool_popup();

        handle_mouse_event(&mut app, mouse_down(10, 6));

        assert_eq!(
            app.tools.popup.as_ref().unwrap().selection,
            Some(PopupTextSelection::new(0, 0))
        );
        assert!(app.mouse.log_selection.is_none());
    }

    #[test]
    fn popup_mouse_down_in_body_prefix_starts_selection() {
        let mut app = app_with_selectable_tool_popup();

        handle_mouse_event(&mut app, mouse_down(9, 6));

        assert_eq!(
            app.tools.popup.as_ref().unwrap().selection,
            Some(PopupTextSelection::new(0, 0))
        );
        assert_eq!(
            app.mouse.diff_popup_drag_origin,
            Some(PopupTextHit::empty(0))
        );
    }

    #[test]
    fn popup_mouse_down_on_left_border_preserves_selection_and_drag_state() {
        let mut app = app_with_selectable_tool_popup();
        let selection = Some(PopupTextSelection::new(1, 4));
        let drag_origin = Some(PopupTextHit::new(1, 2));
        app.tools.popup.as_mut().unwrap().selection = selection;
        app.mouse.diff_popup_drag_origin = drag_origin;

        handle_mouse_event(&mut app, mouse_down(5, 6));

        assert_eq!(app.tools.popup.as_ref().unwrap().selection, selection);
        assert_eq!(app.mouse.diff_popup_drag_origin, drag_origin);
    }

    #[test]
    fn popup_mouse_down_on_scrollbar_preserves_selection_and_drag_state() {
        let mut app = app_with_selectable_tool_popup();
        let selection = Some(PopupTextSelection::new(1, 4));
        let drag_origin = Some(PopupTextHit::new(1, 2));
        app.tools.popup.as_mut().unwrap().selection = selection;
        app.mouse.diff_popup_drag_origin = drag_origin;

        handle_mouse_event(&mut app, mouse_down(28, 6));

        assert_eq!(app.tools.popup.as_ref().unwrap().selection, selection);
        assert_eq!(app.mouse.diff_popup_drag_origin, drag_origin);
    }

    #[test]
    fn popup_forward_drag_includes_both_endpoint_scalars() {
        let mut app = app_with_selectable_tool_popup();

        handle_mouse_event(&mut app, mouse_down(10, 6));
        handle_mouse_event(&mut app, mouse_drag(14, 6));

        assert_eq!(
            app.tools.popup.as_ref().unwrap().copy_content().as_deref(),
            Some("alpha")
        );
    }

    #[test]
    fn popup_backward_drag_includes_both_endpoint_scalars() {
        let mut app = app_with_selectable_tool_popup();

        handle_mouse_event(&mut app, mouse_down(14, 6));
        handle_mouse_event(&mut app, mouse_drag(10, 6));

        assert_eq!(
            app.tools.popup.as_ref().unwrap().copy_content().as_deref(),
            Some("alpha")
        );
    }

    #[test]
    fn popup_drag_from_first_scalar_into_prefix_includes_origin_scalar() {
        let mut app = app_with_selectable_tool_popup();

        handle_mouse_event(&mut app, mouse_down(10, 6));
        handle_mouse_event(&mut app, mouse_drag(9, 6));

        let popup = app.tools.popup.as_ref().unwrap();
        assert_eq!(popup.selection, Some(PopupTextSelection::new(1, 0)));
        assert_eq!(popup.copy_content().as_deref(), Some("a"));
    }

    #[test]
    fn popup_mouse_up_stops_future_drag_updates() {
        let mut app = app_with_selectable_tool_popup();
        handle_mouse_event(&mut app, mouse_down(10, 6));
        handle_mouse_event(&mut app, mouse_drag(14, 6));
        handle_mouse_event(&mut app, mouse_up(14, 6));

        handle_mouse_event(&mut app, mouse_drag(14, 7));

        assert_eq!(
            app.tools.popup.as_ref().unwrap().copy_content().as_deref(),
            Some("alpha")
        );
    }

    #[test]
    fn popup_scroll_preserves_selection() {
        let mut app = app_with_selectable_tool_popup();
        handle_mouse_event(&mut app, mouse_down(10, 6));
        handle_mouse_event(&mut app, mouse_drag(14, 6));
        let selection = app.tools.popup.as_ref().unwrap().selection;

        handle_mouse_event(&mut app, mouse_event(MouseEventKind::ScrollDown, 10, 6));

        let popup = app.tools.popup.as_ref().unwrap();
        assert_eq!(popup.scroll, 1);
        assert_eq!(selection, Some(PopupTextSelection::new(0, 5)));
        assert_eq!(popup.selection, selection);
    }

    #[test]
    fn popup_drag_above_body_clamps_to_first_visible_boundary_without_scrolling() {
        let mut app = app_with_selectable_tool_popup();
        handle_mouse_event(&mut app, mouse_down(14, 7));

        handle_mouse_event(&mut app, mouse_drag(14, 5));

        let popup = app.tools.popup.as_ref().unwrap();
        assert_eq!(popup.selection, Some(PopupTextSelection::new(11, 0)));
        assert_eq!(popup.copy_content().as_deref(), Some("alpha\nomega"));
        assert_eq!(popup.scroll, 0);
    }

    #[test]
    fn popup_drag_below_body_clamps_to_last_visible_boundary_without_scrolling() {
        let mut app = app_with_selectable_tool_popup();
        handle_mouse_event(&mut app, mouse_down(10, 6));

        handle_mouse_event(&mut app, mouse_drag(10, 11));

        let popup = app.tools.popup.as_ref().unwrap();
        assert_eq!(popup.selection, Some(PopupTextSelection::new(0, 11)));
        assert_eq!(popup.copy_content().as_deref(), Some("alpha\nomega"));
        assert_eq!(popup.scroll, 0);
    }

    #[test]
    fn outside_click_still_closes_tool_popup() {
        let mut app = app_with_selectable_tool_popup();

        handle_mouse_event(&mut app, mouse_down(0, 0));

        assert!(app.tools.popup.is_none());
        assert!(app.mouse.log_selection.is_none());
    }

    #[test]
    fn scroll_up_in_log_decrements_offset() {
        let mut app = make_app();
        app.log_scroll.offset = 3;

        handle_mouse_scroll_up(
            &mut app,
            MousePanelHit {
                in_log: true,
                in_plan: false,
                in_divider: false,
            },
        );

        assert_eq!(app.log_scroll.offset, 2);
    }

    #[test]
    fn scroll_down_in_log_increments_offset() {
        let mut app = make_app();
        app.log_scroll.offset = 1;

        handle_mouse_scroll_down(
            &mut app,
            MousePanelHit {
                in_log: true,
                in_plan: false,
                in_divider: false,
            },
        );

        assert_eq!(app.log_scroll.offset, 2);
    }

    #[test]
    fn scroll_in_diff_popup_increments_popup_scroll() {
        let mut app = make_app();
        app.tools.popup = Some(DiffPopup {
            title: "t".into(),
            file_path: None,
            git_diff_path: None,
            workspace_dir: None,
            inline_content: Some("line\n".into()),
            lang: String::new(),
            use_diff_gutter: false,
            is_diff: false,
            scroll: 0,
            selection: None,
            cached_content: None,
            highlighted_lines: Vec::new(),
        });

        handle_mouse_scroll_down(&mut app, MousePanelHit::default());

        assert_eq!(app.tools.popup.as_ref().unwrap().scroll, 1);
    }

    #[test]
    fn double_click_tool_block_opens_diff_popup() {
        let mut app = make_app();
        app.plan.visible = true;
        app.handle_agent_update(AgentUpdate::StepAdded(PlanStep::new(
            "run",
            "bash",
            "b1",
            HashMap::from([("command".to_string(), "echo hi".to_string())]),
        )));
        app.handle_agent_update(AgentUpdate::StepStarted {
            idx: 0,
            tool_id: "b1".into(),
            tool_name: "bash".into(),
            arg_summary: "echo hi".into(),
            arg_full: "echo hi".into(),
        });
        app.handle_agent_update(AgentUpdate::StepFinished {
            idx: 0,
            tool_id: "b1".into(),
            result: StepResult {
                tool: "bash".into(),
                arg_summary: "echo hi".into(),
                arg_full: Some("echo hi".into()),
                status: StepStatus::Success,
                message: "ok".into(),
                detail: Some("hi\n".into()),
                duration_us: Some(1),
                permission_label: None,
            },
        });

        let phys_idx = app.tools.blocks.last().unwrap().phys_idx;
        app.mouse.click_count = 1;
        handle_tool_block_click(&mut app, 0, phys_idx, 0);
        assert!(app.tools.popup.is_none());

        app.mouse.click_count = 2;
        app.mouse.last_click_tool = Some(0);
        handle_tool_block_click(&mut app, 0, phys_idx, TOOL_HEADER_ROWS);
        assert!(app.tools.popup.is_some());
    }

    #[test]
    fn double_click_tool_header_does_not_open_diff_popup() {
        let mut app = make_app();
        app.plan.visible = true;
        app.handle_agent_update(AgentUpdate::StepAdded(PlanStep::new(
            "run",
            "bash",
            "b1",
            HashMap::from([("command".to_string(), "echo hi".to_string())]),
        )));
        app.handle_agent_update(AgentUpdate::StepStarted {
            idx: 0,
            tool_id: "b1".into(),
            tool_name: "bash".into(),
            arg_summary: "echo hi".into(),
            arg_full: "echo hi".into(),
        });
        app.handle_agent_update(AgentUpdate::StepFinished {
            idx: 0,
            tool_id: "b1".into(),
            result: StepResult {
                tool: "bash".into(),
                arg_summary: "echo hi".into(),
                arg_full: Some("echo hi".into()),
                status: StepStatus::Success,
                message: "ok".into(),
                detail: Some("hi\n".into()),
                duration_us: Some(1),
                permission_label: None,
            },
        });

        let phys_idx = app.tools.blocks.last().unwrap().phys_idx;
        app.mouse.click_count = 2;
        app.mouse.last_click_tool = Some(0);
        handle_tool_block_click(&mut app, 0, phys_idx, 0);
        assert!(app.tools.popup.is_none());
        handle_tool_block_click(&mut app, 0, phys_idx, TOOL_HEADER_ROWS - 1);
        assert!(app.tools.popup.is_none());
    }

    #[test]
    fn divider_drag_updates_panel_split_ratio() {
        let mut app = make_app();
        app.panel_split_ratio = 0.20;

        begin_panel_resize(&mut app);
        assert!(app.mouse.is_resizing_panel);

        update_panel_resize(&mut app, 60, 0, 120);
        assert!(
            (app.panel_split_ratio - 0.50).abs() < 0.01,
            "expected ~0.50, got {}",
            app.panel_split_ratio
        );

        end_panel_resize(&mut app);
        assert!(!app.mouse.is_resizing_panel);
    }

    #[test]
    fn divider_drag_clamps_split_ratio() {
        let mut app = make_app();
        begin_panel_resize(&mut app);

        update_panel_resize(&mut app, 5, 0, 100);
        assert_eq!(app.panel_split_ratio, 0.10);

        update_panel_resize(&mut app, 95, 0, 100);
        assert_eq!(app.panel_split_ratio, 0.70);
    }

    #[test]
    fn triple_click_selects_single_line() {
        let mut app = make_app();
        app.add_system_message("pick this line".into());

        handle_log_triple_click(&mut app, 0, false);

        let expected = Some(LogSelection::full_message(0, "pick this line".len()));
        assert_eq!(app.mouse.log_selection, expected);
        assert!(app.mouse.dragging_log);
    }

    #[test]
    fn triple_click_inside_code_fence_selects_whole_block() {
        let mut app = make_app();
        app.add_system_message("```rust\nfn main() {}\n```".into());

        let inside_line = (0..20)
            .find(|&logical| app.find_code_block_containing_logical(logical).is_some())
            .expect("logical line inside fenced code block");
        let (cb_start, cb_end) = app
            .find_code_block_containing_logical(inside_line)
            .expect("code block range");

        handle_log_triple_click(&mut app, inside_line, true);

        let start_phys = app.visible_message_index(cb_start).unwrap();
        let end_phys = app.visible_message_index(cb_end).unwrap();
        let expected = Some(LogSelection::new(
            TextPosition::new(start_phys, 0),
            TextPosition::new(end_phys, app.raw_messages[end_phys].len()),
        ));
        assert_eq!(app.mouse.log_selection, expected);
        assert!(
            expected.as_ref().unwrap().end.phys_idx > expected.as_ref().unwrap().start.phys_idx
                || expected.as_ref().unwrap().end.byte_offset
                    > expected.as_ref().unwrap().start.byte_offset,
            "expected multi-line block selection"
        );
        assert!(app.mouse.dragging_log);
    }
}
