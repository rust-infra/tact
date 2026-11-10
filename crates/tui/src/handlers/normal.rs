use crossterm::event::{KeyCode, KeyEvent};
use tact_protocol::UserCommand;
use tokio::sync::mpsc::UnboundedSender;

use super::{scroll_active_sticky, sticky_scrollable};
use crate::widgets::state::{App, InputMode, Status};

pub(crate) fn handle_normal_mode(
    app: &mut App,
    key: KeyEvent,
    _user_cmd_tx: &UnboundedSender<UserCommand>,
) {
    // Subagent view mode: j/k/G/g/y/Esc scroll / copy / close the view, and
    // Enter raises the cap on the tool blocks.
    if app.show_subagent_view {
        match key.code {
            // Scrolling up is a decision to stop following the child's tail
            // (the pill then offers the way back, exactly like the log's);
            // scrolling back down to the end re-arms it.
            KeyCode::Char('j') | KeyCode::Down => {
                let step = crate::widgets::state::app::scroll::key_cell_step(
                    app.mouse.popup_text_body_area.height as usize,
                ) as isize;
                app.subagent_scroll_by(step);
            }
            KeyCode::Char('k') | KeyCode::Up => {
                let step = crate::widgets::state::app::scroll::key_cell_step(
                    app.mouse.popup_text_body_area.height as usize,
                ) as isize;
                app.subagent_scroll_by(-step);
            }
            // `G` returns to the tail *and* resumes following it.
            KeyCode::Char('G') => {
                app.subagent_scroll_to_bottom();
            }
            KeyCode::Char('g') => {
                if let Some(popup) = app.subagent_popup_mut() {
                    popup.follow = false;
                    popup.unseen = false;
                    popup.scroll = 0;
                }
            }
            KeyCode::Enter => app.toggle_subagent_expanded(),
            KeyCode::Char('y') => app.copy_subagent_popup(),
            KeyCode::Esc => app.close_subagent_popup(),
            _ => {}
        }
        return;
    }

    match key.code {
        KeyCode::Char('j') => {
            if app.mouse.in_task_panel && sticky_scrollable(app) {
                scroll_active_sticky(app, 1);
            } else {
                app.scroll_log_down(crate::widgets::state::app::scroll::LOG_LINE_STEP);
            }
        }
        KeyCode::Char('k') => {
            if app.mouse.in_task_panel && sticky_scrollable(app) {
                scroll_active_sticky(app, -1);
            } else {
                app.scroll_log_up(crate::widgets::state::app::scroll::LOG_LINE_STEP);
            }
        }
        KeyCode::Char('g') => {
            app.scroll_log_to_top();
        }
        KeyCode::Char('G') => {
            app.scroll_log_to_bottom();
        }
        KeyCode::Char('/') => {
            app.input_mode = InputMode::Palette;
            app.cmd_line.clear();
            app.palette_selected = 0;
        }
        KeyCode::Enter => {
            app.input_mode = InputMode::Insert;
        }
        KeyCode::Char('i') => {
            app.input_mode = InputMode::Insert;
        }
        KeyCode::Char('y') => {
            if app.show_subagent_view {
                return;
            }
            // Prefer a virtual-card selection, then character-level log selection,
            // then fall back to the last visible row.
            let text =
                if let Some(text) = app.extract_card_selection().filter(|text| !text.is_empty()) {
                    Some(text)
                } else if let Some(sel) = app.mouse.log_selection {
                    let (start, end) = sel.normalized();
                    Some(app.extract_selected_text(start, end))
                } else {
                    // Last visible message
                    let total = app.total_log_lines();
                    if total > 0 && app.stream_mut().buffer.is_empty() {
                        app.visible_message_index(total - 1)
                            .and_then(|idx| app.log.items.get(idx).map(|item| item.raw.clone()))
                    } else if !app.stream_mut().buffer.is_empty() {
                        Some(app.stream_mut().buffer.clone())
                    } else {
                        None
                    }
                };
            if let Some(t) = text {
                app.copy_text(&t);
                app.add_new_line();
            }
        }
        KeyCode::Char('Y') => {
            if let Some(code) = app.extract_last_code_block() {
                app.copy_text(&code);
                app.add_new_line();
            }
        }
        KeyCode::Char('V') => {
            // Open the most visible code block popup
            if app.code_popup.is_some() {
                app.close_code_popup();
            } else if !app.code_blocks.is_empty() {
                let logical_offset = app.log_scroll.offset as usize;
                // Find the code block whose start_idx is closest to (and not exceeding) the current scroll position
                let best = app
                    .code_blocks
                    .iter()
                    .enumerate()
                    .rfind(|(_, block)| {
                        app.phys_to_logical_fast(block.start_idx)
                            .map(|l| l <= logical_offset)
                            .unwrap_or(false)
                    })
                    .or_else(|| app.code_blocks.iter().enumerate().next_back());
                if let Some((idx, _)) = best {
                    app.open_code_popup(idx);
                }
            }
        }
        KeyCode::Char('c') => {
            // Same gate as `/cancel`: only Planning / Executing. Queued
            // (pending) messages are NOT touched — dropping them is the
            // `[Cancel]` button's job.
            if matches!(app.status, Status::Planning | Status::Executing { .. }) {
                app.cancel_task();
            }
        }
        KeyCode::Char('t') => {
            // Open the most recently visible thinking card popup
            if app.thinking_mut().popup.is_some() {
                app.close_thinking_popup();
            } else if let Some(phys_idx) = app
                .thinking_mut()
                .active
                .as_ref()
                .map(|active| active.phys_idx)
                .or_else(|| app.thinking_mut().blocks.last().map(|block| block.phys_idx))
            {
                app.open_thinking_popup_at_physical_index(phys_idx);
            }
        }
        KeyCode::Char('z') => {
            // Fold or unfold the first tool run in view. `z` is the fold verb in
            // every editor this keymap borrows from. The target is the first run
            // the reader can see, not the row at the viewport top: that row is
            // usually prose or a separator, and expanding a run moves its
            // summary off the top anyway.
            app.toggle_fold_in_view();
        }
        KeyCode::Char('q') => {
            app.should_quit = true;
        }
        KeyCode::Esc => {
            // Same ladder as Insert mode: a log scrolled away from its tail is
            // a view state, so `esc` returns to the bottom first and only
            // clears the selection once the reader is back at the tail.
            if !app.is_log_pinned_to_bottom() {
                app.scroll_log_to_bottom();
            } else {
                app.mouse.log_selection = None;
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use tokio::sync::mpsc::unbounded_channel;

    use super::*;
    use crate::{
        render::test_harness::{make_app, render_log_panel_text},
        test_fixtures::StepCall,
        widgets::state::{LogSelection, TextPosition},
    };

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::empty())
    }

    /// Seed one finished tool call at `idx`.
    fn settled_tool(app: &mut App, idx: usize, id: &str, arg: &str) {
        use std::collections::HashMap;

        use tact_protocol::{AgentUpdate, PlanStep, StepStatus};

        app.handle_agent_update(AgentUpdate::StepAdded(PlanStep::new(
            format!("step {id}"),
            "read_file",
            id,
            HashMap::from([("arg".to_string(), arg.to_string())]),
        )));
        app.handle_agent_update(StepCall::new(idx, id, "read_file", arg).started());
        app.handle_agent_update(
            StepCall::new(idx, id, "read_file", arg)
                .detail("out")
                .status(StepStatus::Success)
                .duration_us(1_200)
                .finished(),
        );
    }

    /// `z` is the reader's only keyboard route into a fold, so it has to reach
    /// the run under the viewport top and nothing else.
    #[test]
    fn z_folds_and_unfolds_the_run_at_the_viewport_top() {
        let mut app = make_app();
        let (tx, _rx) = unbounded_channel();
        settled_tool(&mut app, 0, "r1", "a.rs");
        settled_tool(&mut app, 1, "r2", "b.rs");
        settled_tool(&mut app, 2, "r3", "c.rs");
        app.input_mode = InputMode::Normal;

        let folded = render_log_panel_text(&mut app, 80, 20);
        assert!(folded.contains("3 tools"), "starts folded: {folded}");

        handle_normal_mode(&mut app, key(KeyCode::Char('z')), &tx);
        let open = render_log_panel_text(&mut app, 80, 20);
        assert!(
            open.contains("b.rs") && open.contains("c.rs"),
            "`z` at the run head must reveal every member: {open}"
        );

        handle_normal_mode(&mut app, key(KeyCode::Char('z')), &tx);
        let again = render_log_panel_text(&mut app, 80, 20);
        assert!(
            again.contains("3 tools") && !again.contains("b.rs"),
            "`z` is a toggle, so a second press folds it back: {again}"
        );
    }

    /// A log with no tool run at all folds nothing, and says so by not bumping
    /// the fold generation: `z` over pure prose is not a toggle.
    ///
    /// The companion rule — a run that is merely *below* the viewport top — is
    /// no longer inert: `z` takes the first run the reader can see, because a
    /// rule keyed on the top row found the paragraph instead of the run and
    /// made the key silently do nothing in every real transcript.
    #[test]
    fn z_does_nothing_when_there_is_no_run_to_fold() {
        let mut app = make_app();
        let (tx, _rx) = unbounded_channel();
        app.handle_agent_update(tact_protocol::AgentUpdate::StreamChunk(
            "just prose.\n".into(),
        ));
        app.input_mode = InputMode::Normal;

        let before = app.tools().state().fold_rev;
        handle_normal_mode(&mut app, key(KeyCode::Char('z')), &tx);
        assert_eq!(
            before,
            app.tools().state().fold_rev,
            "a log with no run must not bump the fold generation"
        );
    }

    /// `z` finds the first run below the prose, and folds it. The ordering is
    /// what makes the key usable: the reader scrolled to a turn and pressed the
    /// fold verb, not "point at the exact row".
    #[test]
    fn z_folds_the_first_run_below_plain_markdown() {
        let mut app = make_app();
        let (tx, _rx) = unbounded_channel();
        app.handle_agent_update(tact_protocol::AgentUpdate::StreamChunk(
            "just prose.\n\n".into(),
        ));
        settled_tool(&mut app, 0, "r1", "a.rs");
        settled_tool(&mut app, 1, "r2", "b.rs");
        app.input_mode = InputMode::Normal;

        handle_normal_mode(&mut app, key(KeyCode::Char('z')), &tx);
        let text = render_log_panel_text(&mut app, 100, 24);
        assert!(
            text.contains("a.rs") && text.contains("b.rs"),
            "the run below the prose must expand: {text}"
        );

        handle_normal_mode(&mut app, key(KeyCode::Char('z')), &tx);
        let again = render_log_panel_text(&mut app, 100, 24);
        assert!(
            again.contains("2 tools") && !again.contains("b.rs"),
            "and the next press folds it back: {again}"
        );
    }

    /// The user-visible half of the keyboard-focus fix: after opening the view,
    /// a real `G` keypress must reach the view's handler and jump to the tail.
    /// Before `open_subagent_popup_by_id` took the keyboard, the session was
    /// still in Insert, so this key typed into the input box and the view never
    /// moved.
    #[test]
    fn g_jumps_the_open_subagent_view_to_its_tail() {
        let mut app = make_app();
        let (tx, _rx) = unbounded_channel();
        crate::render::test_harness::seed_subagent_view(&mut app);
        let tool_id = "sa-1".to_string();
        app.open_subagent_popup_by_id(&tool_id);
        assert!(app.has_subagent_popup(), "the view must be open");
        assert!(matches!(app.input_mode, InputMode::Normal));

        // A short viewport, so the seeded conversation overflows and scrolling
        // is meaningful; without a render the body height is still zero.
        crate::render::test_harness::render_main_area_terminal(&mut app, 100, 12);
        app.subagent_scroll_by(-1000);
        assert!(!app.subagent_popup().expect("view").follow);
        handle_normal_mode(&mut app, key(KeyCode::Char('G')), &tx);

        let popup = app.subagent_popup().expect("view");
        assert!(popup.follow, "`G` must re-arm following");
        assert_eq!(popup.scroll, u16::MAX, "and pin to the tail");
    }

    #[test]
    fn slash_enters_palette_mode() {
        let mut app = make_app();
        let (tx, _rx) = unbounded_channel();

        handle_normal_mode(&mut app, key(KeyCode::Char('/')), &tx);

        assert!(matches!(app.input_mode, InputMode::Palette));
        assert_eq!(app.palette_selected, 0);
    }

    #[test]
    fn enter_enters_insert_mode() {
        let mut app = make_app();
        let (tx, _rx) = unbounded_channel();
        app.input_mode = InputMode::Normal;

        handle_normal_mode(&mut app, key(KeyCode::Enter), &tx);

        assert!(matches!(app.input_mode, InputMode::Insert));
    }

    /// The view's branch returns before the normal bindings, so `Enter` raises
    /// the cap on the child's tool blocks (all of them — the per-block gesture
    /// is a click) instead of focusing the input box.
    #[test]
    fn enter_in_the_subagent_view_toggles_expanded_blocks() {
        let mut app = make_app();
        crate::render::test_harness::seed_subagent_view(&mut app);
        let (tx, _rx) = unbounded_channel();
        let mode_before = app.input_mode;
        assert!(
            app.subagent_popup()
                .expect("view is open")
                .expanded_steps
                .is_empty()
        );

        handle_normal_mode(&mut app, key(KeyCode::Enter), &tx);

        assert!(
            app.subagent_popup()
                .expect("view is open")
                .expanded_steps
                .contains("c1"),
            "every step is expanded"
        );
        assert_eq!(
            app.input_mode, mode_before,
            "Enter must not reach the input box while the view is up"
        );

        // And back: the bulk gesture is a toggle, not an expand.
        handle_normal_mode(&mut app, key(KeyCode::Enter), &tx);
        assert!(
            app.subagent_popup()
                .expect("view is open")
                .expanded_steps
                .is_empty()
        );
    }

    #[test]
    fn q_sets_should_quit() {
        let mut app = make_app();
        let (tx, _rx) = unbounded_channel();

        handle_normal_mode(&mut app, key(KeyCode::Char('q')), &tx);

        assert!(app.should_quit);
    }

    #[test]
    fn s_is_unbound_noop_key() {
        use std::path::PathBuf;

        use tact_protocol::{AgentUpdate, UserCommand};

        use crate::widgets::state::Status;

        let (_agent_tx, agent_rx) = unbounded_channel::<AgentUpdate>();
        let (user_cmd_tx, mut user_cmd_rx) = unbounded_channel::<UserCommand>();
        let (plugin_tx, _plugin_request_rx) = unbounded_channel();
        let (_plugin_event_tx, plugin_rx) = unbounded_channel();
        let (history_tx, _history_rx) = unbounded_channel();
        let mut app = App::new(
            agent_rx,
            None,
            plugin_rx,
            plugin_tx,
            user_cmd_tx.clone(),
            PathBuf::from("."),
            Vec::new(),
            "test-session".to_string(),
            history_tx,
            "retro".to_string(),
            String::new(),
            Vec::new(),
        );
        app.status = Status::Executing {
            current_step: 0,
            total: 1,
        };
        app.queue_pending_message("urgent".into(), "urgent".into());

        // `s` is not a bound key: it must not send or drop anything.
        handle_normal_mode(&mut app, key(KeyCode::Char('s')), &user_cmd_tx);

        assert_eq!(app.pending_messages.len(), 1, "s must not touch the queue");
        assert!(
            user_cmd_rx.try_recv().is_err(),
            "s must not dispatch anything"
        );
    }

    #[test]
    fn c_cancels_while_executing() {
        use std::path::PathBuf;

        use tact_protocol::{AgentUpdate, UserCommand};

        use crate::widgets::state::Status;

        let (_agent_tx, agent_rx) = unbounded_channel::<AgentUpdate>();
        let (user_cmd_tx, mut user_cmd_rx) = unbounded_channel::<UserCommand>();
        let (plugin_tx, _plugin_request_rx) = unbounded_channel();
        let (_plugin_event_tx, plugin_rx) = unbounded_channel();
        let (history_tx, _history_rx) = unbounded_channel();
        let mut app = App::new(
            agent_rx,
            None,
            plugin_rx,
            plugin_tx,
            user_cmd_tx.clone(),
            PathBuf::from("."),
            Vec::new(),
            "test-session".to_string(),
            history_tx,
            "retro".to_string(),
            String::new(),
            Vec::new(),
        );
        app.status = Status::Executing {
            current_step: 0,
            total: 1,
        };
        app.queue_pending_message("queued".into(), "queued".into());

        handle_normal_mode(&mut app, key(KeyCode::Char('c')), &user_cmd_tx);

        assert!(matches!(
            user_cmd_rx.try_recv().expect("expected Cancel"),
            UserCommand::Cancel
        ));
        assert_eq!(
            app.pending_messages.len(),
            1,
            "Normal-mode c must NOT drop queued (pending) messages — that is the [Cancel] button's job"
        );
    }

    #[test]
    fn c_noop_while_done() {
        use std::path::PathBuf;

        use tact_protocol::{AgentUpdate, UserCommand};

        use crate::widgets::state::Status;

        let (_agent_tx, agent_rx) = unbounded_channel::<AgentUpdate>();
        let (user_cmd_tx, mut user_cmd_rx) = unbounded_channel::<UserCommand>();
        let (plugin_tx, _plugin_request_rx) = unbounded_channel();
        let (_plugin_event_tx, plugin_rx) = unbounded_channel();
        let (history_tx, _history_rx) = unbounded_channel();
        let mut app = App::new(
            agent_rx,
            None,
            plugin_rx,
            plugin_tx,
            user_cmd_tx.clone(),
            PathBuf::from("."),
            Vec::new(),
            "test-session".to_string(),
            history_tx,
            "retro".to_string(),
            String::new(),
            Vec::new(),
        );
        app.status = Status::Done;

        handle_normal_mode(&mut app, key(KeyCode::Char('c')), &user_cmd_tx);

        assert!(
            user_cmd_rx.try_recv().is_err(),
            "Done must not dispatch Cancel via Normal-mode c"
        );
    }

    #[test]
    fn j_and_k_scroll_log() {
        let mut app = make_app();
        let (tx, _rx) = unbounded_channel();
        for i in 0..10 {
            app.add_system_message(format!("row-{i}"));
        }
        // One render populates the visual caches the scroll handlers read.
        let _ = crate::render::test_harness::render_log_panel_text(&mut app, 60, 4);
        app.scroll_log_to_top();

        handle_normal_mode(&mut app, key(KeyCode::Char('j')), &tx);
        assert_eq!(app.log_scroll.offset, 1);
        handle_normal_mode(&mut app, key(KeyCode::Char('j')), &tx);
        assert_eq!(app.log_scroll.offset, 2);

        handle_normal_mode(&mut app, key(KeyCode::Char('k')), &tx);
        assert_eq!(app.log_scroll.offset, 1);
    }

    #[test]
    fn y_copies_a_virtual_card_selection() {
        let mut app = make_app();
        app.handle_agent_update(tact_protocol::AgentUpdate::ThinkingChunk(
            tact_protocol::ThinkingChunk::Delta("one\ntwo\n".into()),
        ));
        app.handle_agent_update(tact_protocol::AgentUpdate::ThinkingChunk(
            tact_protocol::ThinkingChunk::Finished,
        ));
        app.mouse.card_selection = Some(crate::widgets::state::CardSelection::full(
            app.thinking().blocks[0].phys_idx,
            app.thinking().blocks[0].content.clone(),
        ));
        let (tx, _rx) = unbounded_channel();

        handle_normal_mode(&mut app, key(KeyCode::Char('y')), &tx);

        assert!(app.log.items.iter().any(|item| item.raw.contains("one")));
    }

    #[test]
    fn y_copies_partial_line_selection() {
        let mut app = make_app();
        app.add_system_message("hello world".into());
        app.mouse.log_selection = Some(LogSelection::new(
            TextPosition::new(0, 6),
            TextPosition::new(0, 11), // "world"
        ));
        let (tx, _rx) = unbounded_channel();

        handle_normal_mode(&mut app, key(KeyCode::Char('y')), &tx);

        assert!(app.log.items.iter().any(|item| item.raw.contains("world")));
    }

    #[test]
    fn y_copies_multi_line_selection() {
        let mut app = make_app();
        app.add_system_message("first line".into());
        app.add_system_message("second line".into());
        app.mouse.log_selection = Some(LogSelection::new(
            TextPosition::new(0, 6),
            TextPosition::new(1, 6), // "line\nsecond line"
        ));
        let (tx, _rx) = unbounded_channel();

        handle_normal_mode(&mut app, key(KeyCode::Char('y')), &tx);

        assert!(
            app.log
                .items
                .iter()
                .any(|item| item.raw.contains("second line"))
        );
    }

    /// The user-visible half of the short-log fix: pressing `k` when there is
    /// nothing to scroll must not put a "Back to bottom" pill on the screen.
    ///
    /// The state-level test pins `follow`; this one goes through the real
    /// keypress and the real render, because the pill is what the reader
    /// actually sees and it is drawn from that flag.
    #[test]
    fn k_in_a_short_log_draws_no_scroll_back_pill() {
        let mut app = make_app();
        app.append_markdown("only line");
        let _ = crate::render::test_harness::render_main_area_text(&mut app, 80, 20);
        let (tx, _rx) = unbounded_channel();

        handle_normal_mode(&mut app, key(KeyCode::Char('k')), &tx);

        let text = crate::render::test_harness::render_main_area_text(&mut app, 80, 20);
        assert!(
            !text.contains("Back to bottom"),
            "a keypress that moved nothing must not offer a way back to where \
             the reader already is:\n{text}"
        );
        assert!(
            app.scroll_back_area.is_empty(),
            "and must not leave a click target behind"
        );
    }
}
