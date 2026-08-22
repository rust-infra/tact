//! App-integration tests for the kit's `render_code_cards` (stay in tui — they
//! drive `App` and build a `RenderCtx`).

use ratatui::{Terminal, backend::TestBackend};
use tact_protocol::AgentUpdate;

use crate::render::test_harness::{buffer_text, make_app, render_log_panel_text};
use agent_tui_kit::render::cells::code::render_code_cards;

fn make_ctx(app: &crate::widgets::state::App) -> agent_tui_kit::render::ctx::RenderCtx<'_> {
    app.render_ctx()
}

#[test]
fn code_card_overlay_renders_language_and_body() {
    let mut app = make_app();
    app.handle_agent_update(AgentUpdate::StreamChunk(
        "```rust\nfn overlay_test() {}\n```\n".into(),
    ));
    assert!(!app.code_blocks.is_empty());

    let _ = render_log_panel_text(&mut app, 80, 18);

    let backend = TestBackend::new(80, 18);
    let mut terminal = Terminal::new(backend).expect("terminal");
    terminal
        .draw(|frame| {
            let area = frame.area();
            let ctx = make_ctx(&app);
            render_code_cards(frame, area, &ctx, 0, area.height as usize);
        })
        .expect("draw");

    let text = buffer_text(terminal.backend().buffer());
    assert!(
        text.contains("overlay_test"),
        "code overlay should render code body text, got:\n{text}"
    );
}

#[test]
fn code_card_starts_at_the_thinking_indent() {
    let mut app = make_app();
    app.handle_agent_update(AgentUpdate::StreamChunk(
        "```rust\nfn alignment_test() {}\n```\n".into(),
    ));
    let _ = render_log_panel_text(&mut app, 80, 18);

    let backend = TestBackend::new(80, 18);
    let mut terminal = Terminal::new(backend).expect("terminal");
    terminal
        .draw(|frame| {
            let area = frame.area();
            let ctx = make_ctx(&app);
            render_code_cards(frame, area, &ctx, 0, area.height as usize);
        })
        .expect("draw");

    let buffer = terminal.backend().buffer();
    let border_x = (0..buffer.area.height)
        .find_map(|y| {
            (0..buffer.area.width).find(|&x| matches!(buffer[(x, y)].symbol(), "╭" | "┌"))
        })
        .expect("code card top-left border");
    assert_eq!(
        border_x,
        agent_tui_kit::render::util::LOG_THINKING_INDENT + 1,
        "code card should align with the Thinking card inside the log border"
    );
}
