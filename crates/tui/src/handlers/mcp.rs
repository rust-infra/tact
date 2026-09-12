//! `/mcp` slash command: authorize and inspect remote MCP servers.
//!
//! This module recognizes the `auth`/`login` and `list` forms and delegates the
//! work to the driver: the OAuth round-trip (loopback listener + token
//! persistence + router reload) needs to own the agent while it runs, and
//! `list` is answered there because only the driver can see the agent's live
//! MCP router.

use tact_protocol::UserCommand;

use super::CommandExecOutcome;
use crate::widgets::state::{App, Status};

pub(crate) fn handle_mcp_command(app: &mut App) -> CommandExecOutcome {
    let parts: Vec<&str> = app.input.split_whitespace().collect();
    match parts.as_slice() {
        // `login` is the CLI spelling (`tact-ui mcp login`); `auth` is kept so
        // either name works after following either set of docs.
        ["/mcp", "auth" | "login", server] => {
            let server = (*server).to_owned();
            let started = app.msgs().mcp_auth_started_tmpl.replace("{}", &server);
            app.add_system_message(started);
            let _ = app.user_cmd_tx.send(UserCommand::McpAuth { server });
            CommandExecOutcome {
                handled: true,
                clear_input: true,
            }
        }
        ["/mcp", "list"] => {
            // Idle-only: the driver serializes non-fast commands behind an
            // in-flight turn, so a busy agent would show the table only after
            // the turn finished — flash the busy hint instead, like `/compact`.
            if matches!(app.status, Status::Planning | Status::Executing { .. }) {
                let msg = app.msgs().input_busy_msg.to_string();
                app.flash_msg = Some((msg, std::time::Instant::now()));
            } else {
                let _ = app.user_cmd_tx.send(UserCommand::McpList);
            }
            CommandExecOutcome {
                handled: true,
                clear_input: true,
            }
        }
        _ => {
            // Bare `/mcp` (and any unknown subcommand) leaves the usage hint in
            // the input box so the user can complete the command.
            app.save_undo();
            app.input = "/mcp ".into();
            app.input_cursor = app.input.len();
            app.flash_msg = Some((app.msgs().mcp_usage.to_owned(), std::time::Instant::now()));
            CommandExecOutcome {
                handled: true,
                clear_input: false,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use tact_protocol::{AgentUpdate, UserCommand};
    use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

    use super::handle_mcp_command;
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
    fn mcp_auth_queues_an_authorization_request() {
        let (mut app, mut rx) = make_app();
        app.input = "/mcp auth hosted".into();

        let outcome = handle_mcp_command(&mut app);

        assert!(outcome.handled);
        assert!(outcome.clear_input);
        assert!(matches!(
            rx.try_recv().expect("command sent"),
            UserCommand::McpAuth { server } if server == "hosted"
        ));
    }

    #[test]
    fn mcp_login_is_an_alias_for_auth() {
        let (mut app, mut rx) = make_app();
        app.input = "/mcp login hosted".into();

        let outcome = handle_mcp_command(&mut app);

        assert!(outcome.handled);
        assert!(outcome.clear_input);
        assert!(matches!(
            rx.try_recv().expect("command sent"),
            UserCommand::McpAuth { server } if server == "hosted"
        ));
    }

    #[test]
    fn bare_mcp_shows_usage_and_keeps_editing() {
        let (mut app, mut rx) = make_app();
        app.input = "/mcp".into();

        let outcome = handle_mcp_command(&mut app);

        assert!(outcome.handled);
        assert!(!outcome.clear_input);
        assert_eq!(app.input, "/mcp ");
        assert!(rx.try_recv().is_err(), "no command should be sent");
    }

    #[test]
    fn mcp_list_queues_a_listing_request_when_idle() {
        let (mut app, mut rx) = make_app();
        app.input = "/mcp list".into();

        let outcome = handle_mcp_command(&mut app);

        assert!(outcome.handled);
        assert!(outcome.clear_input);
        assert!(matches!(
            rx.try_recv().expect("command sent"),
            UserCommand::McpList
        ));
    }

    #[test]
    fn mcp_list_flashes_busy_instead_of_queueing_while_a_task_runs() {
        let (mut app, mut rx) = make_app();
        app.input = "/mcp list".into();
        app.status = Status::Executing {
            current_step: 0,
            total: 1,
        };

        let outcome = handle_mcp_command(&mut app);

        assert!(outcome.handled);
        assert!(outcome.clear_input);
        assert!(
            rx.try_recv().is_err(),
            "a busy agent must not queue the listing"
        );
        assert!(app.flash_msg.is_some(), "the busy hint should be flashed");
    }
}
