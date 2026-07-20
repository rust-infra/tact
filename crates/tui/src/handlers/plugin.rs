use super::CommandExecOutcome;
use crate::widgets::state::App;
use tact::plugin::PluginRequest;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PluginUsageError;

pub(crate) fn parse_plugin_command(input: &str) -> Result<PluginRequest, PluginUsageError> {
    let parts: Vec<&str> = input.split_whitespace().collect();
    match parts.as_slice() {
        ["/plugin", "install", plugin_marketplace] => {
            let Some((plugin, marketplace)) = plugin_marketplace.split_once('@') else {
                return Err(PluginUsageError);
            };
            if plugin.is_empty() || marketplace.is_empty() || marketplace.contains('@') {
                return Err(PluginUsageError);
            }
            Ok(PluginRequest::Install {
                plugin: (*plugin).to_owned(),
                marketplace: marketplace.to_owned(),
            })
        }
        ["/plugin", "list"] => Ok(PluginRequest::List),
        ["/plugin", "reload"] => Ok(PluginRequest::Reload),
        ["/plugin", "marketplace", "add", source] => Ok(PluginRequest::MarketplaceAdd {
            source: (*source).to_owned(),
        }),
        ["/plugin", "marketplace", "list"] => Ok(PluginRequest::MarketplaceList),
        ["/plugin", "marketplace", "update", name] if !name.is_empty() => {
            Ok(PluginRequest::MarketplaceUpdate {
                name: (*name).to_owned(),
            })
        }
        ["/plugin", "marketplace", "remove", name] if !name.is_empty() => {
            Ok(PluginRequest::MarketplaceRemove {
                name: (*name).to_owned(),
            })
        }
        _ => Err(PluginUsageError),
    }
}

pub(crate) fn handle_plugin_command(app: &mut App) -> CommandExecOutcome {
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
    use crate::widgets::state::App;
    use std::path::PathBuf;
    use tact::plugin::PluginRequest;
    use tact_protocol::AgentUpdate;
    use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

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
    fn parses_install_and_marketplace_add() {
        assert!(matches!(
            parse_plugin_command("/plugin install demo@fixture"),
            Ok(PluginRequest::Install { .. })
        ));
        assert!(matches!(
            parse_plugin_command("/plugin marketplace add acme/plugins"),
            Ok(PluginRequest::MarketplaceAdd { .. })
        ));
    }

    #[test]
    fn marketplace_add_does_not_derive_a_name_from_the_source_url() {
        assert_eq!(
            parse_plugin_command(
                "/plugin marketplace add https://example.invalid/catalog.json?name=wrong"
            ),
            Ok(PluginRequest::MarketplaceAdd {
                source: "https://example.invalid/catalog.json?name=wrong".into()
            })
        );
    }

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
            parse_plugin_command("/plugin marketplace list"),
            Ok(PluginRequest::MarketplaceList)
        ));
        assert!(matches!(
            parse_plugin_command("/plugin marketplace update fixture"),
            Ok(PluginRequest::MarketplaceUpdate { .. })
        ));
        assert!(matches!(
            parse_plugin_command("/plugin marketplace remove fixture"),
            Ok(PluginRequest::MarketplaceRemove { .. })
        ));
        assert!(parse_plugin_command("/plugin install demo").is_err());
        assert!(parse_plugin_command("/plugin list extra").is_err());
        assert!(parse_plugin_command("/plugin marketplace add").is_err());
    }

    #[test]
    fn valid_plugin_command_queues_request_and_reports_pending() {
        let (mut app, mut requests) = make_app();
        app.input = "/plugin install demo@fixture".into();

        let outcome = handle_plugin_command(&mut app);

        assert!(outcome.handled);
        assert!(outcome.clear_input);
        assert!(matches!(
            requests.try_recv(),
            Ok(PluginRequest::Install { plugin, marketplace })
                if plugin == "demo" && marketplace == "fixture"
        ));
        assert!(
            app.raw_messages
                .iter()
                .any(|message| message.contains("queued"))
        );
    }

    #[test]
    fn invalid_plugin_command_reports_usage_without_queueing() {
        let (mut app, mut requests) = make_app();
        app.input = "/plugin install demo".into();

        handle_plugin_command(&mut app);

        assert!(requests.try_recv().is_err());
        assert!(
            app.raw_messages
                .iter()
                .any(|message| message.starts_with("Usage: /plugin"))
        );
    }

    #[test]
    fn plugin_feedback_uses_the_selected_language() {
        let (mut app, _requests) = make_app();
        app.language = Language::Chinese;
        app.input = "/plugin install demo@fixture".into();

        handle_plugin_command(&mut app);

        assert!(
            app.raw_messages
                .iter()
                .any(|message| message == "插件请求已加入队列"),
            "plugin feedback should use the selected language: {:?}",
            app.raw_messages
        );
    }
}
