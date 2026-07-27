use crossterm::event::{KeyCode, KeyEvent};
use tact_protocol::UserCommand;

use crate::widgets::state::{App, InputMode, SelectKind};

const THINKING_BUDGETS: [usize; 5] = [0, 8_000, 32_000, 64_000, 128_000];

fn nearest_thinking_budget_index(current: usize) -> usize {
    THINKING_BUDGETS
        .iter()
        .enumerate()
        .min_by_key(|(_, budget)| current.abs_diff(**budget))
        .map(|(index, _)| index)
        .expect("THINKING_BUDGETS is non-empty")
}

fn format_thinking_budget(budget: usize) -> String {
    if budget == 0 {
        "0".to_string()
    } else {
        format!("{}K", budget / 1_000)
    }
}

fn thinking_budget_options(msgs: &crate::i18n::Messages) -> Vec<String> {
    vec![
        msgs.model_thinking_budget_off.to_string(),
        msgs.model_thinking_budget_low.to_string(),
        msgs.model_thinking_budget_medium.to_string(),
        msgs.model_thinking_budget_high.to_string(),
        msgs.model_thinking_budget_max.to_string(),
    ]
}

/// Substitute the model first and the formatted thinking budget second.
///
/// Splitting the source template before inserting either value prevents braces in
/// a model id from being mistaken for the second placeholder.
fn format_model_and_budget(template: &str, model: &str, budget: &str) -> String {
    let Some((prefix, after_model)) = template.split_once("{}") else {
        return template.to_string();
    };
    let Some((between, suffix)) = after_model.split_once("{}") else {
        return format!("{prefix}{model}{after_model}");
    };
    format!("{prefix}{model}{between}{budget}{suffix}")
}

/// Select popup mode key handling: up/down to navigate, Enter to confirm, Esc to cancel.
/// Multi-select also uses Space to toggle checkboxes.
pub(crate) fn handle_select_mode(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Char(' ') if app.select.multi => {
            app.select.toggle_checked();
        }
        KeyCode::Enter => {
            if app.select.options.is_empty() {
                let msgs = app.msgs();
                app.add_system_message(msgs.no_options.to_string());
                app.input_mode = InputMode::Normal;
                app.select_kind = SelectKind::Agent;
                return;
            }

            let log_confirm = app.select.log_confirm;
            let multi = app.select.multi;

            if multi {
                let idxs = app.select.confirm_multi();
                let chosen: Vec<String> = idxs
                    .iter()
                    .filter_map(|&i| app.select.options.get(i).cloned())
                    .collect();
                let label = if chosen.is_empty() {
                    "(none)".to_string()
                } else {
                    chosen.join(", ")
                };
                match std::mem::replace(&mut app.select_kind, SelectKind::Agent) {
                    SelectKind::Agent => {
                        if log_confirm {
                            let msgs = app.msgs();
                            app.add_system_message(msgs.selected_tmpl.replace("{}", &label));
                        }
                        app.input_mode = InputMode::Normal;
                    }
                    // Multi is only opened for agent ask_user; local flows stay single-select.
                    SelectKind::ModelPick
                    | SelectKind::ThinkBudgetPick { .. }
                    | SelectKind::PersistModelAndBudget { .. }
                    | SelectKind::ViewSystemPrompt
                    | SelectKind::PermissionModePick
                    | SelectKind::SubagentModelPick
                    | SelectKind::SubagentThinkBudgetPick { .. }
                    | SelectKind::SubagentPersistModelAndBudget { .. } => {
                        app.input_mode = InputMode::Normal;
                    }
                }
                return;
            }

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
                SelectKind::ViewSystemPrompt => {
                    let content = if idx == 0 {
                        Some((
                            "Raw system prompt template",
                            include_str!("../../../tact/src/prompt/system_prompt_template.md")
                                .to_string(),
                        ))
                    } else {
                        app.session_store.as_ref().and_then(|store| {
                            tokio::task::block_in_place(|| {
                                tokio::runtime::Handle::current()
                                    .block_on(store.load_latest_request_body(&app.session_id))
                            })
                            .ok()
                            .flatten()
                            .and_then(|body| {
                                crate::system_prompt::extract_system_prompt(&body).ok()
                            })
                            .map(|content| ("Assembled current system prompt", content))
                        })
                    };
                    let (title, content) = content.unwrap_or_else(|| (
                        "Assembled current system prompt",
                        "## Unavailable\n\nNo persisted LLM request with a system prompt is available for this session.".to_string(),
                    ));
                    let (rendered, _) =
                        crate::render::render_md::render_markdown_tui(&content, &app.theme);
                    app.system_prompt_popup = Some(crate::widgets::state::SystemPromptPopup {
                        title: title.to_string(),
                        rendered,
                        scroll: 0,
                    });
                    app.input_mode = InputMode::Normal;
                }
                SelectKind::ModelPick => {
                    open_thinking_budget_picker(app, strip_current_marker(&chosen));
                }
                SelectKind::ThinkBudgetPick { model } => {
                    let thinking_budget = THINKING_BUDGETS
                        .get(idx)
                        .copied()
                        .unwrap_or(*THINKING_BUDGETS.last().expect("non-empty budgets"));
                    apply_model_and_budget_pick(app, model, thinking_budget);
                }
                SelectKind::PersistModelAndBudget {
                    model,
                    thinking_budget,
                } => {
                    finish_persist_prompt(app, &chosen, &model, thinking_budget);
                }
                SelectKind::PermissionModePick => {
                    let msgs = app.msgs();
                    let (mode_str, display_label) = match idx {
                        0 => ("default", msgs.permission_option_default),
                        1 => ("plan", msgs.permission_option_plan),
                        _ => ("auto", msgs.permission_option_auto),
                    };
                    app.status_bar.permission_mode = mode_str.to_string();
                    app.add_system_message(
                        msgs.permission_set_tmpl.replace("{}", display_label),
                    );
                    let _ = app
                        .user_cmd_tx
                        .send(UserCommand::SetPermissionMode(mode_str.to_string()));
                    app.input_mode = InputMode::Normal;
                }
                SelectKind::SubagentModelPick => {
                    open_subagent_thinking_budget_picker(app, strip_current_marker(&chosen));
                }
                SelectKind::SubagentThinkBudgetPick { model } => {
                    let thinking_budget = THINKING_BUDGETS
                        .get(idx)
                        .copied()
                        .unwrap_or(*THINKING_BUDGETS.last().expect("non-empty budgets"));
                    apply_subagent_model_and_budget_pick(app, model, thinking_budget);
                }
                SelectKind::SubagentPersistModelAndBudget {
                    model,
                    thinking_budget,
                } => {
                    finish_subagent_persist_prompt(app, &chosen, &model, thinking_budget);
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
            match std::mem::replace(&mut app.select_kind, SelectKind::Agent) {
                SelectKind::PersistModelAndBudget {
                    model,
                    thinking_budget,
                } => {
                    let budget_label = format_thinking_budget(thinking_budget);
                    app.add_system_message(format_model_and_budget(
                        msgs.model_session_only_with_budget_tmpl,
                        &model,
                        &budget_label,
                    ));
                }
                SelectKind::SubagentPersistModelAndBudget {
                    model,
                    thinking_budget,
                } => {
                    let budget_label = format_thinking_budget(thinking_budget);
                    app.add_system_message(format_model_and_budget(
                        msgs.model_subagent_session_only_with_budget_tmpl,
                        &model,
                        &budget_label,
                    ));
                }
                SelectKind::Agent
                | SelectKind::ModelPick
                | SelectKind::ThinkBudgetPick { .. }
                | SelectKind::ViewSystemPrompt
                | SelectKind::PermissionModePick
                | SelectKind::SubagentModelPick
                | SelectKind::SubagentThinkBudgetPick { .. } => {
                    app.add_system_message(msgs.selection_cancelled.to_string());
                }
            }
            app.input_mode = InputMode::Normal;
        }
        _ => {}
    }
}

fn strip_current_marker(label: &str) -> String {
    label.strip_suffix(" *").unwrap_or(label).to_string()
}

fn open_thinking_budget_picker(app: &mut App, model: String) {
    let msgs = app.msgs();
    let thinking_budget = tact::config::try_settings()
        .map(|settings| settings.agent.thinking_budget)
        .unwrap_or_default();
    app.select_kind = SelectKind::ThinkBudgetPick { model };
    app.select.set_local(
        msgs.model_thinking_budget_prompt.to_string(),
        thinking_budget_options(&msgs),
        nearest_thinking_budget_index(thinking_budget),
        false,
    );
    app.input_mode = InputMode::Select;
}

fn apply_model_and_budget_pick(app: &mut App, model: String, thinking_budget: usize) {
    let msgs = app.msgs();
    if let Err(err) = tact_llm::set_model(model.clone()) {
        app.add_system_message(msgs.model_switch_failed_tmpl.replace("{}", &err));
        app.input_mode = InputMode::Normal;
        return;
    }

    tact::config::update_llm_model_and_thinking_budget(model.clone(), thinking_budget);
    app.status_bar.model_name = model.clone();
    app.status_bar.model_thinking_budget = (thinking_budget > 0).then_some(thinking_budget as u32);
    app.status_bar.model_reasoning_effort =
        tact_llm::current_reasoning_effort_from_budget(thinking_budget).map(str::to_string);
    let budget_label = format_thinking_budget(thinking_budget);
    app.add_system_message(format_model_and_budget(
        msgs.model_switched_with_budget_tmpl,
        &model,
        &budget_label,
    ));
    let _ = app
        .user_cmd_tx
        .send(UserCommand::SetThinkingBudget(thinking_budget));

    let Some(settings) = tact::config::try_settings() else {
        app.input_mode = InputMode::Normal;
        return;
    };
    if settings.config_path.is_none() {
        app.add_system_message(format_model_and_budget(
            msgs.model_session_only_with_budget_tmpl,
            &model,
            &budget_label,
        ));
        app.input_mode = InputMode::Normal;
        return;
    }

    app.select_kind = SelectKind::PersistModelAndBudget {
        model,
        thinking_budget,
    };
    app.select.set_local(
        msgs.model_persist_with_budget_prompt.to_string(),
        vec![
            msgs.model_persist_yes.to_string(),
            msgs.model_persist_no.to_string(),
        ],
        1,
        false,
    );
    app.input_mode = InputMode::Select;
}

fn finish_persist_prompt(app: &mut App, chosen: &str, model: &str, thinking_budget: usize) {
    let msgs = app.msgs();
    let budget_label = format_thinking_budget(thinking_budget);
    if chosen == msgs.model_persist_yes {
        match tact::config::persist_active_provider_model_and_thinking_budget(
            model,
            thinking_budget,
        ) {
            Ok(()) => app.add_system_message(format_model_and_budget(
                msgs.model_persisted_with_budget_tmpl,
                model,
                &budget_label,
            )),
            Err(err) => app.add_system_message(
                msgs.model_persist_failed_tmpl
                    .replace("{}", &err.to_string()),
            ),
        }
    } else {
        app.add_system_message(format_model_and_budget(
            msgs.model_session_only_with_budget_tmpl,
            model,
            &budget_label,
        ));
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

    let api_ids = if tact_llm::is_models_query_supported() {
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                tokio::task::block_in_place(|| handle.block_on(tact_llm::ensure_api_model_ids()))
            }
            // Sync call sites (e.g. unit tests without a runtime) keep config-only.
            Err(_) => Vec::new(),
        }
    } else {
        Vec::new()
    };

    let mut candidates = tact_llm::merge_model_candidates(&settings.llm.models, &api_ids);

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

/// Open the `/model-subagent` SelectPopup from palette / slash command.
pub(crate) fn start_subagent_model_picker(app: &mut App) {
    let msgs = app.msgs();
    let Some(settings) = tact::config::try_settings() else {
        app.add_system_message(msgs.model_subagent_not_configured.to_string());
        return;
    };
    let Some(subagent) = &settings.agent.subagent else {
        app.add_system_message(msgs.model_subagent_not_configured.to_string());
        return;
    };

    let api_ids = if tact_llm::is_models_query_supported() {
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                tokio::task::block_in_place(|| handle.block_on(tact_llm::ensure_api_model_ids()))
            }
            Err(_) => Vec::new(),
        }
    } else {
        Vec::new()
    };

    let subagent_provider_name = subagent.provider.provider.as_str();
    let mut candidates = tact_llm::merge_model_candidates(&subagent.models, &api_ids);

    if candidates.is_empty() {
        app.add_system_message(
            msgs.model_subagent_list_empty_tmpl
                .replace("{}", subagent_provider_name),
        );
        return;
    }

    let current = subagent.provider.model.clone();
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
        .model_subagent_select_prompt_tmpl
        .replace("{}", subagent_provider_name);
    app.select_kind = SelectKind::SubagentModelPick;
    app.select.set_local(prompt, options, selected, false);
    app.input_mode = InputMode::Select;
}

fn open_subagent_thinking_budget_picker(app: &mut App, model: String) {
    let msgs = app.msgs();
    let thinking_budget = tact::config::try_settings()
        .and_then(|s| s.agent.subagent.as_ref().map(|sa| sa.thinking_budget))
        .unwrap_or_default();
    app.select_kind = SelectKind::SubagentThinkBudgetPick { model };
    app.select.set_local(
        msgs.model_thinking_budget_prompt.to_string(),
        thinking_budget_options(&msgs),
        nearest_thinking_budget_index(thinking_budget),
        false,
    );
    app.input_mode = InputMode::Select;
}

fn apply_subagent_model_and_budget_pick(app: &mut App, model: String, thinking_budget: usize) {
    let msgs = app.msgs();

    tact::config::update_subagent_model(model.clone(), thinking_budget);

    let budget_label = format_thinking_budget(thinking_budget);
    app.add_system_message(format_model_and_budget(
        msgs.model_subagent_switched_with_budget_tmpl,
        &model,
        &budget_label,
    ));

    let Some(settings) = tact::config::try_settings() else {
        app.input_mode = InputMode::Normal;
        return;
    };
    if settings.config_path.is_none() {
        app.add_system_message(format_model_and_budget(
            msgs.model_subagent_session_only_with_budget_tmpl,
            &model,
            &budget_label,
        ));
        app.input_mode = InputMode::Normal;
        return;
    }

    app.select_kind = SelectKind::SubagentPersistModelAndBudget {
        model,
        thinking_budget,
    };
    app.select.set_local(
        msgs.model_subagent_persist_with_budget_prompt.to_string(),
        vec![
            msgs.model_persist_yes.to_string(),
            msgs.model_persist_no.to_string(),
        ],
        1,
        false,
    );
    app.input_mode = InputMode::Select;
}

fn finish_subagent_persist_prompt(app: &mut App, chosen: &str, model: &str, thinking_budget: usize) {
    let msgs = app.msgs();
    let budget_label = format_thinking_budget(thinking_budget);
    if chosen == msgs.model_persist_yes {
        match tact::config::persist_subagent_model(model, thinking_budget) {
            Ok(()) => app.add_system_message(format_model_and_budget(
                msgs.model_subagent_persisted_with_budget_tmpl,
                model,
                &budget_label,
            )),
            Err(err) => app.add_system_message(
                msgs.model_persist_failed_tmpl
                    .replace("{}", &err.to_string()),
            ),
        }
    } else {
        app.add_system_message(format_model_and_budget(
            msgs.model_subagent_session_only_with_budget_tmpl,
            model,
            &budget_label,
        ));
    }
    app.input_mode = InputMode::Normal;
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use tact_llm::{ProviderInfo, ProviderKind};
    use tempfile::TempDir;

    use super::*;
    use crate::render::test_harness::make_app;

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
                protocol: tact_llm::OpenAiProtocol::default(),
                reasoning_effort: None,
                api_key: "sk-test".into(),
                base_url: "https://api.moonshot.cn/v1".into(),
                model: current.into(),
                models: models.into_iter().map(str::to_string).collect(),
            },
            agent: tact::config::AgentSettings {
                max_tokens: 8000,
                thinking_budget: 0,
                model_context_window: 500_000,
                notifications_enabled: false,
                snapshot_max_items: 80,
                micro_compact_enabled: true,
                skill_body_auto_inject: false,
                skill_dirs: Vec::new(),
                instruction_sources: tact::config::InstructionSources::default(),
                subagent: None,
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
                bash_timeout_secs: tact::config::ToolSettings::DEFAULT_BASH_TIMEOUT_SECS,
                bash_nice: tact::config::ToolSettings::DEFAULT_BASH_NICE,
            },
            permission_mode: None,
            tokio_console: false,
            config_path: None,
        });
        tact_llm::init_provider(ProviderInfo {
            provider: ProviderKind::Kimi,
            protocol: tact_llm::OpenAiProtocol::default(),
            reasoning_effort: None,
            api_key: "sk-test".into(),
            base_url: "https://api.moonshot.cn/v1".into(),
            model: current.into(),
        });
    }

    fn install_models_config_with_budget(
        models: Vec<&str>,
        current: &str,
        thinking_budget: usize,
    ) {
        install_models_config(models, current);
        let mut cfg = tact::config::settings();
        cfg.agent.thinking_budget = thinking_budget;
        tact::config::install_or_override(cfg);
    }

    fn install_models_config_with_path(
        models: Vec<&str>,
        current: &str,
        thinking_budget: usize,
    ) -> (TempDir, PathBuf) {
        let temp_dir = tempfile::tempdir().expect("temporary config directory");
        let path = temp_dir.path().join("config.toml");
        std::fs::write(
            &path,
            format!(
                "[llm]
provider = \"kimi\"

[llm.providers.kimi]
model = \"{current}\"
thinking_budget = {thinking_budget}
"
            ),
        )
        .expect("temporary config");
        install_models_config_with_budget(models, current, thinking_budget);
        let mut cfg = tact::config::settings();
        cfg.config_path = Some(path.clone());
        tact::config::install_or_override(cfg);
        (temp_dir, path)
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
    fn arrow_keys_navigate_options() {
        let mut app = make_app();
        let _rx = seed_select(&mut app);

        assert_eq!(app.select.selected, 0);
        handle_select_mode(&mut app, key(KeyCode::Down));
        assert_eq!(app.select.selected, 1);
        handle_select_mode(&mut app, key(KeyCode::Up));
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
        assert!(
            app.raw_messages
                .iter()
                .any(|m| m.contains("Deny") || m.contains("Selected") || m.contains("已选择")),
            "log_confirm should render selection in the log: {:?}",
            app.raw_messages
        );
    }

    #[test]
    fn esc_cancels_and_sends_none() {
        let mut app = make_app();
        let mut rx = seed_select(&mut app);

        handle_select_mode(&mut app, key(KeyCode::Esc));

        assert!(matches!(app.input_mode, InputMode::Normal));
        assert_eq!(rx.try_recv(), Ok(None));
    }

    /// Serialize model-picker tests that share global `CACHE` / `PROVIDER`.
    static MODELS_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    #[tokio::test(flavor = "multi_thread")]
    async fn model_picker_empty_then_confirm_sets_model() {
        let _lock = MODELS_TEST_LOCK.lock().await;
        tact_llm::clear_models_cache_for_tests();
        tact_llm::seed_models_cache_for_tests("https://api.moonshot.cn/v1", "sk-test", vec![]);
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
        handle_select_mode(&mut app, key(KeyCode::Enter));

        assert_eq!(tact_llm::get_provider().model, "kimi-for-coding");
        assert_eq!(app.status_bar.model_name, "kimi-for-coding");
        // No config_path → skip persist popup, return to Normal.
        assert!(matches!(app.input_mode, InputMode::Normal));
    }

    #[test]
    fn format_model_and_budget_only_replaces_template_placeholders() {
        assert_eq!(
            format_model_and_budget("model={} budget={}", "model{}id", "64K"),
            "model=model{}id budget=64K"
        );
    }

    #[test]
    fn nearest_thinking_budget_ties_choose_the_lower_index() {
        assert_eq!(nearest_thinking_budget_index(48_000), 2);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn failed_model_application_leaves_config_and_status_unchanged() {
        let _lock = MODELS_TEST_LOCK.lock().await;
        install_models_config_with_budget(vec!["kimi-k2.5", ""], "kimi-k2.5", 32_000);
        let mut app = make_app();
        app.status_bar.model_name = "status-before".to_string();
        app.status_bar.model_thinking_budget = Some(32_000);
        start_model_picker(&mut app);
        handle_select_mode(&mut app, key(KeyCode::Char('j')));
        handle_select_mode(&mut app, key(KeyCode::Enter));
        handle_select_mode(&mut app, key(KeyCode::Enter));

        assert_eq!(tact_llm::get_provider().model, "kimi-k2.5");
        assert_eq!(tact::config::settings().llm.model, "kimi-k2.5");
        assert_eq!(tact::config::settings().agent.thinking_budget, 32_000);
        assert_eq!(app.status_bar.model_name, "status-before");
        assert_eq!(app.status_bar.model_thinking_budget, Some(32_000));
        assert!(matches!(app.input_mode, InputMode::Normal));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn model_confirmation_opens_budget_picker_without_applying_model() {
        let _lock = MODELS_TEST_LOCK.lock().await;
        install_models_config_with_budget(
            vec!["kimi-k2.5", "kimi-for-coding"],
            "kimi-k2.5",
            32_000,
        );
        let mut app = make_app();
        start_model_picker(&mut app);
        handle_select_mode(&mut app, key(KeyCode::Char('j')));
        handle_select_mode(&mut app, key(KeyCode::Enter));

        assert!(
            matches!(app.select_kind, SelectKind::ThinkBudgetPick { ref model } if model == "kimi-for-coding")
        );
        assert_eq!(tact_llm::get_provider().model, "kimi-k2.5");
        assert_eq!(tact::config::settings().agent.thinking_budget, 32_000);
        assert_eq!(app.select.options.len(), 5);
        assert_eq!(app.select.selected, 2);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn confirmed_budget_applies_model_and_budget_for_this_session() {
        let _lock = MODELS_TEST_LOCK.lock().await;
        install_models_config_with_budget(
            vec!["kimi-k2.5", "kimi-for-coding"],
            "kimi-k2.5",
            32_000,
        );
        let mut app = make_app();
        start_model_picker(&mut app);
        handle_select_mode(&mut app, key(KeyCode::Char('j')));
        handle_select_mode(&mut app, key(KeyCode::Enter));
        handle_select_mode(&mut app, key(KeyCode::Char('j')));
        handle_select_mode(&mut app, key(KeyCode::Enter));

        assert_eq!(tact_llm::get_provider().model, "kimi-for-coding");
        assert_eq!(tact::config::settings().agent.thinking_budget, 64_000);
        assert_eq!(app.status_bar.model_name, "kimi-for-coding");
        assert_eq!(app.status_bar.model_thinking_budget, Some(64_000));
        assert!(matches!(app.input_mode, InputMode::Normal));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn choosing_current_model_still_opens_budget_picker() {
        let _lock = MODELS_TEST_LOCK.lock().await;
        install_models_config_with_budget(vec!["kimi-k2.5"], "kimi-k2.5", 32_000);
        let mut app = make_app();
        start_model_picker(&mut app);
        handle_select_mode(&mut app, key(KeyCode::Enter));

        assert!(matches!(
            app.select_kind,
            SelectKind::ThinkBudgetPick { .. }
        ));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn nonstandard_budget_prefocuses_nearest_fixed_choice() {
        let _lock = MODELS_TEST_LOCK.lock().await;
        install_models_config_with_budget(vec!["kimi-k2.5"], "kimi-k2.5", 40_000);
        let mut app = make_app();
        start_model_picker(&mut app);
        handle_select_mode(&mut app, key(KeyCode::Enter));

        assert_eq!(app.select.selected, 2);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn escape_from_model_or_budget_picker_keeps_model_and_budget_unchanged() {
        let _lock = MODELS_TEST_LOCK.lock().await;
        install_models_config_with_budget(
            vec!["kimi-k2.5", "kimi-for-coding"],
            "kimi-k2.5",
            32_000,
        );
        let mut app = make_app();
        start_model_picker(&mut app);
        handle_select_mode(&mut app, key(KeyCode::Esc));
        assert_eq!(tact_llm::get_provider().model, "kimi-k2.5");
        assert_eq!(tact::config::settings().agent.thinking_budget, 32_000);

        start_model_picker(&mut app);
        handle_select_mode(&mut app, key(KeyCode::Char('j')));
        handle_select_mode(&mut app, key(KeyCode::Enter));
        handle_select_mode(&mut app, key(KeyCode::Esc));

        assert_eq!(tact_llm::get_provider().model, "kimi-k2.5");
        assert_eq!(tact::config::settings().agent.thinking_budget, 32_000);
        assert!(matches!(app.input_mode, InputMode::Normal));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn persisted_model_and_budget_are_written_after_confirming_save() {
        let _lock = MODELS_TEST_LOCK.lock().await;
        let (_temp_dir, path) = install_models_config_with_path(
            vec!["kimi-k2.5", "kimi-for-coding"],
            "kimi-k2.5",
            32_000,
        );
        let mut app = make_app();
        start_model_picker(&mut app);
        handle_select_mode(&mut app, key(KeyCode::Char('j')));
        handle_select_mode(&mut app, key(KeyCode::Enter));
        handle_select_mode(&mut app, key(KeyCode::Char('j')));
        handle_select_mode(&mut app, key(KeyCode::Enter));

        assert!(matches!(
            app.select_kind,
            SelectKind::PersistModelAndBudget { .. }
        ));
        handle_select_mode(&mut app, key(KeyCode::Up));
        handle_select_mode(&mut app, key(KeyCode::Enter));

        let config = std::fs::read_to_string(&path).unwrap();
        assert!(config.contains("model = \"kimi-for-coding\""));
        assert!(config.contains("thinking_budget = 64000"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn escape_from_persist_keeps_applied_values_without_writing_config() {
        let _lock = MODELS_TEST_LOCK.lock().await;
        let (_temp_dir, path) = install_models_config_with_path(
            vec!["kimi-k2.5", "kimi-for-coding"],
            "kimi-k2.5",
            32_000,
        );
        let original = std::fs::read_to_string(&path).unwrap();
        let mut app = make_app();
        start_model_picker(&mut app);
        handle_select_mode(&mut app, key(KeyCode::Char('j')));
        handle_select_mode(&mut app, key(KeyCode::Enter));
        handle_select_mode(&mut app, key(KeyCode::Char('j')));
        handle_select_mode(&mut app, key(KeyCode::Enter));
        assert!(matches!(
            app.select_kind,
            SelectKind::PersistModelAndBudget { .. }
        ));

        handle_select_mode(&mut app, key(KeyCode::Esc));

        assert_eq!(tact_llm::get_provider().model, "kimi-for-coding");
        assert_eq!(tact::config::settings().agent.thinking_budget, 64_000);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
        assert!(app.raw_messages.iter().any(|message| {
            message.contains(
                "Model kimi-for-coding and thinking budget 64K apply only to this session",
            )
        }));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn model_picker_merges_api_ids_after_config() {
        let _lock = MODELS_TEST_LOCK.lock().await;
        tact_llm::clear_models_cache_for_tests();
        install_models_config(vec!["cfg-a", "overlap"], "cfg-a");
        tact_llm::seed_models_cache_for_tests(
            "https://api.moonshot.cn/v1",
            "sk-test",
            vec!["overlap".into(), "api-only".into()],
        );

        let mut app = make_app();
        start_model_picker(&mut app);
        assert!(matches!(app.input_mode, InputMode::Select));
        let options = &app.select.options;
        assert_eq!(
            options
                .iter()
                .map(|o| o.trim_end_matches(" *"))
                .collect::<Vec<_>>(),
            vec!["cfg-a", "overlap", "api-only"]
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn model_picker_api_only_when_config_empty() {
        let _lock = MODELS_TEST_LOCK.lock().await;
        tact_llm::clear_models_cache_for_tests();
        install_models_config(vec![], "current-x");
        tact_llm::seed_models_cache_for_tests(
            "https://api.moonshot.cn/v1",
            "sk-test",
            vec!["api-1".into(), "api-2".into()],
        );

        let mut app = make_app();
        start_model_picker(&mut app);
        assert!(matches!(app.input_mode, InputMode::Select));
        let options = &app.select.options;
        // current-x prepended because it is not in the merged list
        assert!(options[0].starts_with("current-x"));
        assert!(options.iter().any(|o| o.contains("api-1")));
    }
}
