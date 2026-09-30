//! `/hooks` slash command: review the command hooks Tac would run.
//!
//! A repository that ships `.tact/hooks.json`, or a plugin that ships
//! `hooks/hooks.json`, declares subprocesses Tact is asked to run on tool and
//! session events. None of them run until a human approves the exact
//! definition, and the only place to do that today is `tact-ui hooks …`. This
//! command puts the same review — and the same wording — where the hooks fire.
//!
//! Like `/mcp`, the work happens in the driver: the hook sources and the review
//! store (`~/.tact/hooks-state.json`) are only reachable from the Tact crate.
//! `list` is gated on an idle agent, because the driver serializes non-fast
//! commands behind an in-flight turn and a listing that arrives after a long
//! turn reads as unrelated to the command; `trust` and `forget` are state
//! changes that take effect next session, so they are allowed to queue.

use tact_protocol::UserCommand;

use super::CommandExecOutcome;
use crate::widgets::state::{App, Status};

pub(crate) fn handle_hooks_command(app: &mut App) -> CommandExecOutcome {
    // Parsed as a prefix, not by `split_whitespace`: a source label is free text
    // (`plugin ponytail`, `~/.tact/hooks.json`) and splitting would cut it in
    // half, so `--source <label>` could never select the hooks it names.
    let input = app.input.trim().to_string();
    let argument = input
        .strip_prefix("/hooks")
        .filter(|rest| rest.is_empty() || rest.starts_with(char::is_whitespace))
        .map(str::trim);

    let Some(argument) = argument else {
        return usage(app);
    };

    match argument {
        "" | "list" => {
            if matches!(app.status, Status::Planning | Status::Executing { .. }) {
                let msg = app.msgs().input_busy_msg.to_string();
                app.flash_msg = Some((msg, std::time::Instant::now()));
            } else {
                let _ = app.user_cmd_tx.send(UserCommand::HooksList);
            }
            CommandExecOutcome {
                handled: true,
                clear_input: true,
            }
        }
        // Approving is never implicit: `--all` or `--source <label>`, exactly
        // as the CLI requires. A bare `/hooks trust` reaches the usage hint
        // instead of approving everything.
        "trust --all" => {
            let _ = app.user_cmd_tx.send(UserCommand::HooksTrust {
                all: true,
                source: None,
            });
            CommandExecOutcome {
                handled: true,
                clear_input: true,
            }
        }
        other if other.starts_with("trust ") => {
            let Some(source) = other
                .trim_start_matches("trust")
                .trim()
                .strip_prefix("--source")
                .map(str::trim)
                .filter(|source| !source.is_empty())
            else {
                return usage(app);
            };
            let _ = app.user_cmd_tx.send(UserCommand::HooksTrust {
                all: false,
                source: Some(source.to_string()),
            });
            CommandExecOutcome {
                handled: true,
                clear_input: true,
            }
        }
        "forget --all" => {
            let _ = app.user_cmd_tx.send(UserCommand::HooksForget);
            CommandExecOutcome {
                handled: true,
                clear_input: true,
            }
        }
        _ => usage(app),
    }
}

/// Leaves the usage hint in the input box so the user can complete the command.
fn usage(app: &mut App) -> CommandExecOutcome {
    app.save_undo();
    app.input = "/hooks ".into();
    app.input_cursor = app.input.len();
    app.flash_msg = Some((app.msgs().hooks_usage.to_owned(), std::time::Instant::now()));
    CommandExecOutcome {
        handled: true,
        clear_input: false,
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use tact_protocol::{AgentUpdate, UserCommand};
    use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

    use super::handle_hooks_command;
    use crate::widgets::state::{App, Status};

    fn make_app() -> (App, UnboundedReceiver<UserCommand>) {
        let (_agent_tx, agent_rx) = unbounded_channel::<AgentUpdate>();
        let (user_cmd_tx, user_cmd_rx) = unbounded_channel();
        let (plugin_tx, _plugin_rx) = unbounded_channel();
        let (_plugin_event_tx, plugin_event_rx) = unbounded_channel();
        let (history_tx, _history_rx) = unbounded_channel();
        (
            App::new(
                agent_rx,
                None,
                plugin_event_rx,
                plugin_tx,
                user_cmd_tx,
                PathBuf::from("."),
                Vec::new(),
                "test-session".into(),
                history_tx,
                "retro".into(),
                String::new(),
                Vec::new(),
            ),
            user_cmd_rx,
        )
    }

    #[test]
    fn bare_hooks_lists_when_idle() {
        let (mut app, mut rx) = make_app();
        app.input = "/hooks".into();

        let outcome = handle_hooks_command(&mut app);

        assert!(outcome.handled);
        assert!(outcome.clear_input);
        assert!(matches!(
            rx.try_recv().expect("command sent"),
            UserCommand::HooksList
        ));
    }

    #[test]
    fn hooks_list_flashes_busy_instead_of_queueing_while_a_task_runs() {
        let (mut app, mut rx) = make_app();
        app.input = "/hooks list".into();
        app.status = Status::Executing {
            current_step: 0,
            total: 1,
        };

        let outcome = handle_hooks_command(&mut app);

        assert!(outcome.handled);
        assert!(outcome.clear_input);
        assert!(
            rx.try_recv().is_err(),
            "a busy agent must not queue the listing"
        );
        assert!(app.flash_msg.is_some(), "the busy hint should be flashed");
    }

    #[test]
    fn trust_all_queues_an_approval_of_everything() {
        let (mut app, mut rx) = make_app();
        app.input = "/hooks trust --all".into();

        handle_hooks_command(&mut app);

        assert!(matches!(
            rx.try_recv().expect("command sent"),
            UserCommand::HooksTrust {
                all: true,
                source: None
            }
        ));
    }

    #[test]
    fn trust_source_queues_a_narrowed_approval() {
        let (mut app, mut rx) = make_app();
        app.input = "/hooks trust --source plugin ponytail".into();

        handle_hooks_command(&mut app);

        assert!(matches!(
            rx.try_recv().expect("command sent"),
            UserCommand::HooksTrust { all: false, source: Some(source) }
                if source == "plugin ponytail"
        ));
    }

    #[test]
    fn trust_without_a_selector_only_shows_usage() {
        // Approving every hook by accident is exactly what the review step
        // exists to prevent, so a bare `trust` must not approve anything.
        let (mut app, mut rx) = make_app();
        app.input = "/hooks trust".into();

        let outcome = handle_hooks_command(&mut app);

        assert!(outcome.handled);
        assert!(!outcome.clear_input);
        assert_eq!(app.input, "/hooks ");
        assert!(rx.try_recv().is_err(), "no command should be sent");
        assert!(app.flash_msg.is_some(), "usage should be flashed");
    }

    #[test]
    fn forget_requires_an_explicit_all() {
        let (mut app, mut rx) = make_app();
        app.input = "/hooks forget".into();

        let outcome = handle_hooks_command(&mut app);

        assert!(!outcome.clear_input);
        assert!(rx.try_recv().is_err(), "no command should be sent");

        app.input = "/hooks forget --all".into();
        let outcome = handle_hooks_command(&mut app);
        assert!(outcome.clear_input);
        assert!(matches!(
            rx.try_recv().expect("command sent"),
            UserCommand::HooksForget
        ));
    }

    #[test]
    fn an_unknown_subcommand_shows_usage_and_keeps_editing() {
        let (mut app, mut rx) = make_app();
        app.input = "/hooks explain".into();

        let outcome = handle_hooks_command(&mut app);

        assert!(outcome.handled);
        assert!(!outcome.clear_input);
        assert_eq!(app.input, "/hooks ");
        assert!(rx.try_recv().is_err(), "no command should be sent");
    }
}
