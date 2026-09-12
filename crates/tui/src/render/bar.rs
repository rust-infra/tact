//! Bottom bar / status bar — app-layer wrappers.
//!
//! The rendering moved verbatim into `agent_tui_kit::render::bar` (pure
//! `&RenderCtx` functions); this module keeps the `&App`-shaped entry points
//! the shell and the test harness call, plus the App-integration tests that
//! exercise the full `App` (they render through the wrapper, so their
//! expectations are unchanged).

use ratatui::{Frame, layout::Rect};

use agent_tui_kit::render::bar as kit_bar;

use crate::widgets::state::App;

/// Render the bottom bar (permission mode, path, uptime, balance, model,
/// token meter, cache).
pub(crate) fn render_bottom_bar(frame: &mut Frame, area: Rect, app: &App) {
    let ctx = app.render_ctx();
    kit_bar::render_bottom_bar(frame, area, &ctx);
}

/// Render the top status bar (mode, focused panel, Agent state, flash msg).
pub(crate) fn render_status_bar(frame: &mut Frame, area: Rect, app: &App) {
    let ctx = app.render_ctx();
    kit_bar::render_status_bar(frame, area, &ctx);
}

#[cfg(test)]
mod render_tests {
    use ratatui::{Terminal, backend::TestBackend, layout::Rect};
    use tact_protocol::{BalanceEntry, BalanceInfo};

    use super::{
        super::test_harness::{buffer_text, make_app, render_app_text},
        render_bottom_bar,
    };

    #[test]
    fn bottom_bar_shows_balance_row_when_available() {
        let (_tx, account_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = make_app();
        app.account_rx = Some(account_rx);
        app.account.balance = Some(BalanceInfo {
            is_available: true,
            balance_infos: vec![BalanceEntry {
                currency: "USD".into(),
                total_balance: 12.50,
                granted_balance: 10.00,
                topped_up_balance: 2.50,
            }],
        });

        let text = render_app_text(&mut app, 120, 12);
        assert!(
            text.contains("12.50") || text.contains("USD"),
            "balance should append on bottom bar row 1, got:\n{text}"
        );
    }

    #[test]
    fn bottom_bar_renders_without_panic_when_idle() {
        let app = make_app();
        let backend = TestBackend::new(100, 2);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| render_bottom_bar(frame, Rect::new(0, 0, 100, 2), &app))
            .expect("draw");
        assert!(!buffer_text(terminal.backend().buffer()).trim().is_empty());
    }

    #[test]
    fn bottom_bar_shows_context_usage_meter_on_row_2() {
        let mut app = make_app();
        app.model_context_window = 200_000;
        app.status_bar_mut().model_name = "mock-model".into();
        app.status_bar_mut().token_total = 590;
        app.status_bar_mut().token_prompt = 400;
        app.status_bar_mut().token_completion = 190;

        let backend = TestBackend::new(160, 2);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| render_bottom_bar(frame, Rect::new(0, 0, 160, 2), &app))
            .expect("draw");

        let text = buffer_text(terminal.backend().buffer());
        let lines: Vec<&str> = text.lines().collect();
        assert!(lines.len() >= 2, "expected 2 rows, got:\n{text}");
        let row2 = lines[1];
        assert!(
            row2.contains("mock-model") && row2.contains("ctx [") && row2.contains("590/200K"),
            "row 2 should show model + labeled meter + used/window, got:\n{row2}"
        );
        assert!(
            !row2.contains('%'),
            "the ctx percentage is derivable from used/window and must not render, got:\n{row2}"
        );
        assert!(
            row2.contains('[') && row2.contains(']'),
            "row 2 should include progress bar brackets, got:\n{row2}"
        );
        assert!(
            !row2.contains('█') && !row2.contains('░'),
            "row 2 should use mid-height bar glyphs, got:\n{row2}"
        );
    }

    #[test]
    fn bottom_bar_shows_uptime_on_row_1_without_elapsed() {
        let mut app = make_app();
        app.last_prompt_elapsed_secs = Some(65); // 01:05 — belongs on task-end separator now
        app.status_bar_mut().model_name = "mock-model".into();
        app.status_bar_mut().token_total = 42;
        app.workspace_dir = "/tmp/tact-ws".into();
        app.status_bar_mut().git_branch = "main".into();

        let backend = TestBackend::new(140, 2);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| render_bottom_bar(frame, Rect::new(0, 0, 140, 2), &app))
            .expect("draw");

        let text = buffer_text(terminal.backend().buffer());
        let lines: Vec<&str> = text.lines().collect();
        assert!(
            lines.len() >= 2,
            "bottom bar should render two rows, got:\n{text}"
        );
        let row1 = lines[0];
        let row2 = lines[1];
        assert!(
            !row1.contains("Elapsed") && !row1.contains("01:05"),
            "elapsed must not appear on bottom bar, got:\n{row1}"
        );
        assert!(
            row1.contains("│"),
            "row 1 should use box-drawing separators, got:\n{row1}"
        );
        assert!(
            row1.contains("/tmp/tact-ws") && row1.contains("main"),
            "cwd and branch should remain on row 1, got:\n{row1}"
        );
        assert!(
            !row2.contains("Elapsed:") && !row2.contains("Up:"),
            "elapsed/uptime must not appear on row 2, got:\n{row2}"
        );
        assert!(
            row2.contains("ctx [") && row2.contains("42/"),
            "token usage should stay on row 2 via the ctx meter, got:\n{row2}"
        );
    }

    #[test]
    fn bottom_bar_shows_compact_model_with_limits() {
        let mut app = make_app();
        app.status_bar_mut().model_name = "mock-model".into();
        app.status_bar_mut().model_max_tokens = 128_000;
        app.status_bar_mut().model_thinking_budget = Some(32_000);
        app.status_bar_mut().model_reasoning_effort = Some("high".into());
        let backend = TestBackend::new(120, 2);
        let mut terminal = Terminal::new(backend).expect("terminal");

        terminal
            .draw(|frame| render_bottom_bar(frame, Rect::new(0, 0, 120, 2), &app))
            .expect("draw");

        let text = buffer_text(terminal.backend().buffer());
        assert!(
            text.contains("mock-model") && text.contains("out 128K") && text.contains("think high"),
            "bottom bar should show model + out budget + think effort, got:\n{text}"
        );
    }

    #[test]
    fn bottom_bar_shows_compact_model_when_effort_is_absent() {
        let mut app = make_app();
        app.status_bar_mut().model_name = "mock-model".into();
        app.status_bar_mut().model_max_tokens = 128_000;
        app.status_bar_mut().model_thinking_budget = Some(32_000);
        let backend = TestBackend::new(120, 2);
        let mut terminal = Terminal::new(backend).expect("terminal");

        terminal
            .draw(|frame| render_bottom_bar(frame, Rect::new(0, 0, 120, 2), &app))
            .expect("draw");

        let text = buffer_text(terminal.backend().buffer());
        assert!(
            text.contains("out 128K") && text.contains("think 32K"),
            "bottom bar should show out/think without effort label, got:\n{text}"
        );
    }

    #[test]
    fn bottom_bar_subtracts_effort_share_from_out_budget() {
        let mut app = make_app();
        app.status_bar_mut().model_name = "mock-model".into();
        app.status_bar_mut().model_max_tokens = 128_000;
        app.status_bar_mut().model_reasoning_effort = Some("high".into());
        // 128k × 100/175 ≈ 73.1K — the reasoning share is subtracted from the
        // shared envelope for effort-semantic models.
        let backend = TestBackend::new(120, 2);
        let mut terminal = Terminal::new(backend).expect("terminal");

        terminal
            .draw(|frame| render_bottom_bar(frame, Rect::new(0, 0, 120, 2), &app))
            .expect("draw");

        let text = buffer_text(terminal.backend().buffer());
        assert!(
            text.contains("out 73.1K") && text.contains("think high"),
            "bottom bar should subtract the reasoning share, got:\n{text}"
        );
    }

    #[test]
    fn bottom_bar_drops_cache_before_model_on_narrow_width() {
        let mut app = make_app();
        app.status_bar_mut().model_name = "mock-model".into();
        app.status_bar_mut().model_max_tokens = 8_000;
        app.status_bar_mut().model_thinking_budget = Some(32_000);
        app.status_bar_mut().model_reasoning_effort = Some("high".into());
        app.status_bar_mut().token_total = 100;
        app.status_bar_mut().token_cache_hit = 50;
        app.status_bar_mut().token_cache_miss = 50;
        app.model_context_window = 200_000;

        let backend = TestBackend::new(40, 2);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| render_bottom_bar(frame, Rect::new(0, 0, 40, 2), &app))
            .expect("draw");
        let text = buffer_text(terminal.backend().buffer());
        assert!(
            text.contains("mock-model"),
            "model should remain, got:\n{text}"
        );
        assert!(
            !text.contains('▣'),
            "cache segment should drop on narrow width, got:\n{text}"
        );
    }

    /// Width budget: with every row-2 segment populated, nothing may be
    /// dropped at a 100-column terminal.
    ///
    /// The 2026-09-12 compaction got the full row down to 97 columns (from
    /// ~138) by removing the `∑ₜₒₖ` segment — whose value was a second format
    /// of the same `token_total` the ctx meter already renders — plus the
    /// `max_out_token`/`cache%`/`turns` word labels. This test is the guard:
    /// if a future segment pushes the row past ~100 columns it will start
    /// silently dropping segments on ordinary terminals, which is the exact
    /// problem this compaction fixed.
    #[test]
    fn bottom_bar_fits_every_segment_in_100_columns() {
        let mut app = make_app();
        app.status_bar_mut().model_name = "deepseek-v4".into();
        app.status_bar_mut().model_max_tokens = 128_000;
        app.status_bar_mut().model_reasoning_effort = Some("high".into());
        app.status_bar_mut().token_total = 45_000;
        app.status_bar_mut().token_cache_hit = 30;
        app.status_bar_mut().token_cache_miss = 70;
        app.status_bar_mut().turn_user = 12;
        app.status_bar_mut().turn_llm = 3;
        app.status_bar_mut().turn_last_secs = Some(125);
        app.status_bar_mut().turn_done = 2;
        app.status_bar_mut().turn_total_secs = 210;
        app.model_context_window = 1_000_000;

        let backend = TestBackend::new(100, 2);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| render_bottom_bar(frame, Rect::new(0, 0, 100, 2), &app))
            .expect("draw");
        let text = buffer_text(terminal.backend().buffer());
        let row2 = text.lines().nth(1).unwrap_or_default().to_string();

        for marker in [
            "deepseek-v4",
            "out 73.1K",
            "think high",
            "ctx [",
            "⟳",
            "⇅",
            "▣",
            "⏱",
            "avg",
        ] {
            assert!(
                row2.contains(marker),
                "row 2 dropped {marker:?} at 100 columns (budget exceeded), got:\n{row2}"
            );
        }
    }

    #[test]
    fn bottom_bar_shows_turn_counters_and_timing_on_row_2() {
        let mut app = make_app();
        app.status_bar_mut().model_name = "mock-model".into();
        app.status_bar_mut().token_total = 120;
        app.status_bar_mut().turn_user = 12;
        app.status_bar_mut().turn_llm = 3;
        app.status_bar_mut().turn_last_secs = Some(125);
        app.status_bar_mut().turn_done = 2;
        app.status_bar_mut().turn_total_secs = 210;

        let backend = TestBackend::new(200, 2);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| render_bottom_bar(frame, Rect::new(0, 0, 200, 2), &app))
            .expect("draw");

        let text = buffer_text(terminal.backend().buffer());
        let row2 = text.lines().nth(1).unwrap_or_default();
        assert!(
            row2.contains("⟳ 12") && row2.contains("⇅ 3"),
            "row 2 should show session + LLM turn counts, got:\n{row2}"
        );
        assert!(
            row2.contains("⏱ 02:05") && row2.contains("avg 01:45"),
            "row 2 should show last + average turn time, got:\n{row2}"
        );
    }

    #[test]
    fn bottom_bar_omits_llm_segment_before_first_turn() {
        let mut app = make_app();
        app.status_bar_mut().model_name = "mock-model".into();
        app.status_bar_mut().token_total = 120;
        app.status_bar_mut().turn_user = 4;
        // No `TurnStats` yet this task: turn_llm == 0, no timing either.

        let backend = TestBackend::new(200, 2);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| render_bottom_bar(frame, Rect::new(0, 0, 200, 2), &app))
            .expect("draw");

        let text = buffer_text(terminal.backend().buffer());
        let row2 = text.lines().nth(1).unwrap_or_default();
        assert!(
            row2.contains("⟳ 4"),
            "session turn count should render, got:\n{row2}"
        );
        assert!(
            !row2.contains('⇅') && !row2.contains('⏱'),
            "LLM count / timing must stay hidden before the first turn, got:\n{row2}"
        );
    }

    #[test]
    fn bottom_bar_omits_turn_average_before_any_turn_completes() {
        let mut app = make_app();
        app.status_bar_mut().model_name = "mock-model".into();
        app.status_bar_mut().turn_user = 1;
        app.status_bar_mut().turn_llm = 1;
        app.status_bar_mut().turn_last_secs = Some(5);
        // turn_done == 0: `turn_last_secs` may be seeded by a freeze path that
        // has not yet counted a completed turn, so no average must appear.

        let backend = TestBackend::new(200, 2);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| render_bottom_bar(frame, Rect::new(0, 0, 200, 2), &app))
            .expect("draw");

        let row2 = buffer_text(terminal.backend().buffer())
            .lines()
            .nth(1)
            .unwrap_or_default()
            .to_string();
        assert!(row2.contains("⏱ 00:05"), "last turn time missing:\n{row2}");
        assert!(
            !row2.contains("avg") && !row2.contains('均'),
            "average must not render with zero completed turns, got:\n{row2}"
        );
    }

    #[test]
    fn bottom_bar_drops_turn_timing_before_cache() {
        let mut app = make_app();
        app.status_bar_mut().model_name = "mock-model".into();
        app.status_bar_mut().token_total = 100;
        app.status_bar_mut().token_cache_hit = 50;
        app.status_bar_mut().token_cache_miss = 50;
        app.status_bar_mut().turn_user = 12;
        app.status_bar_mut().turn_llm = 3;
        app.status_bar_mut().turn_last_secs = Some(125);
        app.status_bar_mut().turn_done = 2;
        app.status_bar_mut().turn_total_secs = 210;

        // `fit_row_spans` drops the last droppable first, so a segment's
        // appearance width must be monotonic in survival priority: timing
        // (lowest) appears no earlier than cache (higher). Probing a range of
        // widths and comparing the first width each segment survives is
        // robust; asserting on one fixed width is not.
        let mut first_cache: Option<u16> = None;
        let mut first_timing: Option<u16> = None;
        for width in 20u16..=120 {
            let backend = TestBackend::new(width, 2);
            let mut terminal = Terminal::new(backend).expect("terminal");
            terminal
                .draw(|frame| {
                    render_bottom_bar(frame, Rect::new(0, 0, width, 2), &app);
                })
                .expect("draw");
            let text = buffer_text(terminal.backend().buffer());
            if first_cache.is_none() && text.contains('▣') {
                first_cache = Some(width);
            }
            if first_timing.is_none() && text.contains('⏱') {
                first_timing = Some(width);
            }
        }

        let cache_w = first_cache.expect("cache segment never rendered at any probed width");
        let timing_w = first_timing.expect("turn timing never rendered at any probed width");
        assert!(
            timing_w >= cache_w,
            "timing must not survive narrower than cache (timing at {timing_w}, cache at {cache_w})"
        );
        assert!(
            timing_w > cache_w,
            "expected a width band where cache shows but timing is already dropped"
        );
    }
}
