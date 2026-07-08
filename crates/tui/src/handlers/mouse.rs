//! Mouse scroll handling extracted from the main event loop for testability.

use crate::widgets::state::App;

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct MousePanelHit {
    pub in_log: bool,
    pub in_plan: bool,
}

/// Handle mouse wheel up (mirrors `lib.rs` scroll-up branch).
pub(crate) fn handle_mouse_scroll_up(app: &mut App, hit: MousePanelHit) {
    if app.thinking.popup.is_some() {
        app.thinking_popup_scroll_up();
    } else if app.tools.popup.is_some() {
        app.diff_popup_scroll_up();
    } else if app.code_popup.is_some() {
        app.code_popup_scroll_up();
    } else if hit.in_log && app.log_scroll.offset > 0 {
        app.log_scroll.offset -= 1;
    } else if hit.in_plan && app.plan.selected > 0 {
        app.plan.selected -= 1;
        app.plan.list_state.select(Some(app.plan.selected));
    }
}

/// Handle mouse wheel down (mirrors `lib.rs` scroll-down branch).
pub(crate) fn handle_mouse_scroll_down(app: &mut App, hit: MousePanelHit) {
    if app.thinking.popup.is_some() {
        app.thinking_popup_scroll_down();
    } else if app.tools.popup.is_some() {
        app.diff_popup_scroll_down();
    } else if app.code_popup.is_some() {
        app.code_popup_scroll_down();
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
    app.mouse.log_word_selection = None;
    if expand_code_blocks {
        if let Some((cb_start, cb_end)) = app.find_code_block_containing_logical(line_idx) {
            app.mouse.log_selection = Some((cb_start, cb_end));
            app.mouse.dragging_log = true;
            return;
        }
    }
    app.mouse.log_selection = Some((line_idx, line_idx));
    app.mouse.dragging_log = true;
}

/// Double-click on a tool block opens its detail popup.
pub(crate) fn handle_tool_block_click(app: &mut App, tool_idx: usize, phys_idx: usize) {
    if app.mouse.click_count == 2 && app.mouse.last_click_tool == Some(tool_idx) {
        app.open_diff_popup(phys_idx);
        return;
    }
    if app.mouse.click_count == 1 {
        app.mouse.last_click_tool = Some(tool_idx);
        app.mouse.log_word_selection = None;
        app.mouse.log_selection = None;
        app.mouse.dragging_log = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::test_harness::make_app;
    use crate::widgets::state::DiffPopup;
    use std::collections::HashMap;
    use tact_protocol::{AgentUpdate, PlanStep, StepResult, StepStatus};

    #[test]
    fn scroll_up_in_log_decrements_offset() {
        let mut app = make_app();
        app.log_scroll.offset = 3;

        handle_mouse_scroll_up(
            &mut app,
            MousePanelHit {
                in_log: true,
                in_plan: false,
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
            inline_content: Some("line\n".into()),
            lang: String::new(),
            use_diff_gutter: false,
            scroll: 0,
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
        app.handle_agent_update(AgentUpdate::StepStarted(
            0,
            "b1".into(),
            "bash".into(),
            "echo hi".into(),
        ));
        app.handle_agent_update(AgentUpdate::StepFinished(
            0,
            "b1".into(),
            StepResult {
                tool: "bash".into(),
                arg_summary: "echo hi".into(),
                arg_full: Some("echo hi".into()),
                status: StepStatus::Success,
                message: "ok".into(),
                detail: Some("hi\n".into()),
                duration_us: Some(1),
                permission_label: None,
            },
        ));

        let phys_idx = app.tools.blocks.last().unwrap().phys_idx;
        app.mouse.click_count = 1;
        handle_tool_block_click(&mut app, 0, phys_idx);
        assert!(app.tools.popup.is_none());

        app.mouse.click_count = 2;
        app.mouse.last_click_tool = Some(0);
        handle_tool_block_click(&mut app, 0, phys_idx);
        assert!(app.tools.popup.is_some());
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

        assert_eq!(app.mouse.log_selection, Some((0, 0)));
        assert!(app.mouse.dragging_log);
        assert!(app.mouse.log_word_selection.is_none());
    }

    #[test]
    fn triple_click_inside_code_fence_selects_whole_block() {
        let mut app = make_app();
        app.add_system_message("```rust\nfn main() {}\n```".into());

        let inside_line = (0..20)
            .find(|&logical| app.find_code_block_containing_logical(logical).is_some())
            .expect("logical line inside fenced code block");
        let expected = app
            .find_code_block_containing_logical(inside_line)
            .expect("code block range");

        handle_log_triple_click(&mut app, inside_line, true);

        assert_eq!(app.mouse.log_selection, Some(expected));
        assert!(
            expected.1 > expected.0,
            "expected multi-line block selection"
        );
        assert!(app.mouse.dragging_log);
    }
}
