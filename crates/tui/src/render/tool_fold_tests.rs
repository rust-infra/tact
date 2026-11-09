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
