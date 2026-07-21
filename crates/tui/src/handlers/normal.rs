use crate::widgets::state::{App, FocusedPanel, InputMode, Status};
use crossterm::event::{KeyCode, KeyEvent};
use tact_protocol::UserCommand;
use tokio::sync::mpsc::UnboundedSender;

pub(crate) fn handle_normal_mode(
    app: &mut App,
    key: KeyEvent,
    _user_cmd_tx: &UnboundedSender<UserCommand>,
) {
    match key.code {
        KeyCode::Tab => {
            app.focused_panel = match app.focused_panel {
                FocusedPanel::Log => FocusedPanel::Plan,
                FocusedPanel::Plan => FocusedPanel::Log,
            };
        }
        KeyCode::Char('e') => {
            app.plan.visible = !app.plan.visible;
            if !app.plan.visible && app.focused_panel == FocusedPanel::Plan {
                app.focused_panel = FocusedPanel::Log;
            }
        }
        KeyCode::Char('j') => match app.focused_panel {
            FocusedPanel::Log => {
                // Don't check upper bound; render uniformly clamps
                app.log_scroll.offset = app.log_scroll.offset.saturating_add(1);
            }
            FocusedPanel::Plan => {
                if !app.plan.steps.is_empty() && app.plan.selected + 1 < app.plan.steps.len() {
                    app.plan.selected += 1;
                    app.plan.list_state.select(Some(app.plan.selected));
                }
            }
        },
        KeyCode::Char('k') => match app.focused_panel {
            FocusedPanel::Log => {
                if app.log_scroll.offset > 0 {
                    app.log_scroll.offset -= 1;
                }
            }
            FocusedPanel::Plan => {
                if app.plan.selected > 0 {
                    app.plan.selected -= 1;
                    app.plan.list_state.select(Some(app.plan.selected));
                }
            }
        },
        KeyCode::Char('g') => {
            if app.focused_panel == FocusedPanel::Log {
                app.log_scroll.offset = 0;
            }
        }
        KeyCode::Char('G') => {
            if app.focused_panel == FocusedPanel::Log {
                // Set to a large enough value; render clamps to actual max_scroll
                app.log_scroll.offset = u16::MAX;
            }
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
            let text = match app.focused_panel {
                FocusedPanel::Plan => {
                    if let Some((s, e)) = app.mouse.plan_selection {
                        let start = s.min(e);
                        let end = s.max(e);
                        if start < app.plan.steps.len() {
                            let selected: Vec<String> = app.plan.steps
                                [start..=end.min(app.plan.steps.len().saturating_sub(1))]
                                .iter()
                                .map(|step| step.description.clone())
                                .collect();
                            Some(selected.join("\n"))
                        } else {
                            None
                        }
                    } else {
                        app.plan
                            .steps
                            .get(app.plan.selected)
                            .map(|s| s.description.clone())
                    }
                }
                FocusedPanel::Log => {
                    // Prefer character-level mouse selection over last message
                    if let Some(sel) = app.mouse.log_selection {
                        let (start, end) = sel.normalized();
                        Some(app.extract_selected_text(start, end))
                    } else {
                        // Last visible message
                        let total = app.total_log_lines();
                        if total > 0 && app.stream.buffer.is_empty() {
                            app.visible_message_index(total - 1)
                                .and_then(|idx| app.raw_messages.get(idx).cloned())
                        } else if !app.stream.buffer.is_empty() {
                            Some(app.stream.buffer.clone())
                        } else {
                            None
                        }
                    }
                }
            };
            if let Some(t) = text {
                app.copy_text(&t);
                app.add_new_line();
            }
        }
        KeyCode::Char('Y') => {
            if app.focused_panel == FocusedPanel::Log
                && let Some(code) = app.extract_last_code_block()
            {
                app.copy_text(&code);
                app.add_new_line();
            }
        }
        KeyCode::Char('V') => {
            // Open the most visible code block popup
            if app.code_popup.is_some() {
                app.close_code_popup();
            } else if !app.code_blocks.is_empty() && app.focused_panel == FocusedPanel::Log {
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
            // Same gate as `/cancel`: only Planning / Executing.
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
            app.mouse.plan_selection = None;
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::test_harness::make_app;
    use crate::widgets::state::{LogSelection, TextPosition};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use tokio::sync::mpsc::unbounded_channel;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::empty())
    }

    #[test]
    fn tab_toggles_focus_between_log_and_plan() {
        let mut app = make_app();
        let (tx, _rx) = unbounded_channel();
        assert!(matches!(app.focused_panel, FocusedPanel::Log));

        handle_normal_mode(&mut app, key(KeyCode::Tab), &tx);
        assert!(matches!(app.focused_panel, FocusedPanel::Plan));

        handle_normal_mode(&mut app, key(KeyCode::Tab), &tx);
        assert!(matches!(app.focused_panel, FocusedPanel::Log));
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
    fn e_toggles_plan_panel_visibility() {
        let mut app = make_app();
        let (tx, _rx) = unbounded_channel();
        app.plan.visible = true;

        handle_normal_mode(&mut app, key(KeyCode::Char('e')), &tx);
        assert!(!app.plan.visible);

        handle_normal_mode(&mut app, key(KeyCode::Char('e')), &tx);
        assert!(app.plan.visible);
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
    fn c_cancels_while_executing() {
        use crate::widgets::state::Status;
        use std::path::PathBuf;
        use tact_protocol::{AgentUpdate, UserCommand};

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

        handle_normal_mode(&mut app, key(KeyCode::Char('c')), &user_cmd_tx);

        assert!(matches!(
            user_cmd_rx.try_recv().expect("expected Cancel"),
            UserCommand::Cancel
        ));
    }

    #[test]
    fn c_noop_while_done() {
        use crate::widgets::state::Status;
        use std::path::PathBuf;
        use tact_protocol::{AgentUpdate, UserCommand};

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
    fn j_and_k_scroll_log_when_log_focused() {
        let mut app = make_app();
        let (tx, _rx) = unbounded_channel();
        app.focused_panel = FocusedPanel::Log;
        app.log_scroll.offset = 5;

        handle_normal_mode(&mut app, key(KeyCode::Char('j')), &tx);
        assert_eq!(app.log_scroll.offset, 6);

        handle_normal_mode(&mut app, key(KeyCode::Char('k')), &tx);
        assert_eq!(app.log_scroll.offset, 5);
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

        assert!(app.raw_messages.iter().any(|m| m.contains("world")));
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

        assert!(app.raw_messages.iter().any(|m| m.contains("second line")));
    }
}
