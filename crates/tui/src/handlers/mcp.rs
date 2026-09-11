//! `/mcp` slash command: authorize remote MCP servers.
//!
//! Only `auth` is handled here — the actual OAuth round-trip (loopback
//! listener + token persistence + router reload) lives in the driver so it can
//! own the agent while it runs.

use tact_protocol::UserCommand;

use super::CommandExecOutcome;
use crate::widgets::state::App;

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
    use crate::widgets::state::App;

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
}
