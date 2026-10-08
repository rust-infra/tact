//! Sticky host strip under the Log — app-layer wrapper.
//!
//! The kit renders a three-domain host (`Tasks | Subagent | Background`); this
//! wrapper records the host's screen area and per-frame tab hit rectangles on
//! `MouseState` (mirrors how `render_log_panel` stores cancel-button areas),
//! then calls the pure renderer.

use ratatui::{Frame, layout::Rect};

use agent_tui_kit::components::{
    BackgroundPanelComponent, SubagentPanelComponent, TaskPanelComponent,
};

use crate::widgets::state::{App, SurfaceId};

pub(crate) use agent_tui_kit::render::sticky_host::STICKY_BORDER_ROWS;

/// True when any sticky domain (tasks, subagent overview, running background
/// tasks) is visible.
pub(crate) fn sticky_host_visible(app: &App) -> bool {
    agent_tui_kit::state::StickyTab::ALL
        .into_iter()
        .any(|tab| sticky_domain(app, tab).visible())
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

enum StickyDomain<'a> {
    Tasks(&'a TaskPanelComponent),
    Subagent(&'a SubagentPanelComponent),
    Background(&'a BackgroundPanelComponent),
}

impl StickyDomain<'_> {
    fn visible(self) -> bool {
        match self {
            Self::Tasks(panel) => panel.visible,
            Self::Subagent(panel) => panel.visible,
            Self::Background(panel) => panel.visible,
        }
    }

    fn expanded(self) -> bool {
        match self {
            Self::Tasks(panel) => panel.expanded,
            Self::Subagent(panel) => panel.expanded,
            Self::Background(panel) => panel.expanded,
        }
    }
}

enum StickyDomainMut<'a> {
    Tasks(&'a mut TaskPanelComponent),
    Subagent(&'a mut SubagentPanelComponent),
    Background(&'a mut BackgroundPanelComponent),
}

impl<'a> StickyDomainMut<'a> {
    fn set_expanded(self, expanded: bool) {
        match self {
            Self::Tasks(panel) => panel.expanded = expanded,
            Self::Subagent(panel) => panel.expanded = expanded,
            Self::Background(panel) => panel.expanded = expanded,
        }
    }

    fn scroll_mut(self) -> &'a mut usize {
        match self {
            Self::Tasks(panel) => &mut panel.scroll,
            Self::Subagent(panel) => &mut panel.scroll,
            Self::Background(panel) => &mut panel.scroll,
        }
    }
}

fn sticky_domain(app: &App, tab: agent_tui_kit::state::StickyTab) -> StickyDomain<'_> {
    match tab {
        agent_tui_kit::state::StickyTab::Tasks => StickyDomain::Tasks(app.task_panel()),
        agent_tui_kit::state::StickyTab::Subagent => StickyDomain::Subagent(app.subagent_panel()),
        agent_tui_kit::state::StickyTab::Background => {
            StickyDomain::Background(app.background_panel())
        }
    }
}

fn sticky_domain_mut(app: &mut App, tab: agent_tui_kit::state::StickyTab) -> StickyDomainMut<'_> {
    match tab {
        agent_tui_kit::state::StickyTab::Tasks => StickyDomainMut::Tasks(app.task_panel_mut()),
        agent_tui_kit::state::StickyTab::Subagent => {
            StickyDomainMut::Subagent(app.subagent_panel_mut())
        }
        agent_tui_kit::state::StickyTab::Background => {
            StickyDomainMut::Background(app.background_panel_mut())
        }
    }
}

pub(crate) fn sticky_tab_expanded(app: &App, tab: agent_tui_kit::state::StickyTab) -> bool {
    sticky_domain(app, tab).expanded()
}

pub(crate) fn sticky_scrollable(app: &App) -> bool {
    sticky_host_visible(app) && sticky_tab_expanded(app, active_sticky_tab(app))
}
pub(crate) fn set_sticky_expanded(
    app: &mut App,
    tab: agent_tui_kit::state::StickyTab,
    expanded: bool,
) {
    sticky_domain_mut(app, tab).set_expanded(expanded);
}

pub(crate) fn toggle_sticky_expanded(app: &mut App, tab: agent_tui_kit::state::StickyTab) {
    let expanded = sticky_tab_expanded(app, tab);
    set_sticky_expanded(app, tab, !expanded);
}
pub(crate) fn scroll_sticky(app: &mut App, tab: agent_tui_kit::state::StickyTab, delta: isize) {
    let scroll = sticky_domain_mut(app, tab).scroll_mut();
    *scroll = if delta < 0 {
        scroll.saturating_sub(delta.unsigned_abs())
    } else {
        scroll.saturating_add(delta as usize)
    };
}
pub(crate) fn render_task_panel(frame: &mut Frame, area: Rect, app: &mut App) {
    app.mouse.set_area(SurfaceId::TaskPanel, area);
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

    /// Seed a live `background_run` card the way the tool does: the card opens
    /// on `StepStarted`, and `background_run` then sends `ToolMeta { task_id }`
    /// once the task exists (the invocation itself has already returned).
    fn seed_running_background(app: &mut crate::widgets::state::App, task_id: &str, command: &str) {
        let mut presentation = tact_protocol::ToolPresentationInfo::generic("background_run");
        presentation.keep_live = true;
        app.handle_agent_update(tact_protocol::AgentUpdate::StepAdded(
            tact_protocol::PlanStep::new(
                "run build in background",
                "background_run",
                "bg1",
                std::collections::HashMap::from([("command".to_string(), command.to_string())]),
            ),
        ));
        app.handle_agent_update(tact_protocol::AgentUpdate::StepStarted {
            idx: 0,
            tool_id: "bg1".into(),
            tool_name: "background_run".into(),
            arg_summary: command.into(),
            arg_full: command.into(),
            presentation,
        });
        app.handle_agent_update(tact_protocol::AgentUpdate::ToolMeta {
            tool_id: "bg1".into(),
            model: None,
            token_usage: None,
            task_id: Some(task_id.into()),
        });
    }

    #[test]
    fn a_running_background_task_gets_its_own_sticky_domain() {
        let mut app = make_app();
        seed_running_background(&mut app, "018f3a2c", "cargo build");

        let text = render_main_area_text(&mut app, 100, 20);
        let lines: Vec<&str> = text.lines().collect();
        let tabs = lines
            .iter()
            .position(|l| l.contains("[Background]"))
            .expect("background tab missing, got:\n{text}");

        // Row shape: `[Background] N · ⏳ <id> <command> ⏱ <elapsed>`.
        assert!(
            lines[tabs].contains("[Background] 1 · ⏳"),
            "background title shape changed: {}",
            lines[tabs]
        );
        // Only this domain is up, so the title row lists no other tab.
        assert!(
            !lines[tabs].contains("[Tasks]") && !lines[tabs].contains("[Subagent]"),
            "a lone background task must not summon the other tabs: {}",
            lines[tabs]
        );
        // Default expanded → its body row sits under the hairline.
        assert!(
            lines[tabs + 1].trim().chars().all(|c| c == '─' || c == '│'),
            "separator row should be a hairline rule, got: {:?}",
            lines[tabs + 1]
        );
        let body = lines[tabs + 2..].join("\n");
        assert!(
            body.contains("018f3a2c"),
            "task id missing from body:\n{body}"
        );
        assert!(
            body.contains("cargo build"),
            "command missing from body:\n{body}"
        );
    }

    #[test]
    fn live_output_does_not_drop_a_background_row() {
        let mut app = make_app();
        seed_running_background(&mut app, "018f3a2c", "cargo build");

        // The first chunk of output rebuilds the card. The task id must survive
        // that rebuild, or the strip empties the moment the task prints
        // anything (the live report: it flashed for well under a second).
        app.handle_agent_update(tact_protocol::AgentUpdate::ToolProgress {
            tool_id: "bg1".into(),
            chunks: vec![tact_protocol::ToolOutputChunk::stdout("Compiling ...\n")],
        });

        assert_eq!(
            app.tools_mut().active[0].output.task_id.as_deref(),
            Some("018f3a2c"),
            "the card must keep its task id across a progress rebuild"
        );
        let text = render_main_area_text(&mut app, 100, 20);
        assert!(
            text.contains("[Background] 1 · "),
            "the strip lost its row after live output:\n{text}"
        );
        assert!(
            text.contains("018f3a2c"),
            "the row lost its task id:\n{text}"
        );
    }

    #[test]
    fn a_finished_background_task_lingers_then_the_domain_hides() {
        let mut app = make_app();
        seed_running_background(&mut app, "018f3a2c", "cargo build");
        assert!(app.background_panel().visible);

        // What the shell receives when the task ends.
        app.handle_agent_update(tact_protocol::AgentUpdate::BackgroundTaskFinished {
            tool_id: "bg1".into(),
            success: true,
            message: "Background task 018f3a2c completed".into(),
            output: "done".into(),
        });

        // The row stays for the linger window, with the outcome on it: a strip
        // that empties the instant a task ends reads as "it lost my task".
        assert!(
            app.background_panel().visible,
            "a finished task must keep its row for the linger window"
        );
        let text = render_main_area_text(&mut app, 100, 20);
        assert!(
            text.contains("[Background] 1 done"),
            "title must count the lingering row:\n{text}"
        );
        assert!(
            text.contains("✓ 018f3a2c") && text.contains("cargo build"),
            "the lingering row must carry the id and command:\n{text}"
        );

        // Once the window passes the row drops, and the strip goes with it —
        // no task event is involved, the idle tick's prune is the trigger.
        let after =
            std::time::Instant::now() + agent_tui_kit::state::background_panel::BACKGROUND_LINGER;
        app.background_panel_mut().prune_finished(after);
        app.sync_background_sticky();
        assert!(
            !app.background_panel().visible && !app.background_panel().expanded,
            "the strip must close once the last row expires"
        );
        let text = render_main_area_text(&mut app, 100, 20);
        assert!(
            !text.contains("[Background]"),
            "an expired row must leave no tab behind:\n{text}"
        );
    }

    #[test]
    fn all_three_domains_share_one_title_row() {
        let mut app = make_app();
        seed_tasks(&mut app, "task work");
        seed_subagent(&mut app, "sub work");
        seed_running_background(&mut app, "018f3a2c", "cargo build");
        app.background_panel_mut().expanded = true;
        app.mouse.active_sticky_tab = agent_tui_kit::state::StickyTab::Background;

        let text = render_main_area_text(&mut app, 140, 20);
        let title = text
            .lines()
            .find(|l| {
                l.contains("[Tasks]") && l.contains("[Subagent]") && l.contains("[Background]")
            })
            .unwrap_or_else(|| panic!("combined title row missing, got:\n{text}"));
        assert!(title.contains("018f3a2c"), "got:\n{title}");

        // The active domain is Background, so its body (not the task list) is
        // what follows the hairline.
        let lines: Vec<&str> = text.lines().collect();
        let title_idx = lines.iter().position(|l| l.contains("[Tasks]")).unwrap();
        let body = lines[title_idx + 2..].join("\n");
        assert!(
            body.contains("cargo build"),
            "background body missing:\n{body}"
        );
        assert!(!body.contains("task work"), "tasks body leaked in:\n{body}");
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
