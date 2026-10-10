use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    widgets::Borders,
};

use crate::widgets::state::{App, SurfaceId};

/// Main content area layout, switching between history, help, or the Log panel
/// based on current display state. The Log panel is always single-column and
/// full-width — there is no side panel or draggable divider.
///
/// When persistent tasks are visible, the main area is split: scrollable Log on
/// top, sticky task strip below (outer split — Log internals unchanged).
pub(crate) fn render_main_area(frame: &mut Frame, area: Rect, app: &mut App) {
    if app.show_history {
        super::popups::history::render_history_panel(frame, area, app);
        return;
    }
    if app.show_help {
        super::popups::help::render_help_panel(frame, area, app);
        return;
    }

    let sticky_h = if super::task_panel::sticky_host_visible(app) {
        let content = super::task_panel::sticky_host_content_height(app) as u16;
        // Content rows + bottom border so sticky continues the Log box.
        content
            .saturating_add(super::task_panel::STICKY_BORDER_ROWS)
            .min(area.height.saturating_sub(2))
    } else {
        0
    };

    let pill_area = if sticky_h == 0 {
        app.mouse.clear_area(SurfaceId::TaskPanel);
        app.mouse.sticky_tab_areas.clear();
        app.mouse.set_area(SurfaceId::Log, area);
        app.log_scroll.height = area.height.saturating_sub(2);
        super::log::render_log_panel(frame, area, app);
        area
    } else {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(sticky_h)])
            .split(area);
        app.mouse.set_area(SurfaceId::Log, chunks[0]);
        // Log omits bottom border; sticky draws LEFT|RIGHT|BOTTOM to close the box.
        super::log::render_log_panel_with_borders(
            frame,
            chunks[0],
            app,
            Borders::TOP | Borders::LEFT | Borders::RIGHT,
        );
        super::task_panel::render_task_panel(frame, chunks[1], app);
        chunks[1]
    };

    // Scroll-back pill: drawn on the bottom border row of whichever panel
    // closes the Log box (Log itself or the sticky host strip).
    let ctx = app.render_ctx();
    let pill_hit = agent_tui_kit::render::scroll_pill::render_scroll_pill(frame, pill_area, &ctx);
    app.set_scroll_back_area(pill_hit);

    if app.thinking_mut().popup.is_some() {
        super::popups::thinking_popup::render_thinking_popup(frame, area, app);
    }
    if app.tools_mut().popup.is_some() {
        super::popups::diff_popup::render_diff_popup(frame, area, app);
    }
    if app.system_prompt_popup.is_some() {
        super::popups::system_prompt_popup::render_system_prompt_popup(frame, area, app);
    }
    if app.code_popup.is_some() {
        super::popups::code_popup::render_code_popup(frame, area, app);
    }
    if app.mermaid_popup.is_some() {
        super::popups::mermaid_popup::render_mermaid_popup(frame, area, app);
    }
    if app.task_dag_popup.is_some() {
        super::popups::task_dag_popup::render_task_dag_popup(frame, area, app);
    }
    if app.has_subagent_popup() {
        super::popups::subagent_popup::render_subagent_popup(frame, area, app);
    }
}

#[cfg(test)]
mod render_tests {
    use std::collections::HashMap;

    use ratatui::{Terminal, backend::TestBackend};
    use tact_protocol::{AgentErrorKind, AgentUpdate, PlanStep};

    use super::super::test_harness::{
        buffer_contains, buffer_text, make_app, render_app_text, render_main_area_terminal,
    };
    use crate::test_fixtures::StepCall;
    use crate::widgets::state::Status;

    #[test]
    fn main_area_renders_tool_and_stream_content() {
        let mut app = make_app();

        app.handle_agent_update(AgentUpdate::StepAdded(PlanStep::new(
            "read file",
            "read_file",
            "tool_read_1",
            HashMap::from([("path".to_string(), "main.rs".to_string())]),
        )));
        app.handle_agent_update(StepCall::new(0, "tool_read_1", "read_file", "main.rs").started());
        app.handle_agent_update(
            StepCall::new(0, "tool_read_1", "read_file", "main.rs")
                .no_arg_full()
                .detail("fn main() {}")
                .duration_us(1000)
                .finished(),
        );
        app.handle_agent_update(AgentUpdate::StreamChunk("Hello from mock.".into()));
        app.handle_agent_update(AgentUpdate::TaskComplete("Hello from mock.".into()));

        assert!(matches!(app.status, Status::Done));

        let text = render_app_text(&mut app, 100, 30);
        assert!(
            text.contains("read_file") || text.contains("main.rs"),
            "log should show tool activity, buffer:\n{text}"
        );
        assert!(
            text.contains("Hello from mock"),
            "stream chunk should be visible, buffer:\n{text}"
        );
    }

    #[test]
    fn main_area_renders_after_fatal_error() {
        let mut app = make_app();
        app.handle_agent_update(AgentUpdate::Error(AgentErrorKind::Other(
            "provider timeout".into(),
        )));

        assert!(matches!(app.status, Status::Idle));

        let backend = ratatui::backend::TestBackend::new(100, 24);
        let mut terminal = ratatui::Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| super::render_main_area(frame, frame.area(), &mut app))
            .expect("draw");

        assert!(
            buffer_contains(terminal.backend().buffer(), "provider timeout")
                || app
                    .log
                    .items
                    .iter()
                    .any(|item| item.raw.contains("provider timeout")),
            "error should be visible in log or buffer"
        );
    }

    // ── Scroll-back pill (C-tier overlay) ──

    fn draw_main(app: &mut crate::widgets::state::App, width: u16, height: u16) -> String {
        let terminal = render_main_area_terminal(app, width, height);
        buffer_text(terminal.backend().buffer())
    }

    #[test]
    fn pill_not_drawn_when_following() {
        let mut app = make_app();
        let text = draw_main(&mut app, 80, 10);
        assert!(
            !text.contains("Back to bottom"),
            "nothing to say while following the tail:\n{text}"
        );
        assert!(
            app.scroll_back_area.is_empty(),
            "no pill drawn, no click target left behind"
        );
    }

    #[test]
    fn pill_drawn_when_scrolled_away() {
        let mut app = make_app();
        app.log_scroll.follow = false;
        app.log_scroll.unseen = true;

        let text = draw_main(&mut app, 80, 10);
        assert!(
            text.contains("New activity") && text.contains("Back to bottom"),
            "the badge and the action both belong on the pill:\n{text}"
        );
        assert!(
            !app.scroll_back_area.is_empty(),
            "a shown pill must expose a hit area"
        );
    }

    #[test]
    fn pill_retires_when_back_at_bottom() {
        let mut app = make_app();
        app.log_scroll.follow = false;
        app.log_scroll.unseen = true;

        let _ = draw_main(&mut app, 80, 10);
        assert!(!app.scroll_back_area.is_empty());

        app.scroll_log_to_bottom();
        let text = draw_main(&mut app, 80, 10);
        assert!(
            !text.contains("Back to bottom"),
            "returning to the tail retires the pill:\n{text}"
        );
        assert!(app.scroll_back_area.is_empty());
    }

    #[test]
    fn pill_drops_badge_and_separator_without_unseen() {
        let mut app = make_app();
        app.log_scroll.follow = false;

        let text = draw_main(&mut app, 80, 10);
        assert!(text.contains("Back to bottom"), "action stays:\n{text}");
        assert!(
            !text.contains("New activity"),
            "no badge when nothing unseen:\n{text}"
        );
        assert!(
            !text.contains(" · ↓"),
            "the separator goes with the badge:\n{text}"
        );
    }

    #[test]
    fn pill_is_right_docked() {
        let mut app = make_app();
        app.log_scroll.follow = false;

        let backend = TestBackend::new(80, 10);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| super::render_main_area(frame, frame.area(), &mut app))
            .unwrap();

        let area = app.scroll_back_area;
        assert!(!area.is_empty(), "a shown pill must expose a hit area");
        // Pill sits on the bottom border row of the main area.
        assert_eq!(area.y, 9, "bottom border row");
        assert_eq!(area.height, 1);
        // Right edge leaves a 1-column margin inside the border.
        assert_eq!(area.right(), 79, "right margin inside border");
    }

    /// The band fills everything between the two caps; the caps themselves keep
    /// the panel background, which is what lets their rounded shape show.
    #[test]
    fn pill_band_fills_between_the_caps() {
        let mut app = make_app();
        app.log_scroll.follow = false;

        let backend = TestBackend::new(80, 10);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| super::render_main_area(frame, frame.area(), &mut app))
            .unwrap();

        let rect = app.scroll_back_area;
        assert!(!rect.is_empty());
        let buf = terminal.backend().buffer();
        let band = app.theme.status_bar_bg;

        for x in [rect.left(), rect.right() - 1] {
            assert_eq!(
                buf[(x, rect.y)].bg,
                app.theme.bg,
                "cap cell {x} must keep the panel background"
            );
            assert_eq!(
                buf[(x, rect.y)].fg,
                band,
                "cap cell {x} must draw its rounded shape in the band colour"
            );
        }
        for x in (rect.left() + 1)..(rect.right() - 1) {
            assert_eq!(
                buf[(x, rect.y)].bg,
                band,
                "interior cell {x} must carry band bg"
            );
        }
    }

    /// The pill's hit area must survive the **whole** frame. The input box
    /// renders after the Log panel and used to write this field, which silently
    /// un-clicked the pill; only a full-UI draw can see that ordering.
    #[test]
    fn the_pill_stays_clickable_after_the_whole_frame_draws() {
        let mut app = make_app();
        app.log_scroll.follow = false;
        app.log_scroll.unseen = true;

        // `render_app_text` draws status + main + input + bottom, in that order.
        let _ = render_app_text(&mut app, 80, 12);

        let area = app.scroll_back_area;
        assert!(
            !area.is_empty(),
            "the input box must not clobber the pill's hit area"
        );
        assert_eq!(area.height, 1);
        assert_eq!(area.right(), 79, "still docked inside the right border");
        // Main area is rows 1..7 here, so the Log's bottom border row is 6 —
        // well above the input box (row 7).
        assert_eq!(area.y, 6, "the pill lives on the Log's bottom border row");
    }

    /// The ends are rounded, not square: the first and last cells are the
    /// Powerline half-circle glyphs.
    #[test]
    fn pill_caps_the_band_with_rounded_ends() {
        use agent_tui_kit::render::scroll_pill::{CAP_LEFT, CAP_RIGHT};

        let mut app = make_app();
        app.log_scroll.follow = false;

        let backend = TestBackend::new(80, 10);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| super::render_main_area(frame, frame.area(), &mut app))
            .unwrap();

        let rect = app.scroll_back_area;
        let buf = terminal.backend().buffer();
        assert_eq!(
            buf[(rect.left(), rect.y)].symbol().chars().next(),
            Some(CAP_LEFT),
            "left end must be the rounded cap"
        );
        assert_eq!(
            buf[(rect.right() - 1, rect.y)].symbol().chars().next(),
            Some(CAP_RIGHT),
            "right end must be the rounded cap"
        );
    }

    #[test]
    fn pill_hidden_when_too_narrow() {
        let mut app = make_app();
        app.log_scroll.follow = false;

        let text = draw_main(&mut app, 20, 10);
        assert!(
            !text.contains("Back to bottom"),
            "below minimum width the pill hides:\n{text}"
        );
        assert!(app.scroll_back_area.is_empty());
    }
}
