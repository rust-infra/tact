//! `/mcp` slash command: authorize remote servers, inspect what is connected,
//! and run the prompts they publish.
//!
//! This module recognizes the `auth`/`login`, `list`, `prompts` and `prompt`
//! forms and delegates the work to the driver: the OAuth round-trip (loopback
//! listener + token persistence + router reload) needs to own the agent while it
//! runs, and `list` / `prompts` / `prompt` are answered there because only the
//! driver can see the agent's live MCP router.

use std::collections::BTreeMap;

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
        ["/mcp", "list"] => queue_when_idle(app, UserCommand::McpList),
        ["/mcp", "prompts"] => queue_when_idle(app, UserCommand::McpPrompts { server: None }),
        ["/mcp", "prompts", server] => queue_when_idle(
            app,
            UserCommand::McpPrompts {
                server: Some((*server).to_owned()),
            },
        ),
        // Bare `/mcp prompt` is the "I do not know the syntax yet" case, and it
        // gets the same hint every other bare subcommand does. Anything after it
        // is forwarded: the popup's sample input for a value-taking leaf is a
        // *single* token (`/mcp prompt sample`), and the driver is where a
        // missing name can be named alongside "no such prompt".
        ["/mcp", "prompt"] => usage(app),
        ["/mcp", "prompt", rest @ ..] => match parse_prompt_args(rest) {
            Ok((server, name, arguments)) => queue_when_idle(
                app,
                UserCommand::RunMcpPrompt {
                    server,
                    name,
                    arguments,
                },
            ),
            Err(token) => {
                let msg = app.msgs().mcp_prompt_argument_syntax.replace("{}", &token);
                app.flash_msg = Some((msg, std::time::Instant::now()));
                CommandExecOutcome {
                    handled: true,
                    clear_input: false,
                }
            }
        },
        _ => usage(app),
    }
}

/// Sends `cmd` when the agent is idle, or flashes the busy hint instead.
///
/// The driver serializes non-fast commands behind an in-flight turn, so a busy
/// agent would show the answer only after the turn finished — the hint beats a
/// late answer. `/compact` and `/mcp list` behave the same way.
fn queue_when_idle(app: &mut App, cmd: UserCommand) -> CommandExecOutcome {
    if matches!(app.status, Status::Planning | Status::Executing { .. }) {
        let msg = app.msgs().input_busy_msg.to_string();
        app.flash_msg = Some((msg, std::time::Instant::now()));
    } else {
        let _ = app.user_cmd_tx.send(cmd);
    }
    CommandExecOutcome {
        handled: true,
        clear_input: true,
    }
}

/// Bare `/mcp` (and any unknown subcommand): leave the usage hint in the input
/// box so the user can complete the command.
fn usage(app: &mut App) -> CommandExecOutcome {
    app.save_undo();
    app.input = "/mcp ".into();
    app.input_cursor = app.input.len();
    app.flash_msg = Some((app.msgs().mcp_usage.to_owned(), std::time::Instant::now()));
    CommandExecOutcome {
        handled: true,
        clear_input: false,
    }
}

/// Splits `/mcp prompt`'s tail into `(server, name, key=value …)`.
///
/// A token without `=` is refused by name: it is either a third positional
/// argument the syntax does not have or a mistyped pair, and guessing which
/// would silently drop a value the server needs. Values may contain `=` (the
/// split takes the first one) but not spaces — the input box has no quoting, and
/// inventing one would be a language; a multi-word value belongs in the model's
/// `get_mcp_prompt`, whose `arguments` is a JSON object.
///
/// A missing `server` or `name` is deliberately **not** an error here: see the
/// design note on arity — the driver names what is missing, next to the router
/// that can also say "no such prompt".
fn parse_prompt_args(
    tokens: &[&str],
) -> Result<(String, String, BTreeMap<String, String>), String> {
    let server = tokens.first().copied().unwrap_or_default().to_owned();
    let name = tokens.get(1).copied().unwrap_or_default().to_owned();
    let mut arguments = BTreeMap::new();
    for token in tokens.iter().skip(2) {
        match token.split_once('=') {
            Some((key, value)) if !key.is_empty() => {
                arguments.insert(key.to_owned(), value.to_owned());
            }
            _ => return Err((*token).to_owned()),
        }
    }
    Ok((server, name, arguments))
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

    #[test]
    fn mcp_prompts_queues_a_listing_request_when_idle() {
        let (mut app, mut rx) = make_app();
        app.input = "/mcp prompts".into();

        let outcome = handle_mcp_command(&mut app);

        assert!(outcome.handled);
        assert!(outcome.clear_input);
        assert!(matches!(
            rx.try_recv().expect("command sent"),
            UserCommand::McpPrompts { server: None }
        ));
    }

    #[test]
    fn mcp_prompts_can_name_one_server() {
        let (mut app, mut rx) = make_app();
        app.input = "/mcp prompts basic-memory".into();

        let outcome = handle_mcp_command(&mut app);

        assert!(outcome.handled);
        assert!(matches!(
            rx.try_recv().expect("command sent"),
            UserCommand::McpPrompts { server: Some(server) } if server == "basic-memory"
        ));
    }

    #[test]
    fn mcp_prompts_flashes_busy_instead_of_queueing_while_a_task_runs() {
        let (mut app, mut rx) = make_app();
        app.input = "/mcp prompts".into();
        app.status = Status::Planning;

        let outcome = handle_mcp_command(&mut app);

        assert!(outcome.handled);
        assert!(outcome.clear_input);
        assert!(
            rx.try_recv().is_err(),
            "a busy agent must not queue the listing"
        );
        assert!(app.flash_msg.is_some(), "the busy hint should be flashed");
    }

    #[test]
    fn mcp_prompt_queues_a_run_with_its_arguments() {
        let (mut app, mut rx) = make_app();
        app.input = "/mcp prompt basic-memory search_knowledge_base query=notes depth=2".into();

        let outcome = handle_mcp_command(&mut app);

        assert!(outcome.handled);
        assert!(outcome.clear_input);
        match rx.try_recv().expect("command sent") {
            UserCommand::RunMcpPrompt {
                server,
                name,
                arguments,
            } => {
                assert_eq!(server, "basic-memory");
                assert_eq!(name, "search_knowledge_base");
                assert_eq!(arguments.get("query").map(String::as_str), Some("notes"));
                assert_eq!(arguments.get("depth").map(String::as_str), Some("2"));
            }
            other => panic!("expected a prompt run, got {other:?}"),
        }
    }

    #[test]
    fn mcp_prompt_keeps_an_equals_inside_a_value() {
        // The split takes the *first* `=`, so a value that contains one survives.
        let (mut app, mut rx) = make_app();
        app.input = "/mcp prompt bm getting_started topic=a=b".into();

        let _ = handle_mcp_command(&mut app);

        match rx.try_recv().expect("command sent") {
            UserCommand::RunMcpPrompt { arguments, .. } => {
                assert_eq!(arguments.get("topic").map(String::as_str), Some("a=b"));
            }
            other => panic!("expected a prompt run, got {other:?}"),
        }
    }

    #[test]
    fn mcp_prompt_refuses_a_token_without_an_equals_sign() {
        // A bare third token is either a positional argument the syntax does not
        // have or a mistyped pair; guessing would silently drop a value.
        let (mut app, mut rx) = make_app();
        app.input = "/mcp prompt bm getting_started notes".into();

        let outcome = handle_mcp_command(&mut app);

        assert!(outcome.handled);
        assert!(
            !outcome.clear_input,
            "the input must survive so it can be fixed"
        );
        assert!(rx.try_recv().is_err(), "no command should be sent");
        let (msg, _) = app.flash_msg.as_ref().expect("a syntax hint");
        assert!(msg.contains("notes"), "the offending token is named: {msg}");
    }

    #[test]
    fn mcp_prompt_forwards_a_missing_name_for_the_driver_to_name() {
        // The popup's sample input for a value-taking leaf is a single token, so
        // refusing it here would turn a completable command into the usage hint
        // `every_declared_subcommand_has_a_handler` exists to catch. The driver
        // says which piece is missing.
        let (mut app, mut rx) = make_app();
        app.input = "/mcp prompt sample".into();

        let outcome = handle_mcp_command(&mut app);

        assert!(outcome.handled);
        assert!(outcome.clear_input);
        match rx.try_recv().expect("command sent") {
            UserCommand::RunMcpPrompt { server, name, .. } => {
                assert_eq!(server, "sample");
                assert!(name.is_empty());
            }
            other => panic!("expected a prompt run, got {other:?}"),
        }
    }

    #[test]
    fn bare_mcp_prompt_shows_usage_and_keeps_editing() {
        let (mut app, mut rx) = make_app();
        app.input = "/mcp prompt".into();

        let outcome = handle_mcp_command(&mut app);

        assert!(outcome.handled);
        assert!(!outcome.clear_input);
        assert_eq!(app.input, "/mcp ");
        assert!(rx.try_recv().is_err(), "no command should be sent");
    }
}
