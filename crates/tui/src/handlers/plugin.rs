use tact::plugin::PluginRequest;

use super::CommandExecOutcome;
use crate::widgets::state::App;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PluginUsageError;

pub(crate) fn parse_plugin_command(input: &str) -> Result<PluginRequest, PluginUsageError> {
    let parts: Vec<&str> = input.split_whitespace().collect();
    match parts.as_slice() {
        ["/plugin", "list"] => Ok(PluginRequest::List),
        ["/plugin", "reload"] => Ok(PluginRequest::Reload),
        ["/plugin", "marketplace", "list"] => Ok(PluginRequest::MarketplaceList),
        _ => Err(PluginUsageError),
    }
}

pub(crate) fn handle_plugin_command(app: &mut App) -> CommandExecOutcome {
    let trimmed = app.input.trim();
    // Bare `/plugin` is not an error — leave `/plugin ` in the insert box so the
    // user can type list / reload / marketplace list.
    if trimmed == "/plugin" {
        app.save_undo();
        app.input = "/plugin ".into();
        app.input_cursor = app.input.len();
        app.flash_msg = Some((app.msgs().plugin_usage.to_owned(), std::time::Instant::now()));
        return CommandExecOutcome { handled: true, clear_input: false };
    }
    match parse_plugin_command(&app.input) {
        Ok(request) => match app.plugin_tx.send(request) {
            Ok(()) => app.add_system_message(app.msgs().plugin_request_queued.to_owned()),
            Err(_) => app.add_system_message(app.msgs().plugin_worker_unavailable.to_owned()),
        },
        Err(_) => app.add_system_message(app.msgs().plugin_usage.to_owned()),
    }
    CommandExecOutcome { handled: true, clear_input: true }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use tact::plugin::PluginRequest;
    use tact_protocol::AgentUpdate;
    use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

    use super::{handle_plugin_command, parse_plugin_command};
    use crate::{i18n::Language, widgets::state::App};

    fn make_app() -> (App, UnboundedReceiver<PluginRequest>) {
        let (_agent_tx, agent_rx) = unbounded_channel::<AgentUpdate>();
        let (user_cmd_tx, _user_cmd_rx) = unbounded_channel();
        let (plugin_tx, plugin_rx) = unbounded_channel();
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
            plugin_rx,
        )
    }

    #[test]
    fn parses_only_exact_plugin_forms() {
        assert!(matches!(parse_plugin_command("/plugin list"), Ok(PluginRequest::List)));
        assert!(matches!(parse_plugin_command("/plugin reload"), Ok(PluginRequest::Reload)));
        assert!(matches!(parse_plugin_command("/plugin marketplace list"), Ok(PluginRequest::MarketplaceList)));
        assert!(parse_plugin_command("/plugin list extra").is_err());
        assert!(parse_plugin_command("/plugin marketplace add").is_err());
    }

    #[test]
    fn valid_plugin_command_queues_request_and_reports_pending() {
        let (mut app, mut requests) = make_app();
        app.input = "/plugin list".into();

        let outcome = handle_plugin_command(&mut app);

        assert!(outcome.handled);
        assert!(outcome.clear_input);
        assert!(matches!(requests.try_recv(), Ok(PluginRequest::List)));
        assert!(app.raw_messages.iter().any(|message| message.contains("queued")));
    }

    #[test]
    fn invalid_plugin_command_reports_usage_without_queueing() {
        let (mut app, mut requests) = make_app();
        app.input = "/plugin install demo".into();

        handle_plugin_command(&mut app);

        assert!(requests.try_recv().is_err());
        assert!(app.raw_messages.iter().any(|message| message.starts_with("Usage: /plugin")));
    }

    #[test]
    fn bare_plugin_keeps_input_for_subcommand_without_log_spam() {
        let (mut app, mut requests) = make_app();
        app.input = "/plugin".into();

        let outcome = handle_plugin_command(&mut app);

        assert!(outcome.handled);
        assert!(!outcome.clear_input);
        assert_eq!(app.input, "/plugin ");
        assert_eq!(app.input_cursor, "/plugin ".len());
        assert!(requests.try_recv().is_err());
        assert!(
            !app.raw_messages.iter().any(|message| message.starts_with("Usage: /plugin")),
            "bare /plugin must not spam the log: {:?}",
            app.raw_messages
        );
        assert!(
            app.flash_msg.as_ref().is_some_and(|(msg, _)| msg.starts_with("Usage: /plugin")),
            "expected a flash usage hint, got {:?}",
            app.flash_msg
        );
    }

    #[test]
    fn plugin_feedback_uses_the_selected_language() {
        let (mut app, _requests) = make_app();
        app.language = Language::Chinese;
        app.input = "/plugin list".into();

        handle_plugin_command(&mut app);

        assert!(
            app.raw_messages.iter().any(|message| message.contains("插件请求已加入队列")),
            "plugin feedback should use the selected language: {:?}",
            app.raw_messages
        );
    }
}
