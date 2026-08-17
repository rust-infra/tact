//! Mouse handling extracted from the main event loop for testability.

use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};

use crate::widgets::state::{
    App, FocusedPanel, LogSelection, PopupTextHit, PopupTextSelection, TextPosition, VoicePhase,
    VoiceStartResult,
};

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct MousePanelHit {
    pub in_log: bool,
    pub in_task_panel: bool,
}

fn panel_hit(app: &App, column: u16, row: u16) -> MousePanelHit {
    MousePanelHit {
        in_log: point_in_rect(column, row, app.mouse.log_area),
        in_task_panel: point_in_rect(column, row, app.mouse.task_panel_area),
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
        MouseEventKind::Down(MouseButton::Left)
            if app.tools.popup.is_some() || app.thinking.popup.is_some() =>
        {
            handle_text_popup_mouse_down(app, mouse);
        }
        MouseEventKind::Down(MouseButton::Left) => {
            let hit = panel_hit(app, mouse.column, mouse.row);
            handle_mouse_down(app, mouse, hit);
        }
        MouseEventKind::Drag(MouseButton::Left)
            if app.tools.popup.is_some()
                || app.thinking.popup.is_some()
                || app.subagent_popup.is_some() =>
        {
            handle_text_popup_mouse_drag(app, mouse);
        }
        MouseEventKind::Drag(MouseButton::Left) => {
            let hit = panel_hit(app, mouse.column, mouse.row);
            handle_mouse_drag(app, mouse, hit);
        }
        MouseEventKind::Up(MouseButton::Left) => {
            app.mouse.popup_text_drag_origin = None;
            app.mouse.dragging_log = false;
        }
        _ => {}
    }
}

/// Handle mouse wheel up.
pub(crate) fn handle_mouse_scroll_up(app: &mut App, hit: MousePanelHit) {
    if app.has_overlay_popup() {
        app.overlay_popup_scroll_up();
    } else if hit.in_task_panel && sticky_scrollable(app) {
        app.mouse.in_task_panel = true;
        if app.task_panel.scroll > 0 {
            app.task_panel.scroll -= 1;
        }
    } else if hit.in_log {
        app.mouse.in_task_panel = false;
        app.scroll_log_up(crate::widgets::state::app::scroll::WHEEL_CELL_STEP);
    }
}

/// Handle mouse wheel down.
pub(crate) fn handle_mouse_scroll_down(app: &mut App, hit: MousePanelHit) {
    if app.has_overlay_popup() {
        app.overlay_popup_scroll_down();
    } else if hit.in_task_panel && sticky_scrollable(app) {
        app.mouse.in_task_panel = true;
        app.task_panel.scroll = app.task_panel.scroll.saturating_add(1);
    } else if hit.in_log {
        app.mouse.in_task_panel = false;
        app.scroll_log_down(crate::widgets::state::app::scroll::WHEEL_CELL_STEP);
    }
}

fn sticky_scrollable(app: &App) -> bool {
    crate::render::task_panel::sticky_host_visible(app) && app.task_panel.expanded
}

fn handle_mouse_down(app: &mut App, mouse: MouseEvent, hit: MousePanelHit) {
    if point_in_rect(mouse.column, mouse.row, app.voice.button_area) {
        handle_voice_button_click(app);
        return;
    }
    if point_in_rect(mouse.column, mouse.row, app.pending_cancel_btn_area) {
        // Clicking `[Cancel]` drops the queued messages only — the running
        // task keeps going (unlike `/cancel`, which stops it too).
        app.clear_pending_messages();
        return;
    }
    if app.close_overlay_on_outside_click(mouse.column, mouse.row) {
        return;
    }
    // Any click outside the Log panel (task strip, input bar, chrome) ends a
    // Log text selection — it is no longer what the user is looking at.
    if !hit.in_log {
        app.mouse.log_selection = None;
        app.mouse.dragging_log = false;
    }
    if hit.in_task_panel && crate::render::task_panel::sticky_host_visible(app) {
        app.task_panel.expanded = !app.task_panel.expanded;
        app.mouse.in_task_panel = app.task_panel.expanded;
        app.dirty = true;
        return;
    }
    if hit.in_log {
        app.mouse.in_task_panel = false;
        handle_log_click(app, mouse);
    }
}

fn handle_voice_button_click(app: &mut App) {
    match app.voice.phase {
        VoicePhase::Idle => match app.voice.try_start() {
            VoiceStartResult::MissingApiKey => {
                app.flash_msg = Some((
                    app.msgs().voice_missing_config.to_string(),
                    std::time::Instant::now(),
                ));
                app.dirty = true;
            }
            VoiceStartResult::Started => app.dirty = true,
            VoiceStartResult::Ignored => {}
        },
        VoicePhase::Recording { .. } => {
            app.voice.stop();
            app.dirty = true;
        }
        VoicePhase::Transcribing | VoicePhase::Disabled => {}
    }
}

fn handle_text_popup_mouse_down(app: &mut App, mouse: MouseEvent) {
    app.mouse.popup_text_drag_origin = None;
    let popup_area = if app.thinking.popup.is_some() {
        app.mouse.thinking_popup_area
    } else if app.subagent_popup.is_some() {
        app.mouse.subagent_popup_area
    } else {
        app.mouse.diff_popup_area
    };
    let inside_popup = point_in_rect(mouse.column, mouse.row, popup_area);
    app.close_overlay_on_outside_click(mouse.column, mouse.row);
    if !inside_popup || !point_in_rect(mouse.column, mouse.row, app.mouse.popup_text_body_area) {
        return;
    }

    let Some(origin) = popup_text_hit(app, mouse.column, mouse.row, false) else {
        return;
    };
    if let Some(popup) = app.thinking.popup.as_mut() {
        popup.selection = Some(PopupTextSelection::new(origin.start, origin.start));
        app.mouse.popup_text_drag_origin = Some(origin);
    } else if let Some(popup) = app.tools.popup.as_mut() {
        popup.selection = Some(PopupTextSelection::new(origin.start, origin.start));
        app.mouse.popup_text_drag_origin = Some(origin);
    } else if let Some(popup) = app.subagent_popup.as_mut() {
        popup.selection = Some(PopupTextSelection::new(origin.start, origin.start));
        app.mouse.popup_text_drag_origin = Some(origin);
    }
}

fn popup_text_hit(app: &App, column: u16, row: u16, clamp_vertical: bool) -> Option<PopupTextHit> {
    let first = app.mouse.popup_text_hit_rows.first()?;
    let last = app.mouse.popup_text_hit_rows.last()?;
    let body = app.mouse.popup_text_body_area;

    if row < body.y {
        return clamp_vertical.then(|| PopupTextHit::empty(first.line_start));
    }
    if row >= body.y.saturating_add(body.height) {
        return clamp_vertical.then(|| PopupTextHit::empty(last.line_end));
    }
    app.mouse
        .popup_text_hit_rows
        .iter()
        .find(|hit_row| hit_row.screen_y == row)
        .map(|hit_row| hit_row.hit(column))
}

fn handle_log_click(app: &mut App, mouse: MouseEvent) {
    app.focused_panel = FocusedPanel::Log;
    let visual_base = app.log_viewport_top();
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
        // Clicked in empty space below the last message: nothing here can
        // extend a selection, so clear it instead of keeping a stale one.
        app.mouse.log_selection = None;
        app.mouse.dragging_log = false;
        return;
    };

    // Whole-Markdown rows are cards: the MarkdownCell renderer draws no
    // selection overlay, so refuse to create an invisible selection here
    // (symmetric with rendering).
    if app.is_markdown_row(phys_idx) {
        app.mouse.log_selection = None;
        app.mouse.dragging_log = false;
        return;
    }

    // Task-stats `[copy]` button: copy this turn's log text. Only clicks that
    // land inside the button glyphs count — the rest of the row selects text.
    if let Some(item) = app.log_items.get(phys_idx)
        && crate::widgets::state::is_task_stats_line(&item.raw)
        && let Some((btn_start, btn_end)) =
            crate::widgets::state::find_task_stats_copy_button(&item.raw)
        && let Some((_, byte)) = app.byte_offset_from_log_position(line_idx, visual_row, col)
        && byte >= btn_start
        && byte < btn_end
    {
        app.copy_turn_ending_at_stats(phys_idx);
        app.mouse.log_selection = None;
        app.mouse.dragging_log = false;
        return;
    }

    let thinking_hit = app
        .find_thinking_at_logical(line_idx)
        .map(|(thinking_phys, _, _)| thinking_phys);
    if let Some(thinking_phys) = thinking_hit {
        if app.mouse.click_count == 1 {
            app.mouse.last_click_card = Some(thinking_phys);
            app.mouse.log_selection = None;
            app.mouse.dragging_log = false;
        } else if app.mouse.click_count == 2 && app.mouse.last_click_card == Some(thinking_phys) {
            app.open_thinking_popup(thinking_phys);
        } else if app.mouse.click_count >= 3 {
            handle_log_triple_click(app, line_idx, false);
        }
        return;
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
    let mermaid_hit = app.mermaid_blocks.iter().enumerate().find(|(_, b)| {
        app.phys_to_logical_fast(b.start_idx)
            .is_some_and(|si| line_idx >= si)
            && app
                .phys_to_logical_fast(b.end_idx)
                .is_some_and(|ei| line_idx < ei)
    });
    if let Some((mermaid_idx, _block)) = mermaid_hit {
        if app.mouse.click_count == 1 {
            app.mouse.last_click_mermaid = Some(mermaid_idx);
            app.mouse.log_selection = None;
            app.mouse.dragging_log = false;
        } else if app.mouse.click_count == 2 && app.mouse.last_click_mermaid == Some(mermaid_idx) {
            app.open_mermaid_popup(mermaid_idx);
        } else if app.mouse.click_count >= 3 {
            handle_log_triple_click(app, line_idx, false);
        }
        return;
    }

    app.mouse.last_click_mermaid = None;
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
    if app.mouse.dragging_log && hit.in_log {
        let visual_base = app.log_viewport_top();
        let visual_row = visual_base + mouse.row.saturating_sub(app.mouse.log_area.y + 1) as usize;
        let line_idx = app.logical_from_visual(visual_row);
        let col = mouse.column.saturating_sub(app.mouse.log_area.x + 1) as usize;
        if line_idx < app.total_log_lines()
            && let Some((phys, byte)) = app.byte_offset_from_log_position(line_idx, visual_row, col)
        {
            // Markdown cards carry no selection overlay: stop the selection at
            // the last text row instead of extending invisibly into one.
            if app.is_markdown_row(phys) {
                return;
            }
            if let Some(ref mut sel) = app.mouse.log_selection {
                sel.end = TextPosition::new(phys, byte);
            }
        }
    }
}

fn handle_text_popup_mouse_drag(app: &mut App, mouse: MouseEvent) {
    let Some(origin) = app.mouse.popup_text_drag_origin else {
        return;
    };
    let Some(current) = popup_text_hit(app, mouse.column, mouse.row, true) else {
        return;
    };
    let selection = if current.end > origin.start {
        PopupTextSelection::new(origin.start, current.end)
    } else {
        PopupTextSelection::new(origin.end, current.start)
    };
    if let Some(popup) = app.thinking.popup.as_mut() {
        popup.selection = Some(selection);
    } else if let Some(popup) = app.tools.popup.as_mut() {
        popup.selection = Some(selection);
    } else if let Some(popup) = app.subagent_popup.as_mut() {
        popup.selection = Some(selection);
    }
}

/// Triple-click on a log line selects the line (or whole code block when enabled).
pub(crate) fn handle_log_triple_click(app: &mut App, line_idx: usize, expand_code_blocks: bool) {
    if expand_code_blocks
        && let Some((cb_start, cb_end)) = app.find_code_block_containing_logical(line_idx)
    {
        if let Some(start_phys) = app.visible_message_index(cb_start) {
            let end_phys = app.visible_message_index(cb_end).unwrap_or(start_phys);
            let end_len = app.log_items[end_phys].raw.len();
            app.mouse.log_selection = Some(LogSelection::new(
                TextPosition::new(start_phys, 0),
                TextPosition::new(end_phys, end_len),
            ));
        }
        app.mouse.dragging_log = true;
        return;
    }
    if let Some(phys) = app.visible_message_index(line_idx) {
        // Markdown cards never show a selection overlay; triple-click cannot
        // select them either.
        if app.is_markdown_row(phys) {
            app.mouse.log_selection = None;
            app.mouse.dragging_log = false;
            return;
        }
        let len = app.log_items[phys].raw.len();
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
        // Subagent tools open a dedicated live/markdown popup.
        let is_subagent = app
            .tools
            .active
            .iter()
            .find(|a| a.phys_idx == phys_idx)
            .map(|a| a.output.tool_name.as_str() == "spawn_subagent")
            .or_else(|| {
                app.tools
                    .blocks
                    .iter()
                    .find(|b| b.phys_idx == phys_idx)
                    .map(|b| b.output.tool_name.as_str() == "spawn_subagent")
            })
            .unwrap_or(false);
        if is_subagent {
            app.open_subagent_popup(phys_idx);
        } else {
            app.open_diff_popup_at_row(phys_idx, relative_row);
        }
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
    use std::collections::HashMap;

    use crossterm::event::KeyModifiers;
    use ratatui::layout::Rect;
    use tact_protocol::{AgentUpdate, PlanStep, StepResult, StepStatus, ToolPresentationInfo};

    use super::*;
    use crate::{
        render::test_harness::make_app,
        widgets::{
            state::{
                DiffPopup, LogSelection, PopupHitRow, PopupTextHit, PopupTextSelection,
                ThinkingPopup,
            },
            tool_widget::TOOL_HEADER_ROWS,
        },
    };

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

    #[test]
    fn click_task_panel_toggles_expanded() {
        use tact_protocol::{TaskSnapshot, TaskStatusSnapshot};

        let mut app = make_app();
        app.task_panel.visible = true;
        app.task_panel.expanded = false;
        app.task_panel.expanded = false;
        app.task_panel.snapshot = vec![TaskSnapshot {
            id: 1,
            subject: "Fix auth".into(),
            status: TaskStatusSnapshot::InProgress,
            session_id: String::new(),
            owner: String::new(),
            blocks: Vec::new(),
            blocked_by: Vec::new(),
            created_at: None,
            started_at: None,
            completed_at: None,
        }];
        app.mouse.task_panel_area = Rect::new(0, 10, 40, 1);

        // Click outside tab strip (x>=18) toggles expand when both tabs aren't shown.
        handle_mouse_event(&mut app, mouse_down(20, 10));
        assert!(app.task_panel.expanded);
        assert!(app.task_panel.expanded);

        handle_mouse_event(&mut app, mouse_down(20, 10));
        assert!(!app.task_panel.expanded);
        assert!(!app.task_panel.expanded);
    }

    #[test]
    fn voice_button_click_starts_and_stops_recording() {
        use tact::voice::VoiceCommand;
        use tokio::sync::mpsc::unbounded_channel;

        let mut app = make_app();
        let (cmd_tx, mut cmd_rx) = unbounded_channel();
        let (_event_tx, event_rx) = unbounded_channel();
        app.voice = crate::widgets::state::VoiceState::enabled(
            tact::voice::VoiceWorkerHandle::stub_for_test(cmd_tx, event_rx),
            false,
        );
        let title_row = 0u16;
        let button_x = 70u16;
        app.voice
            .set_button_area(Rect::new(button_x, title_row, 8, 1));

        handle_mouse_event(&mut app, mouse_down(button_x, title_row));
        assert!(matches!(cmd_rx.try_recv(), Ok(VoiceCommand::Start)));

        app.voice
            .apply_event(tact::voice::VoiceEvent::RecordingStarted);
        handle_mouse_event(&mut app, mouse_down(button_x, title_row));
        assert!(matches!(cmd_rx.try_recv(), Ok(VoiceCommand::Stop)));

        handle_mouse_event(&mut app, mouse_down(1, title_row));
        assert!(
            cmd_rx.try_recv().is_err(),
            "outside click should not send commands"
        );
    }

    #[test]
    fn pending_cancel_button_drops_queue_without_touching_task() {
        use tact_protocol::UserCommand;
        use tokio::sync::mpsc::unbounded_channel;

        let (_agent_tx, agent_rx) = unbounded_channel::<tact_protocol::AgentUpdate>();
        let (user_cmd_tx, mut user_cmd_rx) = unbounded_channel::<UserCommand>();
        let (plugin_tx, _plugin_request_rx) = unbounded_channel();
        let (_plugin_event_tx, plugin_rx) = unbounded_channel();
        let (history_tx, _history_rx) = unbounded_channel();
        let mut app = App::new(
            agent_rx,
            None,
            plugin_rx,
            plugin_tx,
            user_cmd_tx,
            std::path::PathBuf::from("."),
            Vec::new(),
            "test-session".to_string(),
            history_tx,
            "retro".to_string(),
            String::new(),
            Vec::new(),
        );
        app.status = crate::widgets::state::Status::Executing {
            current_step: 0,
            total: 1,
        };
        app.queue_pending_message("regret".into(), "regret".into());
        app.set_cancel_button_area(Rect::new(70, 0, 10, 1));

        // Click [Cancel]: the queue is dropped, the running task is untouched.
        handle_mouse_event(&mut app, mouse_down(71, 0));

        assert!(
            app.pending_messages.is_empty(),
            "[Cancel] must drop the queued messages"
        );
        assert!(
            matches!(app.status, crate::widgets::state::Status::Executing { .. }),
            "[Cancel] must not change the task status"
        );
        assert!(
            user_cmd_rx.try_recv().is_err(),
            "[Cancel] must not dispatch Cancel/SubmitTask"
        );
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
        app.mouse.popup_text_body_area = Rect::new(6, 6, 22, 5);
        app.mouse.popup_text_hit_rows = vec![
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

    fn app_with_selectable_thinking_popup() -> App {
        let mut app = make_app();
        app.mouse.log_area = Rect::new(0, 0, 40, 20);
        app.mouse.thinking_popup_area = Rect::new(5, 5, 24, 8);
        app.mouse.popup_text_body_area = Rect::new(6, 6, 22, 5);
        app.mouse.popup_text_hit_rows = vec![
            popup_hit_row(6, 6, 0, "alpha"),
            popup_hit_row(7, 6, 6, "omega"),
        ];
        app.thinking.popup = Some(ThinkingPopup {
            phys_idx: 0,
            title: "thinking".into(),
            scroll: 0,
            selection: None,
            selection_text: "alpha\nomega".into(),
        });
        app
    }

    #[test]
    fn thinking_popup_mouse_drag_selects_visible_text() {
        let mut app = app_with_selectable_thinking_popup();

        handle_mouse_event(&mut app, mouse_down(6, 6));
        handle_mouse_event(&mut app, mouse_drag(10, 6));

        let popup = app.thinking.popup.as_ref().expect("thinking popup");
        assert_eq!(popup.copy_content("raw reasoning"), "alpha");
    }

    #[test]
    fn thinking_popup_mouse_scroll_preserves_selection() {
        let mut app = app_with_selectable_thinking_popup();
        handle_mouse_event(&mut app, mouse_down(6, 6));
        handle_mouse_event(&mut app, mouse_drag(10, 6));
        let selection = app.thinking.popup.as_ref().expect("popup").selection;

        handle_mouse_event(&mut app, mouse_event(MouseEventKind::ScrollDown, 10, 6));

        let popup = app.thinking.popup.as_ref().expect("thinking popup");
        assert_eq!(popup.scroll, 1);
        assert_eq!(popup.selection, selection);
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
            app.mouse.popup_text_drag_origin,
            Some(PopupTextHit::empty(0))
        );
    }

    #[test]
    fn popup_chrome_mouse_down_clears_stale_drag_without_changing_selection() {
        for (column, row) in [(5, 6), (10, 5), (28, 6)] {
            let mut app = app_with_selectable_tool_popup();
            let selection = Some(PopupTextSelection::new(1, 4));
            app.tools.popup.as_mut().unwrap().selection = selection;
            app.mouse.popup_text_drag_origin = Some(PopupTextHit::new(1, 2));

            handle_mouse_event(&mut app, mouse_down(column, row));
            handle_mouse_event(&mut app, mouse_drag(14, 7));

            assert_eq!(app.tools.popup.as_ref().unwrap().selection, selection);
            assert!(app.mouse.popup_text_drag_origin.is_none());
        }
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
        for i in 0..10 {
            app.add_system_message(format!("row-{i}"));
        }
        let _ = crate::render::test_harness::render_log_panel_text(&mut app, 60, 4);
        app.log_scroll.visual_top = 2;

        handle_mouse_scroll_up(
            &mut app,
            MousePanelHit {
                in_log: true,
                in_task_panel: false,
            },
        );

        assert_eq!(app.log_scroll.offset, 1);
    }

    #[test]
    fn scroll_down_in_log_increments_offset() {
        let mut app = make_app();
        for i in 0..10 {
            app.add_system_message(format!("row-{i}"));
        }
        let _ = crate::render::test_harness::render_log_panel_text(&mut app, 60, 4);
        app.scroll_log_to_top();

        handle_mouse_scroll_down(
            &mut app,
            MousePanelHit {
                in_log: true,
                in_task_panel: false,
            },
        );

        assert_eq!(app.log_scroll.offset, 1);
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
            presentation: ToolPresentationInfo::generic("bash"),
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
                presentation: ToolPresentationInfo::generic("bash"),
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
            presentation: ToolPresentationInfo::generic("bash"),
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
                presentation: ToolPresentationInfo::generic("bash"),
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
        let (lines, raw) =
            crate::render::render_md::render_markdown_tui("```rust\nfn main() {}\n```", &app.theme);
        app.extend_msgs(
            lines,
            raw,
            crate::widgets::state::LogItemKind::SystemMarkdown,
        );

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
            TextPosition::new(end_phys, app.log_items[end_phys].len()),
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

    /// Set up a one-row log panel at (0,0) with a 40x10 click surface so
    /// `handle_mouse_event` clicks can resolve positions.
    fn app_with_clickable_log() -> App {
        let mut app = make_app();
        app.mouse.log_area = Rect::new(0, 0, 40, 10);
        app
    }

    #[test]
    fn double_click_selects_cjk_run() {
        let mut app = app_with_clickable_log();
        app.add_system_message("你好世界 hello".into());
        app.log_scroll.visual_start = vec![0, 1];

        handle_mouse_event(&mut app, mouse_down(1, 1));
        handle_mouse_event(&mut app, mouse_down(1, 1));

        let expected = Some(LogSelection::span(0, 0, "你好世界".len()));
        assert_eq!(app.mouse.log_selection, expected);
    }

    #[test]
    fn click_below_last_message_clears_selection() {
        let mut app = app_with_clickable_log();
        app.add_system_message("only row".into());
        app.log_scroll.visual_start = vec![0, 1];
        app.mouse.log_selection = Some(LogSelection::full_message(0, "only row".len()));

        // Row 5 is below the only message row.
        handle_mouse_event(&mut app, mouse_down(1, 5));

        assert!(app.mouse.log_selection.is_none());
        assert!(!app.mouse.dragging_log);
    }

    #[test]
    fn click_on_markdown_row_does_not_create_invisible_selection() {
        let mut app = app_with_clickable_log();
        app.append_markdown("# Title\n");
        app.log_scroll.visual_start = vec![0, 1];

        handle_mouse_event(&mut app, mouse_down(1, 1));

        assert!(app.mouse.log_selection.is_none());
        assert!(!app.mouse.dragging_log);
    }

    #[test]
    fn drag_into_markdown_row_does_not_extend_selection() {
        let mut app = app_with_clickable_log();
        app.add_system_message("select me".into());
        app.append_markdown("# Card\n");
        app.log_scroll.visual_start = vec![0, 1, 2];

        handle_mouse_event(&mut app, mouse_down(1, 1));
        handle_mouse_event(&mut app, mouse_drag(1, 2));

        let sel = app.mouse.log_selection.expect("selection started");
        assert_eq!(sel.end.phys_idx, 0, "selection must stop at the text row");
    }

    #[test]
    fn click_outside_log_clears_selection() {
        let mut app = app_with_clickable_log();
        app.add_system_message("row".into());
        app.mouse.log_selection = Some(LogSelection::full_message(0, 3));

        handle_mouse_event(&mut app, mouse_down(1, 20));

        assert!(app.mouse.log_selection.is_none());
    }

    #[test]
    fn task_stats_copy_only_triggers_inside_button() {
        let mut app = app_with_clickable_log();
        app.add_system_message("first answer".into());
        app.add_system_message("second answer".into());
        app.last_prompt_elapsed_secs = Some(5);
        app.add_task_stats_block();
        app.log_scroll.visual_start = vec![0, 1, 2, 3];

        // Stats row is logical 2 (visual row 2 → mouse row 3). Raw row is
        // `[copy]  Task stats:⏱ 00:05`; column 3 maps to byte 2, inside the
        // button glyphs (bytes 0..6). A successful copy appends a notice row.
        let before = app.log_items.len();
        handle_mouse_event(&mut app, mouse_down(3, 3));
        assert!(
            app.log_items.len() > before,
            "clicking the [copy] button should copy this turn"
        );
        let last = app.log_items.last().expect("copy notice");
        assert!(
            last.contains("已复制") || last.contains("Copied"),
            "expected a copy notice, got: {last}"
        );
    }

    #[test]
    fn task_stats_body_click_does_not_copy() {
        let mut app = app_with_clickable_log();
        app.add_system_message("first answer".into());
        app.add_system_message("second answer".into());
        app.last_prompt_elapsed_secs = Some(5);
        app.add_task_stats_block();
        app.log_scroll.visual_start = vec![0, 1, 2, 3];

        // Column 15 maps into the "Task stats:" body (byte 11) — outside the
        // button range (0..6), so no copy notice may be appended.
        let before = app.log_items.len();
        handle_mouse_event(&mut app, mouse_down(15, 3));
        assert_eq!(
            app.log_items.len(),
            before,
            "clicking the stats body must not copy"
        );
    }
}
