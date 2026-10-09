use tact_extensions::plugin::PluginRequest;

use super::CommandExecOutcome;
use crate::widgets::state::App;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PluginUsageError;

pub(crate) fn parse_plugin_command(input: &str) -> Result<PluginRequest, PluginUsageError> {
    let parts: Vec<&str> = input.split_whitespace().collect();
    match parts.as_slice() {
        ["/plugin", "list"] => Ok(PluginRequest::List),
        ["/plugin", "reload"] => Ok(PluginRequest::Reload),
        ["/plugin", "uninstall", name] => Ok(PluginRequest::Uninstall {
            plugin: (*name).to_owned(),
        }),
        ["/plugin", "update", name] => Ok(PluginRequest::Update {
            plugin: (*name).to_owned(),
        }),
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
        app.flash_msg = Some((
            app.msgs().plugin_usage.to_owned(),
            std::time::Instant::now(),
        ));
        return CommandExecOutcome {
            handled: true,
            clear_input: false,
        };
    }
    match parse_plugin_command(&app.input) {
        Ok(request) => match app.plugin_tx.send(request) {
            Ok(()) => app.add_system_message(app.msgs().plugin_request_queued.to_owned()),
            Err(_) => app.add_system_message(app.msgs().plugin_worker_unavailable.to_owned()),
        },
        Err(_) => app.add_system_message(app.msgs().plugin_usage.to_owned()),
    }
    CommandExecOutcome {
        handled: true,
        clear_input: true,
    }
}

#[cfg(test)]
mod tests {
    use super::{handle_plugin_command, parse_plugin_command};
    use crate::i18n::Language;
    use crate::test_fixtures::TestApp;
    use tact_extensions::plugin::PluginRequest;

    #[test]
    fn parses_only_exact_plugin_forms() {
        assert!(matches!(
            parse_plugin_command("/plugin list"),
            Ok(PluginRequest::List)
        ));
        assert!(matches!(
            parse_plugin_command("/plugin reload"),
            Ok(PluginRequest::Reload)
        ));
        assert!(matches!(
            parse_plugin_command("/plugin uninstall demo"),
            Ok(PluginRequest::Uninstall { plugin }) if plugin == "demo"
        ));
        assert!(matches!(
            parse_plugin_command("/plugin update demo"),
            Ok(PluginRequest::Update { plugin }) if plugin == "demo"
        ));
        assert!(matches!(
            parse_plugin_command("/plugin marketplace list"),
            Ok(PluginRequest::MarketplaceList)
        ));
        assert!(parse_plugin_command("/plugin list extra").is_err());
        assert!(parse_plugin_command("/plugin uninstall").is_err());
        assert!(parse_plugin_command("/plugin uninstall demo extra").is_err());
        assert!(parse_plugin_command("/plugin update").is_err());
        assert!(parse_plugin_command("/plugin update demo extra").is_err());
        assert!(parse_plugin_command("/plugin marketplace add").is_err());
    }

    #[test]
    fn update_plugin_command_queues_request() {
        let (mut app, mut requests) = TestApp::new().into_plugin_requests();
        app.input = "/plugin update demo".into();

        let outcome = handle_plugin_command(&mut app);

        assert!(outcome.handled);
        assert!(outcome.clear_input);
        assert!(matches!(
            requests.try_recv(),
            Ok(PluginRequest::Update { plugin }) if plugin == "demo"
        ));
    }

    #[test]
    fn uninstall_plugin_command_queues_request() {
        let (mut app, mut requests) = TestApp::new().into_plugin_requests();
        app.input = "/plugin uninstall demo".into();

        let outcome = handle_plugin_command(&mut app);

        assert!(outcome.handled);
        assert!(outcome.clear_input);
        assert!(matches!(
            requests.try_recv(),
            Ok(PluginRequest::Uninstall { plugin }) if plugin == "demo"
        ));
    }

    #[test]
    fn valid_plugin_command_queues_request_and_reports_pending() {
        let (mut app, mut requests) = TestApp::new().into_plugin_requests();
        app.input = "/plugin list".into();

        let outcome = handle_plugin_command(&mut app);

        assert!(outcome.handled);
        assert!(outcome.clear_input);
        assert!(matches!(requests.try_recv(), Ok(PluginRequest::List)));
        assert!(
            app.log
                .items
                .iter()
                .any(|message| message.raw.contains("queued"))
        );
    }

    #[test]
    fn invalid_plugin_command_reports_usage_without_queueing() {
        let (mut app, mut requests) = TestApp::new().into_plugin_requests();
        app.input = "/plugin install demo".into();

        handle_plugin_command(&mut app);

        assert!(requests.try_recv().is_err());
        assert!(
            app.log
                .items
                .iter()
                .any(|message| message.raw.starts_with("Usage: /plugin"))
        );
    }

    #[test]
    fn bare_plugin_keeps_input_for_subcommand_without_log_spam() {
        let (mut app, mut requests) = TestApp::new().into_plugin_requests();
        app.input = "/plugin".into();

        let outcome = handle_plugin_command(&mut app);

        assert!(outcome.handled);
        assert!(!outcome.clear_input);
        assert_eq!(app.input, "/plugin ");
        assert_eq!(app.input_cursor, "/plugin ".len());
        assert!(requests.try_recv().is_err());
        assert!(
            !app.log
                .items
                .iter()
                .any(|message| message.raw.starts_with("Usage: /plugin")),
            "bare /plugin must not spam the log: {:?}",
            app.log.items
        );
        assert!(
            app.flash_msg
                .as_ref()
                .is_some_and(|(msg, _)| msg.starts_with("Usage: /plugin")),
            "expected a flash usage hint, got {:?}",
            app.flash_msg
        );
    }

    #[test]
    fn plugin_feedback_uses_the_selected_language() {
        let (mut app, _requests) = TestApp::new().into_plugin_requests();
        app.language = Language::Chinese;
        app.input = "/plugin list".into();

        handle_plugin_command(&mut app);

        assert!(
            app.log
                .items
                .iter()
                .any(|message| message.raw.contains("插件请求已加入队列")),
            "plugin feedback should use the selected language: {:?}",
            app.log.items
        );
    }
}
