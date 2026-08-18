use crossterm::event::{KeyCode, KeyEvent};
use tact_protocol::UserCommand;
use tokio::sync::mpsc::UnboundedSender;

use crate::widgets::state::{App, InputMode, Status};

fn sticky_scrollable(app: &App) -> bool {
    crate::render::task_panel::sticky_host_visible(app) && app.task_panel.expanded
}

pub(crate) fn handle_normal_mode(
    app: &mut App,
    key: KeyEvent,
    _user_cmd_tx: &UnboundedSender<UserCommand>,
) {
    match key.code {
        KeyCode::Char('j') => {
            if app.mouse.in_task_panel && sticky_scrollable(app) {
                app.task_panel.scroll = app.task_panel.scroll.saturating_add(1);
            } else {
                let step = crate::widgets::state::app::scroll::key_cell_step(
                    app.log_scroll.height as usize,
                );
                app.scroll_log_down(step);
            }
        }
        KeyCode::Char('k') => {
            if app.mouse.in_task_panel && sticky_scrollable(app) {
                if app.task_panel.scroll > 0 {
                    app.task_panel.scroll -= 1;
                }
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
            // Prefer character-level mouse selection over last message
            let text = if let Some(sel) = app.mouse.log_selection {
                let (start, end) = sel.normalized();
                Some(app.extract_selected_text(start, end))
            } else {
                // Last visible message
                let total = app.total_log_lines();
                if total > 0 && app.stream.buffer.is_empty() {
                    app.visible_message_index(total - 1)
                        .and_then(|idx| app.log.items.get(idx).map(|item| item.raw.clone()))
                } else if !app.stream.buffer.is_empty() {
                    Some(app.stream.buffer.clone())
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
                let _ = _user_cmd_tx.send(UserCommand::Cancel);
            }
        }
        KeyCode::Char('t') => {
            // Open the most recently visible thinking card popup
            if app.thinking.popup.is_some() {
                app.close_thinking_popup();
            } else if let Some(phys_idx) = app
                .thinking
                .active
                .as_ref()
                .map(|active| active.phys_idx)
                .or_else(|| app.thinking.blocks.last().map(|block| block.phys_idx))
            {
                app.open_thinking_popup(phys_idx);
            }
        }
        KeyCode::Char('q') => {
            app.should_quit = true;
        }
        KeyCode::Esc => {
            app.mouse.log_selection = None;
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
