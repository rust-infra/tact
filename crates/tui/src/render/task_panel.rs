//! Sticky host strip under the Log — app-layer wrapper.
//!
//! The kit renders a two-domain host (`Tasks | Subagent`); this wrapper
//! records the host's screen area and per-frame tab hit rectangles on
//! `MouseState` (mirrors how `render_log_panel` stores cancel-button areas),
//! then calls the pure renderer.

use ratatui::{Frame, layout::Rect};

use crate::widgets::state::App;

pub(crate) use agent_tui_kit::render::sticky_host::STICKY_BORDER_ROWS;

/// True when either sticky domain (tasks or subagent overview) is visible.
pub(crate) fn sticky_host_visible(app: &App) -> bool {
    app.task_panel().visible || app.subagent_panel().visible
}

/// Content rows inside the sticky host (excluding border). Collapsed = 1;
/// expanded = 2 + the active visible domain's body rows.
pub(crate) fn sticky_host_content_height(app: &App) -> usize {
    let ctx = app.render_ctx();
    agent_tui_kit::render::sticky_host::sticky_host_content_height(&ctx)
}

/// Which domain is currently expanded / scrolled by wheel + jk keys.
pub(crate) fn active_sticky_tab(app: &App) -> agent_tui_kit::state::StickyTab {
    let ctx = app.render_ctx();
    agent_tui_kit::render::sticky_host::active_visible_tab(&ctx)
}

pub(crate) fn sticky_tab_expanded(app: &App, tab: agent_tui_kit::state::StickyTab) -> bool {
    match tab {
        agent_tui_kit::state::StickyTab::Tasks => app.task_panel().expanded,
        agent_tui_kit::state::StickyTab::Subagent => app.subagent_panel().expanded,
    }
}

pub(crate) fn render_task_panel(frame: &mut Frame, area: Rect, app: &mut App) {
    app.mouse.task_panel_area = area;
    let ctx = app.render_ctx();
    let hit = agent_tui_kit::render::sticky_host::render_sticky_host(frame, area, &ctx);
    app.mouse.sticky_tab_areas.clear();
    app.mouse.sticky_tab_areas.extend(hit.tab_areas);
}

#[cfg(test)]
mod sticky_host_tests {
    use tact_protocol::{
        SubagentRunSnapshot, SubagentStatusSnapshot, TaskSnapshot, TaskStatusSnapshot,
    };

    use super::super::test_harness::{make_app, render_main_area_text};

    fn seed_tasks(app: &mut crate::widgets::state::App, subject: &str) {
        app.task_panel_mut().apply_snapshot(vec![TaskSnapshot {
            id: 1,
            subject: subject.into(),
            status: TaskStatusSnapshot::InProgress,
            ..Default::default()
        }]);
    }

    fn seed_subagent(app: &mut crate::widgets::state::App, summary: &str) {
        app.subagent_panel_mut()
            .apply_snapshot(vec![SubagentRunSnapshot {
                child_id: "0123456789abcdef".into(),
                status: SubagentStatusSnapshot::Running,
                summary_first: summary.into(),
                started_at: Some(1),
                finished_at: None,
            }]);
    }

    #[test]
    fn tasks_only_sticky_keeps_existing_single_domain_shape() {
        let mut app = make_app();
        seed_tasks(&mut app, "buy bitcoin");
        app.task_panel_mut().expanded = true;

        let text = render_main_area_text(&mut app, 80, 20);
        let lines: Vec<&str> = text.lines().collect();
        let tabs = lines
            .iter()
            .position(|l| l.contains("[Tasks]"))
            .expect("tab row missing, got:\n{text}");
        let pending = lines
            .iter()
            .position(|l| l.contains("In Progress"))
            .expect("in-progress group header missing");

        assert_eq!(
            pending - tabs,
            2,
            "expected one separator row between tabs and body, got:\n{text}"
        );
        assert!(
            lines[tabs + 1].trim().chars().all(|c| c == '─' || c == '│'),
            "separator row should be a hairline rule, got: {:?}",
            lines[tabs + 1]
        );
        // No Subagent segment in a Tasks-only sticky.
        assert!(
            !lines[tabs].contains("[Subagent]"),
            "tasks-only title must not show the subagent tab: {}",
            lines[tabs]
        );
    }

    #[test]
    fn subagent_only_sticky_shows_single_tab_and_body() {
        let mut app = make_app();
        seed_subagent(&mut app, "refactor the store");
        app.subagent_panel_mut().expanded = true;

        let text = render_main_area_text(&mut app, 80, 20);
        let lines: Vec<&str> = text.lines().collect();
        let tabs = lines
            .iter()
            .position(|l| l.contains("[Subagent]"))
            .expect("tab row missing, got:\n{text}");
        let running = lines
            .iter()
            .position(|l| l.contains("Running"))
            .expect("running group header missing");

        assert_eq!(
            running - tabs,
            2,
            "expected one separator row between tabs and body, got:\n{text}"
        );
        assert!(
            lines[tabs + 1].trim().chars().all(|c| c == '─' || c == '│'),
            "separator row should be a hairline rule, got: {:?}",
            lines[tabs + 1]
        );
        // The body shows the short child id + summary.
        let body_text = lines[tabs + 2..].join("\n");
        assert!(body_text.contains("refactor the store"), "got:\n{text}");
        assert!(
            !lines[tabs].contains("[Tasks]"),
            "subagent-only title must not show the tasks tab: {}",
            lines[tabs]
        );
    }

    #[test]
    fn both_domains_show_tabs_on_one_title_row() {
        let mut app = make_app();
        seed_tasks(&mut app, "task work");
        seed_subagent(&mut app, "sub work");
        app.subagent_panel_mut().expanded = true;

        let text = render_main_area_text(&mut app, 120, 20);
        let title = text
            .lines()
            .find(|l| l.contains("[Tasks]") && l.contains("[Subagent]"))
            .expect("combined title row missing, got:\n{text}");

        // Both summaries appear on the same row (no double [Tasks] label).
        assert!(title.contains("task work"), "got:\n{title}");
        assert!(title.contains("sub work"), "got:\n{title}");

        // Active (default Tasks) is expanded → its body is under the hairline.
        let lines: Vec<&str> = text.lines().collect();
        let title_idx = lines.iter().position(|l| l.contains("[Tasks]")).unwrap();
        let body = lines[title_idx + 2..].join("\n");
        assert!(body.contains("task work"), "tasks body missing:\n{body}");
    }

    #[test]
    fn sticky_body_cells_carry_theme_bg() {
        // Render-invariant guard: blank cells in the expanded sticky must have
        // the theme background (AGENTS.md render invariants) so no "shadow"
        // residue survives when the strip changes shape.
        let mut app = make_app();
        seed_subagent(&mut app, "summary");
        app.subagent_panel_mut().expanded = true;

        let terminal = super::super::test_harness::render_main_area_terminal(&mut app, 80, 20);
        let buf = terminal.backend().buffer();
        let bg = app.theme.bg;
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                let cell = &buf[(x, y)];
                if cell.symbol() == " " || cell.symbol().is_empty() {
                    assert_eq!(cell.bg, bg, "blank cell at {x},{y} must carry theme.bg");
                }
            }
        }
    }
}
