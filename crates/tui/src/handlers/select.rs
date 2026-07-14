use crate::widgets::state::{App, InputMode, SelectKind};
use crossterm::event::{KeyCode, KeyEvent};

/// Select popup mode key handling: up/down to navigate, Enter to confirm, Esc to cancel.
pub(crate) fn handle_select_mode(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Enter => {
            if app.select.options.is_empty() {
                let msgs = app.msgs();
                app.add_system_message(msgs.no_options.to_string());
                app.input_mode = InputMode::Normal;
                app.select_kind = SelectKind::Agent;
                return;
            }

            let log_confirm = app.select.log_confirm;
            let idx = app.select.confirm().unwrap_or(0);
            let chosen = app
                .select
                .options
                .get(idx)
                .cloned()
                .unwrap_or_else(|| "?".to_string());

            match std::mem::replace(&mut app.select_kind, SelectKind::Agent) {
                SelectKind::Agent => {
                    if log_confirm {
                        let msgs = app.msgs();
                        app.add_system_message(msgs.selected_tmpl.replace("{}", &chosen));
                    }
                    app.input_mode = InputMode::Normal;
                }
                SelectKind::ModelPick => {
                    apply_model_pick(app, strip_current_marker(&chosen));
                }
                SelectKind::PersistModel { model } => {
                    finish_persist_prompt(app, &chosen, &model);
                }
            }
        }
        KeyCode::Char('j') | KeyCode::Down => {
            app.select.move_down();
        }
        KeyCode::Char('k') | KeyCode::Up => {
            app.select.move_up();
        }
        KeyCode::Esc => {
            app.select.cancel();
            let msgs = app.msgs();
            app.add_system_message(msgs.selection_cancelled.to_string());
            app.select_kind = SelectKind::Agent;
            app.input_mode = InputMode::Normal;
        }
        _ => {}
    }
}

fn strip_current_marker(label: &str) -> String {
    label.strip_suffix(" *").unwrap_or(label).to_string()
}

fn apply_model_pick(app: &mut App, model: String) {
    let msgs = app.msgs();
    if let Err(err) = tact_llm::set_model(model.clone()) {
        app.add_system_message(msgs.model_switch_failed_tmpl.replace("{}", &err));
        app.input_mode = InputMode::Normal;
        return;
    }
    tact::config::update_llm_model(model.clone());
    app.status_bar.model_name = model.clone();
    app.add_system_message(msgs.model_switched_tmpl.replace("{}", &model));

    let Some(settings) = tact::config::try_settings() else {
        app.input_mode = InputMode::Normal;
        return;
    };
    if settings.config_path.is_none() {
        app.add_system_message(msgs.model_no_config_file.to_string());
        app.input_mode = InputMode::Normal;
        return;
    }

    app.select_kind = SelectKind::PersistModel {
        model: model.clone(),
    };
    app.select.set_local(
        msgs.model_persist_prompt.to_string(),
        vec![
            msgs.model_persist_yes.to_string(),
            msgs.model_persist_no.to_string(),
        ],
        1,
        false,
    );
    app.input_mode = InputMode::Select;
}

fn finish_persist_prompt(app: &mut App, chosen: &str, model: &str) {
    let msgs = app.msgs();
    let yes = msgs.model_persist_yes;
    if chosen == yes {
        match tact::config::persist_active_provider_model(model) {
            Ok(()) => {
                app.add_system_message(msgs.model_persisted_tmpl.replace("{}", model));
            }
            Err(err) => {
                app.add_system_message(
                    msgs.model_persist_failed_tmpl
                        .replace("{}", &err.to_string()),
                );
            }
        }
    } else {
        app.add_system_message(msgs.model_session_only.to_string());
    }
    app.input_mode = InputMode::Normal;
}

/// Open the `/model` SelectPopup from palette / slash command.
pub(crate) fn start_model_picker(app: &mut App) {
    let msgs = app.msgs();
    let Some(settings) = tact::config::try_settings() else {
        app.add_system_message(msgs.model_config_unavailable.to_string());
        return;
    };

    let mut candidates = settings.llm.models.clone();
    if candidates.is_empty() {
        app.add_system_message(
            msgs.model_list_empty_tmpl
                .replace("{}", settings.llm.provider.as_str()),
        );
        return;
    }

    let current = settings.llm.model.clone();
    if !candidates.iter().any(|m| m == &current) {
        candidates.insert(0, current.clone());
    }

    let selected = candidates.iter().position(|m| m == &current).unwrap_or(0);
    let options: Vec<String> = candidates
        .into_iter()
        .enumerate()
        .map(|(i, m)| if i == selected { format!("{m} *") } else { m })
        .collect();

    let prompt = msgs
        .model_select_prompt_tmpl
        .replace("{}", settings.llm.provider.as_str());
    app.select_kind = SelectKind::ModelPick;
    app.select.set_local(prompt, options, selected, false);
    app.input_mode = InputMode::Select;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::test_harness::make_app;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use tact_llm::{ProviderInfo, ProviderKind};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::empty())
    }

    fn seed_select(app: &mut App) -> tokio::sync::oneshot::Receiver<Option<usize>> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        app.select_kind = SelectKind::Agent;
        app.input_mode = InputMode::Select;
        app.select.set(
            "Pick one".into(),
            vec!["Allow once".into(), "Deny".into()],
            tx,
            true,
        );
        rx
    }

    fn install_models_config(models: Vec<&str>, current: &str) {
        tact::config::install_or_override(tact::config::ResolvedConfig {
            llm: tact::config::LlmSettings {
                provider: ProviderKind::Kimi,
                api_key: "sk-test".into(),
                base_url: "https://api.moonshot.cn/v1".into(),
                model: current.into(),
                models: models.into_iter().map(str::to_string).collect(),
            },
            agent: tact::config::AgentSettings {
                max_tokens: 8000,
                thinking_budget: 0,
                context_limit_chars: 500_000,
                notifications_enabled: false,
                snapshot_max_items: 80,
                micro_compact_enabled: true,
                skill_body_auto_inject: false,
            },
            ui: tact::config::UiSettings {
                theme: "retro".into(),
                vision_image: tact::config::VisionImageSettings {
                    compress: true,
                    max_edge: 1280,
                    jpeg_quality: 80,
                },
            },
            tools: tact::config::ToolSettings {
                brave_search_api_key: None,
            },
            permission_mode: None,
            tokio_console: false,
            config_path: None,
        });
        tact_llm::init_provider(ProviderInfo {
            provider: ProviderKind::Kimi,
            api_key: "sk-test".into(),
            base_url: "https://api.moonshot.cn/v1".into(),
            model: current.into(),
        });
    }

    #[test]
    fn j_k_navigates_options() {
        let mut app = make_app();
        let _rx = seed_select(&mut app);

        assert_eq!(app.select.selected, 0);
        handle_select_mode(&mut app, key(KeyCode::Char('j')));
        assert_eq!(app.select.selected, 1);
        handle_select_mode(&mut app, key(KeyCode::Char('k')));
        assert_eq!(app.select.selected, 0);
    }

    #[test]
    fn enter_confirms_selection_and_returns_to_normal() {
        let mut app = make_app();
        let mut rx = seed_select(&mut app);

        handle_select_mode(&mut app, key(KeyCode::Char('j')));
        handle_select_mode(&mut app, key(KeyCode::Enter));

        assert!(matches!(app.input_mode, InputMode::Normal));
        assert_eq!(rx.try_recv(), Ok(Some(1)));
    }

    #[test]
    fn esc_cancels_and_sends_none() {
        let mut app = make_app();
        let mut rx = seed_select(&mut app);

        handle_select_mode(&mut app, key(KeyCode::Esc));

        assert!(matches!(app.input_mode, InputMode::Normal));
        assert_eq!(rx.try_recv(), Ok(None));
    }

    #[test]
    fn model_picker_empty_then_confirm_sets_model() {
        install_models_config(vec![], "kimi-k2.5");
        let mut app = make_app();
        start_model_picker(&mut app);
        assert!(!matches!(app.input_mode, InputMode::Select));
        assert!(
            app.raw_messages
                .iter()
                .any(|m| m.contains("models") || m.contains("models =")),
            "expected empty-models hint, got {:?}",
            app.raw_messages
        );

        install_models_config(vec!["kimi-k2.5", "kimi-for-coding"], "kimi-k2.5");
        start_model_picker(&mut app);
        assert!(matches!(app.input_mode, InputMode::Select));
        assert!(matches!(app.select_kind, SelectKind::ModelPick));

        handle_select_mode(&mut app, key(KeyCode::Char('j')));
        handle_select_mode(&mut app, key(KeyCode::Enter));

        assert_eq!(tact_llm::get_provider().model, "kimi-for-coding");
        assert_eq!(app.status_bar.model_name, "kimi-for-coding");
        // No config_path → skip persist popup, return to Normal.
        assert!(matches!(app.input_mode, InputMode::Normal));
    }
}
