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
        app.status_bar.model_name = "mock-model".into();
        app.status_bar.token_total = 590;
        app.status_bar.token_prompt = 400;
        app.status_bar.token_completion = 190;

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
            row2.contains("mock-model")
                && row2.contains("ctx [")
                && row2.contains("590/200K")
                && row2.contains("%"),
            "row 2 should show model + labeled meter + ratio, got:\n{row2}"
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
        app.status_bar.model_name = "mock-model".into();
        app.status_bar.token_total = 42;
        app.workspace_dir = "/tmp/tact-ws".into();
        app.status_bar.git_branch = "main".into();

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
            row2.contains("∑ₜₒₖ 42"),
            "token stats should stay on row 2, got:\n{row2}"
        );
    }

    #[test]
    fn bottom_bar_shows_compact_model_with_limits() {
        let mut app = make_app();
        app.status_bar.model_name = "mock-model".into();
        app.status_bar.model_max_tokens = 128_000;
        app.status_bar.model_thinking_budget = Some(32_000);
        app.status_bar.model_reasoning_effort = Some("high".into());
        let backend = TestBackend::new(120, 2);
        let mut terminal = Terminal::new(backend).expect("terminal");

        terminal
            .draw(|frame| render_bottom_bar(frame, Rect::new(0, 0, 120, 2), &app))
            .expect("draw");

        let text = buffer_text(terminal.backend().buffer());
        assert!(
            text.contains("mock-model")
                && text.contains("max_out_token 128K")
                && text.contains("think high"),
            "bottom bar should show model + max_out_token + think effort, got:\n{text}"
        );
    }

    #[test]
    fn bottom_bar_shows_compact_model_when_effort_is_absent() {
        let mut app = make_app();
        app.status_bar.model_name = "mock-model".into();
        app.status_bar.model_max_tokens = 128_000;
        app.status_bar.model_thinking_budget = Some(32_000);
        let backend = TestBackend::new(120, 2);
        let mut terminal = Terminal::new(backend).expect("terminal");

        terminal
            .draw(|frame| render_bottom_bar(frame, Rect::new(0, 0, 120, 2), &app))
            .expect("draw");

        let text = buffer_text(terminal.backend().buffer());
        assert!(
            text.contains("max_out_token 128K") && text.contains("think 32K"),
            "bottom bar should show max_out_token/think without effort label, got:\n{text}"
        );
    }

    #[test]
    fn bottom_bar_subtracts_effort_share_from_max_out_tokens() {
        let mut app = make_app();
        app.status_bar.model_name = "mock-model".into();
        app.status_bar.model_max_tokens = 128_000;
        app.status_bar.model_reasoning_effort = Some("high".into());
        // 128k × 100/175 ≈ 73.1K — the reasoning share is subtracted from the
        // shared envelope for effort-semantic models.
        let backend = TestBackend::new(120, 2);
        let mut terminal = Terminal::new(backend).expect("terminal");

        terminal
            .draw(|frame| render_bottom_bar(frame, Rect::new(0, 0, 120, 2), &app))
            .expect("draw");

        let text = buffer_text(terminal.backend().buffer());
        assert!(
            text.contains("max_out_token 73.1K") && text.contains("think high"),
            "bottom bar should subtract the reasoning share, got:\n{text}"
        );
    }

    #[test]
    fn bottom_bar_drops_cache_before_model_on_narrow_width() {
        let mut app = make_app();
        app.status_bar.model_name = "mock-model".into();
        app.status_bar.model_max_tokens = 8_000;
        app.status_bar.model_thinking_budget = Some(32_000);
        app.status_bar.model_reasoning_effort = Some("high".into());
        app.status_bar.token_total = 100;
        app.status_bar.token_cache_hit = 50;
        app.status_bar.token_cache_miss = 50;
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
            !text.contains("cache%") && !text.contains("缓存%"),
            "cache segment should drop first on narrow width, got:\n{text}"
        );
    }
}
