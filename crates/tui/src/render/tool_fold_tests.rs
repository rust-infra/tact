//! Behavior coverage for folded tool runs, driven through the real render.

use std::collections::HashMap;

use tact_protocol::{AgentUpdate, PlanStep, StepStatus};

use crate::render::test_harness::{make_app, render_log_panel_text};
use crate::test_fixtures::StepCall;
use crate::widgets::state::App;

fn seed(app: &mut App, idx: usize, id: &str, tool: &str, arg: &str, status: StepStatus) {
    app.handle_agent_update(AgentUpdate::StepAdded(PlanStep::new(
        format!("step {id}"),
        tool,
        id,
        HashMap::from([("arg".to_string(), arg.to_string())]),
    )));
    app.handle_agent_update(StepCall::new(idx, id, tool, arg).started());
    app.handle_agent_update(
        StepCall::new(idx, id, tool, arg)
            .detail("out-1\nout-2\nout-3")
            .status(status)
            .duration_us(1_200)
            .finished(),
    );
}

/// Three adjacent successful reads, the canonical foldable run.
fn three_reads(app: &mut App) {
    seed(
        app,
        0,
        "r1",
        "read_file",
        "src/main.rs",
        StepStatus::Success,
    );
    seed(app, 1, "r2", "read_file", "src/lib.rs", StepStatus::Success);
    seed(app, 2, "r3", "read_file", "src/app.rs", StepStatus::Success);
}

#[test]
fn a_settled_run_renders_one_summary_row_instead_of_every_member() {
    let mut app = make_app();
    app.handle_agent_update(AgentUpdate::StreamChunk("Checking.\n".into()));
    three_reads(&mut app);
    app.handle_agent_update(AgentUpdate::StreamChunk("Done.\n".into()));

    let text = render_log_panel_text(&mut app, 80, 20);

    assert!(
        text.contains("3 tools"),
        "the run must collapse to a summary: {text}"
    );
    assert!(
        !text.contains("src/lib.rs"),
        "a hidden member must not be drawn: {text}"
    );
    assert!(
        !text.contains("src/app.rs"),
        "a hidden member must not be drawn: {text}"
    );
}

#[test]
fn unfolding_shows_every_member_and_folding_hides_them_again() {
    let mut app = make_app();
    app.handle_agent_update(AgentUpdate::StreamChunk("Checking.\n".into()));
    three_reads(&mut app);
    app.handle_agent_update(AgentUpdate::StreamChunk("Done.\n".into()));

    let folded = render_log_panel_text(&mut app, 80, 20);
    assert!(folded.contains("3 tools"));

    let phys = app.tools().blocks[0].phys_idx;
    let run_id = app.tools().run_identity_at(phys).expect("run identity");
    assert!(app.tools_mut().toggle_run_by_id(&run_id));

    let open = render_log_panel_text(&mut app, 80, 20);
    assert!(
        open.contains("src/lib.rs") && open.contains("src/app.rs"),
        "unfolding must reveal every member: {open}"
    );

    assert!(app.tools_mut().toggle_run_by_id(&run_id));
    let again = render_log_panel_text(&mut app, 80, 20);
    assert!(
        again.contains("3 tools") && !again.contains("src/lib.rs"),
        "refolding must hide them again: {again}"
    );
}

#[test]
fn the_summary_reports_one_total_not_a_per_call_sum() {
    let mut app = make_app();
    app.handle_agent_update(AgentUpdate::StreamChunk("Checking.\n".into()));
    three_reads(&mut app);
    app.handle_agent_update(AgentUpdate::StreamChunk("Done.\n".into()));

    let text = render_log_panel_text(&mut app, 90, 20);
    // Three calls ran, and the summary carries exactly one duration figure; the
    // per-call `1.20ms` rows are gone with the members.
    assert!(text.contains("3 tools"), "summary row expected: {text}");
    assert!(
        !text.contains("1.20ms"),
        "a folded run must not repeat every member's own duration: {text}"
    );
}

#[test]
fn a_failed_member_folds_and_the_summary_says_so() {
    let mut app = make_app();
    app.handle_agent_update(AgentUpdate::StreamChunk("Checking.\n".into()));
    seed(&mut app, 0, "r1", "read_file", "a.rs", StepStatus::Success);
    seed(&mut app, 1, "b1", "bash", "cargo test", StepStatus::Failed);
    seed(&mut app, 2, "r2", "read_file", "b.rs", StepStatus::Success);
    app.handle_agent_update(AgentUpdate::StreamChunk("Done.\n".into()));

    let text = render_log_panel_text(&mut app, 80, 20);
    assert!(
        text.contains("1 failed"),
        "a folded failure must be counted on the summary row: {text}"
    );
    assert!(
        !text.contains("Error"),
        "the failure's detail card folds away too: {text}"
    );
}

#[test]
fn a_running_tool_keeps_its_own_rows() {
    let mut app = make_app();
    app.handle_agent_update(AgentUpdate::StreamChunk("Checking.\n".into()));
    seed(&mut app, 0, "r1", "read_file", "a.rs", StepStatus::Success);
    seed(&mut app, 1, "r2", "read_file", "b.rs", StepStatus::Success);
    // Third call dispatched but not finished.
    app.handle_agent_update(AgentUpdate::StepAdded(PlanStep::new(
        "step r3",
        "read_file",
        "r3",
        HashMap::from([("arg".to_string(), "c.rs".to_string())]),
    )));
    app.handle_agent_update(StepCall::new(2, "r3", "read_file", "c.rs").started());

    let text = render_log_panel_text(&mut app, 80, 20);
    // The two finished calls are a settled run and fold. The live third call is
    // not in `blocks` at all, so it keeps its own rows underneath the summary —
    // that is the boundary the reader needs to see: folded history above, work
    // in progress below.
    assert!(text.contains("2 tools"), "the settled pair folds: {text}");
    assert!(
        text.contains("c.rs"),
        "the running call keeps its own rows: {text}"
    );
}

#[test]
fn rows_hidden_by_a_fold_are_not_clickable() {
    let mut app = make_app();
    app.handle_agent_update(AgentUpdate::StreamChunk("Checking.\n".into()));
    three_reads(&mut app);
    app.handle_agent_update(AgentUpdate::StreamChunk("Done.\n".into()));
    let _ = render_log_panel_text(&mut app, 80, 20);

    // The cache is what mouse hit-testing reads. A hidden member has no logical
    // row at all, which is why upward scroll cannot stall on a zero-height one.
    for block in &app.tools().blocks[1..] {
        assert!(
            app.log_scroll
                .phys_to_logical_cache
                .get(block.phys_idx)
                .copied()
                .flatten()
                .is_none(),
            "hidden member {} must not map to a logical row",
            block.tool_id
        );
    }
    assert!(
        app.log_scroll
            .phys_to_logical_cache
            .get(app.tools().blocks[0].phys_idx)
            .copied()
            .flatten()
            .is_some(),
        "the summary row itself stays addressable"
    );
}

#[test]
fn the_fold_generation_invalidates_the_layout_cache() {
    let mut app = make_app();
    app.handle_agent_update(AgentUpdate::StreamChunk("Checking.\n".into()));
    three_reads(&mut app);
    app.handle_agent_update(AgentUpdate::StreamChunk("Done.\n".into()));
    let _ = render_log_panel_text(&mut app, 80, 20);

    let before = app.log_scroll.visual_cache_fold_rev;
    let phys = app.tools().blocks[0].phys_idx;
    let run_id = app.tools().run_identity_at(phys).expect("run identity");
    assert!(app.tools_mut().toggle_run_by_id(&run_id));
    let _ = render_log_panel_text(&mut app, 80, 20);

    assert_ne!(
        before, app.log_scroll.visual_cache_fold_rev,
        "a toggle must be visible to the cache key, or the frame goes stale"
    );
}

#[test]
fn an_explicit_expand_survives_a_new_member_settling() {
    let mut app = make_app();
    app.handle_agent_update(AgentUpdate::StreamChunk("Checking.\n".into()));
    three_reads(&mut app);
    app.handle_agent_update(AgentUpdate::StreamChunk("Done.\n".into()));
    let _ = render_log_panel_text(&mut app, 80, 20);

    let phys = app.tools().blocks[0].phys_idx;
    let run_id = app.tools().run_identity_at(phys).expect("run identity");
    assert!(
        app.tools_mut().toggle_run_by_id(&run_id),
        "reader expands it"
    );

    // Another tool settles afterwards, in a separate run.
    app.handle_agent_update(AgentUpdate::StreamChunk("More.\n".into()));
    seed(&mut app, 3, "x1", "bash", "ls", StepStatus::Success);
    seed(&mut app, 4, "x2", "bash", "pwd", StepStatus::Success);
    app.handle_agent_update(AgentUpdate::StreamChunk("Done.\n".into()));

    let text = render_log_panel_text(&mut app, 80, 20);
    assert!(
        text.contains("src/lib.rs"),
        "the reader's expand must not snap shut when other work lands: {text}"
    );
}

/// Three reads preceded by a paragraph: the shape a real turn has, and the one
/// the old `z` rule could not fold.
fn reads_after_prose(app: &mut App) {
    app.add_user_message("Check the tree.".into());
    app.handle_agent_update(AgentUpdate::StreamChunk("Looking.\n\n".into()));
    seed(app, 0, "r1", "read_file", "a.rs", StepStatus::Success);
    seed(app, 1, "r2", "read_file", "b.rs", StepStatus::Success);
    app.handle_agent_update(AgentUpdate::StreamChunk("Done.\n\n".into()));
}

/// A left-button press at `(x, y)`, routed through the real mouse handler.
fn click(app: &mut App, x: u16, y: u16) {
    crate::handle_mouse_event(
        app,
        crossterm::event::MouseEvent {
            kind: crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
            column: x,
            row: y,
            modifiers: crossterm::event::KeyModifiers::NONE,
        },
    );
}

fn z(app: &mut App) {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    app.input_mode = crate::widgets::state::InputMode::Normal;
    crate::handlers::handle_normal_mode(
        app,
        crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char('z'),
            crossterm::event::KeyModifiers::NONE,
        ),
        &tx,
    );
}

/// Expanding moves the summary row off the viewport top, so a `z` keyed on that
/// row lost the run the instant it was unfolded: the second press did nothing
/// and the run could never be folded again.
#[test]
fn z_refolds_a_run_it_unfolded() {
    let mut app = make_app();
    reads_after_prose(&mut app);
    let folded = render_log_panel_text(&mut app, 100, 24);
    assert!(folded.contains("2 tools"), "starts folded: {folded}");

    z(&mut app);
    let open = render_log_panel_text(&mut app, 100, 24);
    assert!(
        open.contains("a.rs") && open.contains("b.rs"),
        "the first press expands: {open}"
    );

    z(&mut app);
    let again = render_log_panel_text(&mut app, 100, 24);
    assert!(
        again.contains("2 tools") && !again.contains("b.rs"),
        "the second press must fold it back: {again}"
    );
}

/// The same with prose in front, which puts a non-tool row above the run: a
/// rule keyed on the viewport top row found the paragraph, not the run, so even
/// the first press did nothing.
#[test]
fn z_folds_a_run_that_does_not_start_at_the_viewport_top() {
    let mut app = make_app();
    reads_after_prose(&mut app);
    let phys = app.tools().blocks[0].phys_idx;
    assert!(
        phys > 0,
        "the run must not start on the first row for this test to mean anything"
    );

    z(&mut app);
    let open = render_log_panel_text(&mut app, 100, 24);
    assert!(
        open.contains("a.rs"),
        "`z` must find the run below the prose, not give up on the paragraph: {open}"
    );
}

/// A click on an expanded run's head folds it again. The summary row that
/// carried the control is gone while the run is open, so the head row takes it
/// over — otherwise a run could only ever be expanded by mouse, never closed.
#[test]
fn clicking_an_expanded_run_head_folds_it_again() {
    use crate::render::test_harness::render_app_text;

    let mut app = make_app();
    reads_after_prose(&mut app);
    let _ = render_app_text(&mut app, 100, 24);

    let summary = app
        .mouse
        .open_actions
        .iter()
        .find(|action| {
            matches!(
                action.target,
                agent_tui_kit::render::log::OpenTarget::ToolRun { .. }
            )
        })
        .cloned()
        .expect("the folded summary is a control");
    click(&mut app, summary.rect.x + 2, summary.rect.y);
    let open = render_app_text(&mut app, 100, 24);
    assert!(open.contains("a.rs"), "the click expands: {open}");

    let head = app
        .mouse
        .open_actions
        .iter()
        .find(|action| {
            matches!(
                action.target,
                agent_tui_kit::render::log::OpenTarget::ToolRun { .. }
            )
        })
        .cloned()
        .expect("an expanded run's head is the control that closes it");
    click(&mut app, head.rect.x, head.rect.y);
    let again = render_app_text(&mut app, 100, 24);
    assert!(
        again.contains("2 tools") && !again.contains("b.rs"),
        "the head must fold the run back: {again}"
    );
}

/// The head's control is drawn, not invisible: `⏷` sits in the indent gutter on
/// the row the collapsed summary's `⏵` occupied, so a reader can see there is a
/// way back.
#[test]
fn an_expanded_run_head_draws_the_collapse_glyph() {
    let mut app = make_app();
    reads_after_prose(&mut app);
    let _ = render_log_panel_text(&mut app, 100, 24);
    let phys = app.tools().blocks[0].phys_idx;
    let run_id = app.tools().run_identity_at(phys).expect("run identity");
    assert!(app.tools_mut().toggle_run_by_id(&run_id));

    let text = render_log_panel_text(&mut app, 100, 24);
    assert!(text.contains('⏷'), "the way back must be visible: {text}");

    // And the members keep the head's columns: a prefix glyph would have pushed
    // the head one column right of the block below it.
    let head = text
        .lines()
        .find(|line| line.contains("a.rs"))
        .expect("head title");
    let member = text
        .lines()
        .find(|line| line.contains("b.rs"))
        .expect("member title");
    // A terminal column, not a byte offset: `⏷` is three bytes and one cell, so
    // a byte comparison would report a staircase that is not on screen.
    let column =
        |line: &str, needle: &str| line[..line.find(needle).expect("needle")].chars().count();
    assert_eq!(
        column(head, "1."),
        column(member, "2."),
        "the run must not staircase:\n{text}"
    );
}

/// A folded run whose summary was clicked must not also register the head's
/// control, or the summary and the head would overlap on the same row and the
/// click would depend on which one the list happened to hold first.
#[test]
fn only_the_state_that_is_drawn_registers_a_fold_control() {
    use crate::render::test_harness::render_app_text;

    let mut app = make_app();
    reads_after_prose(&mut app);
    let _ = render_app_text(&mut app, 100, 24);
    let folded = app
        .mouse
        .open_actions
        .iter()
        .filter(|action| {
            matches!(
                action.target,
                agent_tui_kit::render::log::OpenTarget::ToolRun { .. }
            )
        })
        .count();
    assert_eq!(folded, 1, "a folded run has exactly one fold control");

    let summary = app
        .mouse
        .open_actions
        .iter()
        .find(|action| {
            matches!(
                action.target,
                agent_tui_kit::render::log::OpenTarget::ToolRun { .. }
            )
        })
        .cloned()
        .expect("summary");
    click(&mut app, summary.rect.x + 2, summary.rect.y);
    let _ = render_app_text(&mut app, 100, 24);
    let expanded = app
        .mouse
        .open_actions
        .iter()
        .filter(|action| {
            matches!(
                action.target,
                agent_tui_kit::render::log::OpenTarget::ToolRun { .. }
            )
        })
        .count();
    assert_eq!(expanded, 1, "an expanded run has exactly one fold control");
}

/// Folding must not move the reader when the run is what they are looking at.
///
/// The run sits at the top of the viewport, so folding it replaces those rows in
/// place and nothing above the eye changed. The rows above wrap, which is what
/// makes a physical row index and a visual line disagree — and the compensation
/// used to compare one against the other, so it scrolled the reader up by the
/// run's saved rows and put unrelated prose on the line they were reading.
#[test]
fn folding_the_run_under_the_eye_leaves_the_eye_where_it_was() {
    let mut app = make_app();
    for i in 0..14 {
        app.add_system_message(format!(
            "note-{i:02} a body long enough to wrap into two or three visual lines here"
        ));
    }
    seed(&mut app, 0, "t1", "bash", "one", StepStatus::Success);
    seed(&mut app, 1, "t2", "bash", "two", StepStatus::Success);
    seed(&mut app, 2, "t3", "bash", "three", StepStatus::Success);
    // Enough tail that parking on the run is nowhere near the bottom clamp: a
    // clamped viewport moves for a reason that has nothing to do with the fold.
    for i in 0..40 {
        app.add_system_message(format!("tail-{i:02} line"));
    }

    let (width, height) = (44u16, 12u16);
    let _ = render_log_panel_text(&mut app, width, height);
    let run = agent_tui_kit::state::tool_group::runs(&app.tools().blocks)
        .into_iter()
        .next()
        .expect("three adjacent calls form a run");
    let key = agent_tui_kit::state::tool_group::run_key(&app.tools().blocks, &run)
        .expect("run key")
        .to_string();

    // Fixture: expand the run, then park the viewport on its own first row so
    // the run is what the reader is looking at, with wrapped prose above.
    assert!(app.toggle_fold_by_run_id(&key), "expand the run");
    let _ = render_log_panel_text(&mut app, width, height);
    let run_start = app.log_scroll.visual_start[run.phys_idx];
    app.log_scroll.visual_top = run_start;
    app.log_scroll.follow = false;
    let before = render_log_panel_text(&mut app, width, height);
    assert_eq!(
        app.log_viewport_top(),
        run_start,
        "fixture: the viewport must not be clamped away from the run"
    );
    assert!(
        before.lines().nth(1).unwrap_or("").contains("Bash"),
        "fixture: the eye must start on the run:\n{before}"
    );

    // The action: `z`, the key a reader presses, folds the run in view.
    z(&mut app);
    let after = render_log_panel_text(&mut app, width, height);
    let run_after = agent_tui_kit::state::tool_group::runs(&app.tools().blocks)
        .into_iter()
        .next()
        .expect("the run still exists");
    assert!(
        agent_tui_kit::state::tool_group::is_folded(
            &app.tools().blocks,
            &run_after,
            app.tools().state()
        ),
        "fixture: `z` must have folded the run it was pointed at"
    );

    assert_eq!(
        app.log_viewport_top(),
        run_start,
        "folding the run under the eye must not scroll: it collapses in \\
         place\nbefore:\n{before}\nafter:\n{after}"
    );
    assert!(
        after.lines().nth(1).unwrap_or("").contains("tools"),
        "the eye must still be on the run's summary row, not on prose that was \\
         above it\nbefore:\n{before}\nafter:\n{after}"
    );
}

// ── Activity regions: a thinking card and the calls beside it ──────────

/// A finished thought followed by two reads, the shape every reasoning turn has.
///
/// The card and the calls are one region of work, so they fold into **one**
/// row that counts both halves. Before, the turn cost the card's row plus the
/// run's row — two summaries of one stretch of work.
fn thought_then_reads(app: &mut App) {
    app.add_user_message("Check the tree.".into());
    app.handle_agent_update(AgentUpdate::StreamChunk("Looking.\n\n".into()));
    app.handle_agent_update(AgentUpdate::ThinkingChunk(
        tact_protocol::ThinkingChunk::Started,
    ));
    app.handle_agent_update(AgentUpdate::ThinkingChunk(
        tact_protocol::ThinkingChunk::Delta("I should read both files.\n".into()),
    ));
    app.handle_agent_update(AgentUpdate::ThinkingChunk(
        tact_protocol::ThinkingChunk::Finished,
    ));
    seed(app, 0, "r1", "read_file", "a.rs", StepStatus::Success);
    seed(app, 1, "r2", "read_file", "b.rs", StepStatus::Success);
    app.handle_agent_update(AgentUpdate::StreamChunk("Done.\n\n".into()));
}

#[test]
fn a_thought_and_the_calls_after_it_fold_into_one_row() {
    let mut app = make_app();
    thought_then_reads(&mut app);

    let text = render_log_panel_text(&mut app, 100, 24);

    assert!(
        text.contains("1 thought") && text.contains("2 tools"),
        "one row must count both halves: {text}"
    );
    assert!(
        !text.contains("Thought") && !text.contains("a.rs"),
        "the card's status row and the calls are behind the summary: {text}"
    );
    assert_eq!(
        text.matches("1 thought").count(),
        1,
        "the region is one summary, not one per member: {text}"
    );
}

#[test]
fn opening_a_region_shows_the_card_and_every_call_again() {
    let mut app = make_app();
    thought_then_reads(&mut app);
    let _ = render_log_panel_text(&mut app, 100, 24);

    let regions =
        agent_tui_kit::state::activity_region::regions(&app.thinking().blocks, &app.tools().blocks);
    let region = regions
        .into_iter()
        .find(|region| region.mixes_thinking_and_tools())
        .expect("one mixed region");
    assert!(app.toggle_region(region));

    let text = render_log_panel_text(&mut app, 100, 24);
    assert!(text.contains("Thought"), "the card draws again: {text}");
    assert!(!text.contains("1 thought"), "{text}");
    assert!(
        !text.contains("1 thought · 2 tools"),
        "the region summary is gone: {text}"
    );
    // The calls are inside the region, so they fold with it: opening the region
    // must not leave a run summary lying across its own members. One stretch of
    // work gets one summary — the region's — and opening it gives the members
    // their rows back.
    assert!(
        text.contains("a.rs") && text.contains("b.rs"),
        "the calls show their own rows: {text}"
    );
    assert!(
        !text.contains("2 tools"),
        "a run inside a region does not draw a second summary: {text}"
    );
}

/// A region whose calls all succeeded has no failure to report; one with a
/// failure keeps `⚠` and the count, so folding cannot make an error quieter.
#[test]
fn a_failed_call_inside_a_region_keeps_its_warning_in_the_summary() {
    let mut app = make_app();
    app.handle_agent_update(AgentUpdate::StreamChunk("Looking.\n\n".into()));
    app.handle_agent_update(AgentUpdate::ThinkingChunk(
        tact_protocol::ThinkingChunk::Started,
    ));
    app.handle_agent_update(AgentUpdate::ThinkingChunk(
        tact_protocol::ThinkingChunk::Delta("Try it.\n".into()),
    ));
    app.handle_agent_update(AgentUpdate::ThinkingChunk(
        tact_protocol::ThinkingChunk::Finished,
    ));
    seed(&mut app, 0, "ok", "read_file", "a.rs", StepStatus::Success);
    seed(&mut app, 1, "bad", "bash", "false", StepStatus::Failed);
    app.handle_agent_update(AgentUpdate::StreamChunk("Done.\n\n".into()));

    let text = render_log_panel_text(&mut app, 100, 24);

    assert!(text.contains("1 thought"), "{text}");
    assert!(text.contains("1 failed"), "{text}");
    assert!(text.contains('⚠'), "a failure keeps its glyph: {text}");
}

/// The region is keyed on its first member, so a reader's choice survives the
/// tool span growing or a later call landing.
#[test]
fn an_opened_region_stays_open_when_another_call_settles() {
    let mut app = make_app();
    thought_then_reads(&mut app);
    let _ = render_log_panel_text(&mut app, 100, 24);

    let regions =
        agent_tui_kit::state::activity_region::regions(&app.thinking().blocks, &app.tools().blocks);
    let region = regions
        .into_iter()
        .find(|region| region.mixes_thinking_and_tools())
        .expect("one mixed region");
    assert!(app.toggle_region(region));

    // A later call, separated by prose so it is its own work.
    app.handle_agent_update(AgentUpdate::StreamChunk("More.\n\n".into()));
    seed(&mut app, 2, "x1", "bash", "ls", StepStatus::Success);

    let text = render_log_panel_text(&mut app, 100, 24);
    assert!(
        text.contains("Thought"),
        "the reader's open region must not snap shut: {text}"
    );
    assert!(
        !text.contains("1 thought · 2 tools"),
        "it stays a region of separate members, not one summary: {text}"
    );
}

/// A turn ends with a separator + stats block, which must break the region: two
/// turns never fold into one summary.
///
/// This pins the boundary the fold has no explicit turn id for — the log is the
/// only evidence, so the separator rows are what keep turns apart.
#[test]
fn two_turns_never_fold_into_one_summary() {
    let mut app = make_app();

    // Turn 1: a thought and two calls, then the turn ends.
    thought_then_reads(&mut app);
    app.handle_agent_update(AgentUpdate::TaskComplete("first turn done".into()));

    // Turn 2: another thought and two calls.
    app.add_user_message("Again.".into());
    app.handle_agent_update(AgentUpdate::StreamChunk("Looking.\n\n".into()));
    app.handle_agent_update(AgentUpdate::ThinkingChunk(
        tact_protocol::ThinkingChunk::Started,
    ));
    app.handle_agent_update(AgentUpdate::ThinkingChunk(
        tact_protocol::ThinkingChunk::Delta("Read them again.\n".into()),
    ));
    app.handle_agent_update(AgentUpdate::ThinkingChunk(
        tact_protocol::ThinkingChunk::Finished,
    ));
    seed(&mut app, 0, "s1", "read_file", "c.rs", StepStatus::Success);
    seed(&mut app, 1, "s2", "read_file", "d.rs", StepStatus::Success);
    app.handle_agent_update(AgentUpdate::StreamChunk("Done.\n\n".into()));

    // One frame, so the prepare pass fills `turn_boundary_rows` — the same set
    // the fold rules read in the real render loop.
    let _ = render_log_panel_text(&mut app, 100, 40);
    let regions = agent_tui_kit::state::activity_region::regions_bounded(
        &app.thinking().blocks,
        &app.tools().blocks,
        |phys| app.log_scroll.turn_boundary_rows.contains(&phys),
    );
    assert_eq!(app.thinking().blocks.len(), 2, "two thoughts exist");
    assert_eq!(app.tools().blocks.len(), 4, "four calls exist");
    assert!(
        !app.log_scroll.turn_boundary_rows.is_empty(),
        "the prepare pass must fill the boundary set the fold rules read"
    );
    assert_eq!(
        regions.len(),
        2,
        "the turn separator splits the log into two regions, not one: {:?}",
        regions
            .iter()
            .map(|region| (region.phys_idx, region.thoughts(), region.tools()))
            .collect::<Vec<_>>()
    );
    for region in &regions {
        assert_eq!(region.thoughts(), 1, "one thought per turn");
        assert_eq!(region.tools(), 2, "two calls per turn");
    }
}

/// The turn boundary is row evidence, not a turn id: the separator's own row
/// sits between the two turns' members, which is what breaks adjacency.
#[test]
fn the_turn_separator_row_sits_between_two_turns_activities() {
    let mut app = make_app();
    thought_then_reads(&mut app);
    let last_call = app.tools().blocks.last().expect("a call").phys_idx;
    let call_rows = app
        .tools()
        .blocks
        .last()
        .expect("a call")
        .output
        .visual_rows(false);
    app.handle_agent_update(AgentUpdate::TaskComplete("first turn done".into()));

    // Something in turn 2 must come after the separator rows.
    app.add_user_message("Again.".into());
    let separator_phys = app
        .log
        .items
        .iter()
        .position(|item| crate::render::cells::separator::is_task_end_separator(&item.raw))
        .expect("the turn separator row");

    assert!(
        separator_phys > last_call + call_rows,
        "the separator must sit after the turn's last call row \
         (call at {last_call}, {call_rows} rows, separator at {separator_phys})"
    );
}

/// Two turns whose activities happen to sit on adjacent rows are still two
/// regions: the fold asks the log where a turn ended instead of inferring it
/// from spacing.
#[test]
fn adjacency_alone_does_not_fuse_two_turns() {
    use agent_tui_kit::widgets::tool_widget::{
        ToolLayout, ToolPhase as Phase, ToolRenderOutput, tool_visual_rows,
    };

    let mk_tool = |id: &str, phys: usize| agent_tui_kit::state::ToolBlock {
        phys_idx: phys,
        tool_id: id.to_string(),
        output: ToolRenderOutput {
            title_raw: format!("$ Bash  {id}"),
            phase: Phase::Success,
            permission_label: None,
            error_message: None,
            duration_us: None,
            size_bytes: None,
            tool_name: "bash".to_string(),
            use_diff_gutter: false,
            arg_summary: id.to_string(),
            arg_full: String::new(),
            layout: ToolLayout {
                visual_rows: tool_visual_rows(false, 0, 0, false),
                preview_lines: 0,
                has_detail_card: false,
                detail_collapsed: true,
            },
            live_detail: false,
            detail_preview: Vec::new(),
            detail_total_lines: 0,
            detail_full: None,
            subagent_id: None,
            subagent_model: None,
            subagent_tokens: None,
            subagent_context_window: None,
            task_id: None,
            visual_kind: tact_protocol::ToolVisualKind::Command,
        },
        span: None,
    };
    let mk_card = |id: &str, phys: usize| {
        agent_tui_kit::state::ThinkingBlock::new(
            id.to_string(),
            phys,
            "x".into(),
            "x".into(),
            Vec::new(),
            std::time::Duration::from_millis(1),
        )
    };

    // Turn 1 owns rows 1–2, turn 2 owns 4–5. Row 3 is the boundary probe: a
    // turn end that left no member of its own.
    let cards = vec![mk_card("turn1-thought", 1), mk_card("turn2-thought", 4)];
    let tools = vec![mk_tool("turn1-call", 2), mk_tool("turn2-call", 5)];

    let fused = agent_tui_kit::state::activity_region::regions(&cards, &tools);
    assert_eq!(fused.len(), 1, "without a boundary, adjacency merges them");
    assert_eq!(fused[0].thoughts(), 2);

    let split =
        agent_tui_kit::state::activity_region::regions_bounded(&cards, &tools, |phys| phys == 3);
    assert_eq!(
        split.len(),
        2,
        "the turn boundary keeps them apart: {:?}",
        split
            .iter()
            .map(|r| (r.thoughts(), r.tools()))
            .collect::<Vec<_>>()
    );
}

/// `/lang` rewrites the folded summary: the words come from `Messages`, the
/// tool labels and the `×` glyph do not.
#[test]
fn a_folded_region_summary_follows_the_language() {
    let mut app = make_app();
    thought_then_reads(&mut app);
    app.language = crate::i18n::Language::Chinese;

    let text = render_log_panel_text(&mut app, 100, 24);

    // The buffer holds one cell per column, so a wide character's second cell
    // is blank: compare the glyphs, not the contiguous string.
    let glyphs: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    assert!(
        glyphs.contains("1段思考") && glyphs.contains("2个工具"),
        "the counts are localized: {text}"
    );
    assert!(
        glyphs.contains("📖Read×2"),
        "the tool label and repeat glyph stay: {text}"
    );
    assert!(
        !glyphs.contains("1thought"),
        "no English wording survives the switch: {text}"
    );
}

/// A tool-only run's summary is localized too — it composes through the same
/// `Messages` half as a mixed region.
#[test]
fn a_folded_tool_run_summary_follows_the_language() {
    let mut app = make_app();
    app.handle_agent_update(AgentUpdate::StreamChunk("Checking.\n\n".into()));
    three_reads(&mut app);
    app.handle_agent_update(AgentUpdate::StreamChunk("Done.\n\n".into()));
    app.language = crate::i18n::Language::Chinese;

    let text = render_log_panel_text(&mut app, 100, 24);

    let glyphs: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    assert!(
        glyphs.contains("3个工具"),
        "the tool count is localized: {text}"
    );
    assert!(!glyphs.contains("3tools"), "{text}");
}

// ── Narration folding ([ui] fold_commentary) ─────────────────────────────

/// A turn that narrates, calls a tool, then answers: the shape
/// `fold_commentary` exists for.
fn narration_then_tool_then_answer(app: &mut App) {
    app.add_user_message("Check the tree.".into());
    app.handle_agent_update(AgentUpdate::MessageStarted {
        message_id: "m1".into(),
    });
    app.handle_agent_update(AgentUpdate::StreamChunk("Let me check that.\n\n".into()));
    app.handle_agent_update(AgentUpdate::MessageFinished {
        message_id: "m1".into(),
        phase: Some(tact_protocol::MessagePhase::Commentary),
    });
    seed(app, 0, "r1", "read_file", "a.rs", StepStatus::Success);
    app.handle_agent_update(AgentUpdate::MessageStarted {
        message_id: "m2".into(),
    });
    app.handle_agent_update(AgentUpdate::StreamChunk("All clean.\n\n".into()));
    app.handle_agent_update(AgentUpdate::MessageFinished {
        message_id: "m2".into(),
        phase: Some(tact_protocol::MessagePhase::FinalAnswer),
    });
}

#[test]
fn a_commentary_label_turns_the_rows_into_narration() {
    use agent_tui_kit::state::LogItemKind;

    let mut app = make_app();
    narration_then_tool_then_answer(&mut app);

    let narration: Vec<&str> = app
        .log
        .items
        .iter()
        .filter(|item| item.kind == LogItemKind::AssistantCommentary)
        .map(|item| item.raw.as_str())
        .collect();
    assert!(
        narration
            .iter()
            .any(|raw| raw.contains("Let me check that.")),
        "the narrative text is marked: {narration:?}"
    );
    assert!(
        app.log
            .items
            .iter()
            .any(|item| item.kind == LogItemKind::AssistantMarkdown
                && item.raw.contains("All clean.")),
        "the final answer keeps its own kind"
    );
}

#[test]
fn narration_folds_with_the_call_and_the_answer_stays_visible() {
    let mut app = make_app();
    narration_then_tool_then_answer(&mut app);

    let text = render_log_panel_text(&mut app, 100, 32);

    assert!(
        text.contains("All clean."),
        "the answer is never folded: {text}"
    );
    assert!(
        !text.contains("Let me check that."),
        "narration is behind the summary: {text}"
    );
    assert!(
        text.contains("note") || text.contains("tool"),
        "the summary states what it hid: {text}"
    );
}

#[test]
fn the_switch_restores_narration_rows() {
    let mut app = make_app();
    narration_then_tool_then_answer(&mut app);
    assert!(app.fold_commentary, "folded by default");

    app.toggle_fold_commentary();
    let text = render_log_panel_text(&mut app, 100, 32);

    assert!(
        text.contains("Let me check that."),
        "off keeps every narration row: {text}"
    );
    assert!(text.contains("All clean."), "the answer too: {text}");
}

/// Text no provider labelled must not be folded on a guess.
#[test]
fn unlabelled_text_is_never_folded() {
    let mut app = make_app();
    app.add_user_message("Check the tree.".into());
    app.handle_agent_update(AgentUpdate::MessageStarted {
        message_id: "m1".into(),
    });
    app.handle_agent_update(AgentUpdate::StreamChunk("No phase here.\n\n".into()));
    app.handle_agent_update(AgentUpdate::MessageFinished {
        message_id: "m1".into(),
        phase: None,
    });
    seed(&mut app, 0, "r1", "read_file", "a.rs", StepStatus::Success);

    let text = render_log_panel_text(&mut app, 100, 32);
    assert!(
        text.contains("No phase here."),
        "an unlabelled message keeps its rows: {text}"
    );
}

/// A region's clock covers its thinking **and** its calls. Before, any region
/// that held a tool dropped the card's time entirely — the summary reported the
/// calls' union and the thought was invisible.
#[test]
fn a_folded_region_reporting_time_includes_the_thought() {
    let mut app = make_app();
    app.handle_agent_update(AgentUpdate::StreamChunk("Looking.\n\n".into()));
    // A thought that takes real time: the card's own clock.
    app.handle_agent_update(AgentUpdate::ThinkingChunk(
        tact_protocol::ThinkingChunk::Started,
    ));
    std::thread::sleep(std::time::Duration::from_millis(40));
    app.handle_agent_update(AgentUpdate::ThinkingChunk(
        tact_protocol::ThinkingChunk::Delta("Consider the tree.\n".into()),
    ));
    app.handle_agent_update(AgentUpdate::ThinkingChunk(
        tact_protocol::ThinkingChunk::Finished,
    ));
    seed(&mut app, 0, "r1", "read_file", "a.rs", StepStatus::Success);
    seed(&mut app, 1, "r2", "read_file", "b.rs", StepStatus::Success);
    app.handle_agent_update(AgentUpdate::StreamChunk("Done.\n\n".into()));

    // The card carries a span now, so the region's total can see it.
    let card_span = app.thinking().blocks[0]
        .span
        .expect("a live card keeps its clock");
    assert!(
        card_span.finished_at > card_span.started_at,
        "the card's span is a real interval"
    );

    let _ = render_log_panel_text(&mut app, 100, 32);
    let regions = app.activity_regions();
    let region = &regions[0];
    let total = agent_tui_kit::state::activity_region::total_span(
        &app.thinking().blocks,
        &app.tools().blocks,
        region,
    )
    .expect("the region was timed");

    let thought = card_span.finished_at.duration_since(card_span.started_at);
    assert!(
        total >= thought,
        "the total must cover the thought's own span: total={total:?} thought={thought:?}"
    );
}
