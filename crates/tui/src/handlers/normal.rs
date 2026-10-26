use crossterm::event::{KeyCode, KeyEvent};
use tact_protocol::DriverMessage;
use tokio::sync::mpsc::UnboundedSender;

use super::{scroll_active_sticky, sticky_scrollable};
use crate::widgets::state::{App, InputMode, Status};

pub(crate) fn handle_normal_mode(
    app: &mut App,
    key: KeyEvent,
    _user_cmd_tx: &UnboundedSender<DriverMessage>,
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
                let step = crate::widgets::state::app::scroll::key_cell_step(
                    app.log_scroll.height as usize,
                );
                app.scroll_log_down(step);
            }
        }
        KeyCode::Char('k') => {
            if app.mouse.in_task_panel && sticky_scrollable(app) {
                scroll_active_sticky(app, -1);
            } else {
                let step = crate::widgets::state::app::scroll::key_cell_step(
                    app.log_scroll.height as usize,
                );
                app.scroll_log_up(step);
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
        render::test_harness::make_app,
        widgets::state::{LogSelection, TextPosition},
    };

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::empty())
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

        use tact_protocol::{AgentUpdate, DriverMessage};

        use crate::widgets::state::Status;

        let (_agent_tx, agent_rx) = unbounded_channel::<AgentUpdate>();
        let (user_cmd_tx, mut user_cmd_rx) = unbounded_channel::<DriverMessage>();
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

        use tact_protocol::{AgentUpdate, DriverMessage};

        use crate::widgets::state::Status;

        let (_agent_tx, agent_rx) = unbounded_channel::<AgentUpdate>();
        let (user_cmd_tx, mut user_cmd_rx) = unbounded_channel::<DriverMessage>();
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
            DriverMessage::Cancel
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

        use tact_protocol::{AgentUpdate, DriverMessage};

        use crate::widgets::state::Status;

        let (_agent_tx, agent_rx) = unbounded_channel::<AgentUpdate>();
        let (user_cmd_tx, mut user_cmd_rx) = unbounded_channel::<DriverMessage>();
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
}
