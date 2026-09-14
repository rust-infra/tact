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
        render_bottom_bar, render_status_bar,
    };
    use crate::widgets::state::Status;

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
            row2.contains("mock-model") && row2.contains("ctx 0% 590/200K"),
            "row 2 should show model + ctx percentage + used/window, got:\n{row2}"
        );
        assert!(
            !row2.contains('[') && !row2.contains(']'),
            "the ctx gauge was dropped on 2026-09-12, got:\n{row2}"
        );
        assert!(
            !row2.contains('█') && !row2.contains('░') && !row2.contains('▍'),
            "old gauge glyphs must not survive, got:\n{row2}"
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
            row2.contains("ctx ") && row2.contains('%') && row2.contains("42/"),
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

    /// The rendered `out` is the request's `max_tokens`, at any effort.
    ///
    /// Regression guard for the removed reasoning-share subtraction: on this
    /// 128_000 envelope at `high`, the bar used to read `out 73.1K`.
    #[test]
    fn bottom_bar_out_is_the_wire_value_at_any_effort() {
        let render = |effort: Option<&str>| {
            let mut app = make_app();
            app.status_bar_mut().model_name = "mock-model".into();
            app.status_bar_mut().model_max_tokens = 128_000;
            app.status_bar_mut().model_reasoning_effort = effort.map(str::to_string);
            let backend = TestBackend::new(120, 2);
            let mut terminal = Terminal::new(backend).expect("terminal");
            terminal
                .draw(|frame| render_bottom_bar(frame, Rect::new(0, 0, 120, 2), &app))
                .expect("draw");
            buffer_text(terminal.backend().buffer())
        };

        // Same envelope, different thinking settings: `out` must not move, so
        // the segment cannot disagree with the request body either way.
        let none = render(None);
        let high = render(Some("high"));
        assert!(none.contains("out 128K"), "got:\n{none}");
        assert!(
            high.contains("out 128K") && high.contains("think high"),
            "got:\n{high}"
        );
        // 128k × 100/175 ≈ 73.1K was the old subtracted render.
        assert!(
            !high.contains("out 73.1K"),
            "out must not subtract a reasoning share, got:\n{high}"
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

    /// Width budget: row 2 with every segment populated must fit a 100-column
    /// terminal.
    ///
    /// The 2026-09-12 compaction got the full row down to 90 columns (from
    /// ~138) by removing the `∑ₜₒₖ` segment — whose value was a second format
    /// of the same `token_total` the ctx meter already renders — plus the
    /// `max_out_token`/`cache%`/`turns` word labels. Replacing the ctx gauge
    /// with the percentage the same day took it to 86. This test is the guard:
    /// if a future segment pushes the row past ~100 columns it will start
    /// silently dropping segments on ordinary terminals, which is the exact
    /// problem this compaction fixed. The task clock that row 1 gained on
    /// 2026-09-14 has its own guard in
    /// `bottom_bar_fits_the_task_elapsed_on_row_1_in_100_columns`.
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
            "out 128K",
            "think high",
            "ctx 4% 45K/1M",
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

    /// Width budget with the task clock on row 1: permission, path, uptime, the
    /// live elapsed and the branch all survive at 100 columns.
    ///
    /// The elapsed is the last droppable of the row (pushed after uptime), so it
    /// is the first segment to go on a narrower terminal — the row sheds the
    /// transient task clock before the session uptime and the cwd.
    #[test]
    fn bottom_bar_fits_the_task_elapsed_on_row_1_in_100_columns() {
        let mut app = make_app();
        app.workspace_dir = "/tmp/tact-ws".into();
        app.status_bar_mut().model_name = "deepseek-v4".into();
        app.status_bar_mut().git_branch = "main".into();
        app.task_start_time = Some(chrono::Local::now() - chrono::Duration::seconds(65));

        let backend = TestBackend::new(100, 2);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| render_bottom_bar(frame, Rect::new(0, 0, 100, 2), &app))
            .expect("draw");
        let text = buffer_text(terminal.backend().buffer());
        let row1 = text.lines().next().unwrap_or_default().to_string();

        for marker in ["/tmp/tact-ws", "Up", "Elapsed 01:05", "main"] {
            assert!(
                row1.contains(marker),
                "row 1 dropped {marker:?} at 100 columns (budget exceeded), got:\n{row1}"
            );
        }
    }

    /// Segment order on row 2: `ctx` → cache → turns → timing.
    ///
    /// Requested 2026-09-12: the cache percentage reads directly after the ctx
    /// meter (both are session-wide ratios), with the turn counters pushed after
    /// them. Position is asserted, not just presence — row 2 is built by pushing
    /// groups in survival order, so a reorder is invisible to the presence-only
    /// tests above, and `fit_row_spans` changes which of them survive at narrow
    /// widths.
    #[test]
    fn bottom_bar_orders_cache_before_turn_counters() {
        let mut app = make_app();
        app.status_bar_mut().model_name = "mock-model".into();
        app.status_bar_mut().token_total = 45_000;
        app.status_bar_mut().token_cache_hit = 30;
        app.status_bar_mut().token_cache_miss = 70;
        app.status_bar_mut().turn_user = 12;
        app.status_bar_mut().turn_llm = 3;
        app.status_bar_mut().turn_last_secs = Some(125);
        app.status_bar_mut().turn_done = 2;
        app.status_bar_mut().turn_total_secs = 210;
        app.model_context_window = 1_000_000;

        let backend = TestBackend::new(200, 2);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| render_bottom_bar(frame, Rect::new(0, 0, 200, 2), &app))
            .expect("draw");

        let text = buffer_text(terminal.backend().buffer());
        let row2 = text.lines().nth(1).unwrap_or_default().to_string();
        let pos = |needle: &str| {
            row2.find(needle)
                .unwrap_or_else(|| panic!("{needle:?} missing from row 2:\n{row2}"))
        };
        let ctx_at = pos("ctx ");
        let cache_at = pos("▣");
        let turn_at = pos("⟳");
        let timing_at = pos("⏱");
        assert!(ctx_at < cache_at, "cache must follow ctx, got:\n{row2}");
        assert!(
            cache_at < turn_at,
            "turn counters must follow cache, got:\n{row2}"
        );
        assert!(
            turn_at < timing_at,
            "turn timing must stay last, got:\n{row2}"
        );
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

    /// The live task elapsed renders on row 1, directly after the uptime.
    ///
    /// Moved there (from the top status bar) 2026-09-14: row 1 carries the two
    /// clocks that describe *this run* — the process uptime and the task
    /// elapsed — while row 2 keeps the token/ctx readouts and the frozen
    /// per-turn timing.
    #[test]
    fn bottom_bar_puts_live_elapsed_next_to_uptime_on_row_1() {
        let mut app = make_app();
        app.workspace_dir = "/tmp/tact-ws".into();
        app.status_bar_mut().model_name = "mock-model".into();
        app.status_bar_mut().git_branch = "main".into();
        app.task_start_time = Some(chrono::Local::now() - chrono::Duration::seconds(65));

        let backend = TestBackend::new(140, 2);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| render_bottom_bar(frame, Rect::new(0, 0, 140, 2), &app))
            .expect("draw");

        let text = buffer_text(terminal.backend().buffer());
        let row1 = text.lines().next().unwrap_or_default().to_string();
        let row2 = text.lines().nth(1).unwrap_or_default().to_string();
        let pos = |needle: &str| {
            row1.find(needle)
                .unwrap_or_else(|| panic!("{needle:?} missing from row 1:\n{row1}"))
        };
        let elapsed_at = pos("Elapsed 01:05");
        assert!(
            pos("Up 00:0") < elapsed_at,
            "the elapsed must follow the uptime, got:\n{row1}"
        );
        assert!(
            elapsed_at < pos("main"),
            "the elapsed must precede the branch, got:\n{row1}"
        );
        assert!(
            !row2.contains("Elapsed") && !row2.contains("01:05"),
            "the live clock belongs to row 1 only, got:\n{row2}"
        );
    }

    /// No task in flight → no elapsed segment on either row (`task_start_time`
    /// is what makes the label non-empty, and `make_app` leaves it `None`).
    #[test]
    fn bottom_bar_omits_live_elapsed_without_a_task() {
        let mut app = make_app();
        app.status_bar_mut().model_name = "mock-model".into();
        app.status_bar_mut().turn_user = 4;

        let backend = TestBackend::new(200, 2);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| render_bottom_bar(frame, Rect::new(0, 0, 200, 2), &app))
            .expect("draw");

        let text = buffer_text(terminal.backend().buffer());
        assert!(
            !text.contains("Elapsed") && !text.contains("耗时"),
            "no task is running, so no elapsed must render, got:\n{text}"
        );
    }

    /// Row-1 drop order with the task clock on the row: the transient elapsed
    /// goes first, then the session uptime, then the cwd.
    ///
    /// Probing a range of widths and comparing the first width each segment
    /// survives is robust; asserting on one fixed width is not.
    #[test]
    fn bottom_bar_drops_the_task_elapsed_before_uptime_and_path() {
        let mut app = make_app();
        app.workspace_dir = "/tmp/tact-ws".into();
        app.status_bar_mut().model_name = "mock-model".into();
        app.status_bar_mut().git_branch = "main".into();
        app.task_start_time = Some(chrono::Local::now() - chrono::Duration::seconds(65));

        let mut first_elapsed: Option<u16> = None;
        let mut first_uptime: Option<u16> = None;
        let mut first_path: Option<u16> = None;
        for width in 10u16..=140 {
            let backend = TestBackend::new(width, 2);
            let mut terminal = Terminal::new(backend).expect("terminal");
            terminal
                .draw(|frame| {
                    render_bottom_bar(frame, Rect::new(0, 0, width, 2), &app);
                })
                .expect("draw");
            let text = buffer_text(terminal.backend().buffer());
            if first_elapsed.is_none() && text.contains("Elapsed") {
                first_elapsed = Some(width);
            }
            if first_uptime.is_none() && text.contains("Up ") {
                first_uptime = Some(width);
            }
            if first_path.is_none() && text.contains("/tmp/tact-ws") {
                first_path = Some(width);
            }
        }

        let elapsed_w = first_elapsed.expect("elapsed segment never rendered at any probed width");
        let uptime_w = first_uptime.expect("uptime segment never rendered at any probed width");
        let path_w = first_path.expect("path segment never rendered at any probed width");
        assert!(
            elapsed_w > uptime_w,
            "the elapsed must drop before the uptime (elapsed at {elapsed_w}, uptime at {uptime_w})"
        );
        assert!(
            uptime_w > path_w,
            "the uptime must drop before the path (uptime at {uptime_w}, path at {path_w})"
        );
    }

    /// The status bar carries the step label only — no `[████░░] n%` gauge, no
    /// running clock (both left it on 2026-09-14; the clock moved to bottom-bar
    /// row 1, next to the uptime), and no denominator after the step number.
    #[test]
    fn status_bar_executing_shows_the_step_label_without_a_gauge() {
        let mut app = make_app();
        app.status_bar_mut().model_name = "mock-model".into();
        app.task_start_time = Some(chrono::Local::now() - chrono::Duration::seconds(65));
        app.status = Status::Executing {
            current_step: 1,
            total: 4,
        };

        let backend = TestBackend::new(120, 1);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| render_status_bar(frame, Rect::new(0, 0, 120, 1), &app))
            .expect("draw");

        let text = buffer_text(terminal.backend().buffer());
        assert!(
            text.contains("Executing step 1 "),
            "the step label must stay, got:\n{text}"
        );
        assert!(
            !text.contains("1/4") && !text.contains("step 1/"),
            "the step label must not carry the plan's denominator, got:\n{text}"
        );
        for banned in ['█', '░', '%', '⏱'] {
            assert!(
                !text.contains(banned),
                "{banned:?} must not render on the status bar, got:\n{text}"
            );
        }
    }

    /// Regression (2026-09-14): `status_idle_tmpl` carried three placeholders
    /// while the arm substituted four values, so `Log` landed in the 🎨 theme
    /// slot and the language label was dropped outright. The focus label is
    /// rendered by every `render_status_bar` arm; Idle keeps its own theme and
    /// language hints in their own slots.
    #[test]
    fn status_bar_idle_keeps_focus_theme_and_language_in_their_own_slots() {
        let app = make_app();

        let backend = TestBackend::new(120, 1);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| render_status_bar(frame, Rect::new(0, 0, 120, 1), &app))
            .expect("draw");

        // `buffer_text` reads the continuation cell of a wide glyph (`🎨`, `🌐`)
        // as a space, so each emoji is followed by one extra space. Collapse
        // whitespace runs before matching the one-space templates, otherwise
        // `🎨  Log` would satisfy a `!contains("🎨 Log")` guard even when the
        // focus label really did land in the theme slot.
        let text = buffer_text(terminal.backend().buffer());
        let norm = text.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(
            norm.contains("Log"),
            "the focused-panel label must render while idle, got:\n{norm}"
        );
        assert!(
            !norm.contains("🎨 Log"),
            "the focus label must not occupy the theme slot, got:\n{norm}"
        );
        assert!(
            norm.contains("🌐 EN"),
            "the language label must keep its own slot, got:\n{norm}"
        );
    }

    /// Same rule while planning: the spinner and the phase word, nothing else.
    #[test]
    fn status_bar_planning_has_no_elapsed() {
        let mut app = make_app();
        app.task_start_time = Some(chrono::Local::now() - chrono::Duration::seconds(65));
        app.status = Status::Planning;

        let backend = TestBackend::new(120, 1);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| render_status_bar(frame, Rect::new(0, 0, 120, 1), &app))
            .expect("draw");

        let text = buffer_text(terminal.backend().buffer());
        assert!(text.contains("Planning"), "got:\n{text}");
        assert!(
            !text.contains('⏱') && !text.contains("Elapsed"),
            "the live elapsed belongs to the bottom bar, got:\n{text}"
        );
    }
}
