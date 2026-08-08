use crossterm::event::{KeyCode, KeyEvent};
use tact_protocol::UserCommand;

use crate::widgets::state::{App, InputMode, SelectKind};

const THINKING_BUDGETS: [usize; 5] = [0, 8_000, 32_000, 64_000, 128_000];

/// Default effort tiers per provider (no model mapping).
///
/// openai: minimal..max (official enum, default medium);
/// deepseek: low/high/max (minimal/medium illegal, xhigh not offered in UI);
/// kimi k3 family: low/high/max (default high).
fn default_effort_tiers(info: &tact_llm::ProviderInfo) -> Vec<tact_llm::OpenAiReasoningEffort> {
    use tact_llm::OpenAiReasoningEffort as E;
    match &info.provider {
        tact_llm::ProviderKind::DeepSeek | tact_llm::ProviderKind::Kimi => {
            vec![E::Low, E::High, E::Max]
        }
        _ => vec![E::Minimal, E::Low, E::Medium, E::High, E::Xhigh, E::Max],
    }
}

fn nearest_budget_index(budgets: &[usize], current: usize) -> usize {
    budgets
        .iter()
        .enumerate()
        .min_by_key(|(_, budget)| current.abs_diff(**budget))
        .map(|(index, _)| index)
        .unwrap_or(0)
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

fn budget_option_labels(msgs: &crate::i18n::Messages, budgets: &[usize]) -> Vec<String> {
    if budgets == THINKING_BUDGETS {
        thinking_budget_options(msgs)
    } else {
        budgets.iter().map(|b| format_thinking_budget(*b)).collect()
    }
}

fn effort_label(msgs: &crate::i18n::Messages, effort: tact_llm::OpenAiReasoningEffort) -> String {
    use tact_llm::OpenAiReasoningEffort as E;
    match effort {
        E::Minimal => msgs.model_effort_minimal.to_string(),
        E::Low => msgs.model_effort_low.to_string(),
        E::Medium => msgs.model_effort_medium.to_string(),
        E::High => msgs.model_effort_high.to_string(),
        E::Xhigh => msgs.model_effort_xhigh.to_string(),
        E::Max => msgs.model_effort_max.to_string(),
        // The effort picker never offers `None`; reaching it is a programming
        // error (e.g. a profile that leaks a `None` tier).
        E::None => {
            unreachable!("OpenAiReasoningEffort::None is never offered in the effort picker")
        }
    }
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
                    | SelectKind::ModelProfileEffortPick { .. }
                    | SelectKind::ThinkBudgetPick { .. }
                    | SelectKind::PersistModelAndBudget { .. }
                    | SelectKind::PersistModelAndEffort { .. }
                    | SelectKind::ViewSystemPrompt
                    | SelectKind::PermissionModePick
                    | SelectKind::SubagentModelPick
                    | SelectKind::SubagentModelProfileEffortPick { .. }
                    | SelectKind::SubagentThinkBudgetPick { .. }
                    | SelectKind::SubagentPersistModelAndBudget { .. }
                    | SelectKind::SubagentPersistModelAndEffort { .. } => {
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
                    open_second_step(app, strip_current_marker(&chosen), false);
                }
                SelectKind::ModelProfileEffortPick { model, efforts } => {
                    let effort = efforts.get(idx).copied().unwrap_or_else(|| {
                        efforts
                            .last()
                            .copied()
                            .unwrap_or(tact_llm::OpenAiReasoningEffort::Medium)
                    });
                    apply_model_and_effort_pick(app, model, effort);
                }
                SelectKind::ThinkBudgetPick { model, budgets } => {
                    let thinking_budget = budgets
                        .get(idx)
                        .copied()
                        .unwrap_or(*budgets.last().unwrap_or(&0));
                    apply_model_and_budget_pick(app, model, thinking_budget);
                }
                SelectKind::PersistModelAndBudget {
                    model,
                    thinking_budget,
                } => {
                    finish_persist_prompt(app, &chosen, &model, thinking_budget);
                }
                SelectKind::PersistModelAndEffort { model, effort } => {
                    finish_persist_effort_prompt(app, &chosen, &model, effort);
                }
                SelectKind::PermissionModePick => {
                    let msgs = app.msgs();
                    let (mode_str, display_label) = match idx {
                        0 => ("default", msgs.permission_option_default),
                        1 => ("plan", msgs.permission_option_plan),
                        _ => ("auto", msgs.permission_option_auto),
                    };
                    app.status_bar.permission_mode = mode_str.to_string();
                    app.add_system_message(msgs.permission_set_tmpl.replace("{}", display_label));
                    let _ = app
                        .user_cmd_tx
                        .send(UserCommand::SetPermissionMode(mode_str.to_string()));
                    app.input_mode = InputMode::Normal;
                }
                SelectKind::SubagentModelPick => {
                    open_second_step(app, strip_current_marker(&chosen), true);
                }
                SelectKind::SubagentModelProfileEffortPick { model, efforts } => {
                    let effort = efforts.get(idx).copied().unwrap_or_else(|| {
                        efforts
                            .last()
                            .copied()
                            .unwrap_or(tact_llm::OpenAiReasoningEffort::Medium)
                    });
                    apply_subagent_model_and_effort_pick(app, model, effort);
                }
                SelectKind::SubagentThinkBudgetPick { model, budgets } => {
                    let thinking_budget = budgets
                        .get(idx)
                        .copied()
                        .unwrap_or(*budgets.last().unwrap_or(&0));
                    apply_subagent_model_and_budget_pick(app, model, thinking_budget);
                }
                SelectKind::SubagentPersistModelAndBudget {
                    model,
                    thinking_budget,
                } => {
                    finish_subagent_persist_prompt(app, &chosen, &model, thinking_budget);
                }
                SelectKind::SubagentPersistModelAndEffort { model, effort } => {
                    finish_subagent_persist_effort_prompt(app, &chosen, &model, effort);
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
                SelectKind::PersistModelAndEffort { model, effort } => {
                    app.add_system_message(
                        msgs.model_session_only_with_effort_tmpl
                            .replace("{}", &model)
                            .replace("{}", effort.as_str()),
                    );
                }
                SelectKind::SubagentPersistModelAndEffort { model, effort } => {
                    app.add_system_message(
                        msgs.model_session_only_with_effort_tmpl
                            .replace("{}", &model)
                            .replace("{}", effort.as_str()),
                    );
                }
                SelectKind::Agent
                | SelectKind::ModelPick
                | SelectKind::ModelProfileEffortPick { .. }
                | SelectKind::ThinkBudgetPick { .. }
                | SelectKind::ViewSystemPrompt
                | SelectKind::PermissionModePick
                | SelectKind::SubagentModelPick
                | SelectKind::SubagentModelProfileEffortPick { .. }
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

/// `/model` second step: branch by the selected model's semantics.
fn open_second_step(app: &mut App, model: String, subagent: bool) {
    let provider = provider_for_second_step(subagent);
    let Some(provider) = provider else {
        app.input_mode = InputMode::Normal;
        return;
    };
    let profile =
        tact::config::try_settings().and_then(|s| s.llm.model_profiles.get(&model).cloned());

    if tact_llm::model_uses_effort(&model, &provider) {
        let efforts = profile
            .and_then(|p| (!p.reasoning_efforts.is_empty()).then_some(p.reasoning_efforts))
            .unwrap_or_else(|| default_effort_tiers(&provider));
        open_effort_picker(app, model, efforts, subagent);
    } else {
        let budgets = profile
            .and_then(|p| (!p.thinking_budgets.is_empty()).then_some(p.thinking_budgets))
            .unwrap_or_else(|| THINKING_BUDGETS.to_vec());
        open_budget_picker(app, model, budgets, subagent);
    }
}

/// Provider identity used to decide the second step (main vs subagent).
fn provider_for_second_step(subagent: bool) -> Option<tact_llm::ProviderInfo> {
    if subagent {
        tact::config::try_settings()
            .and_then(|s| s.agent.subagent)
            .map(|sa| sa.provider)
    } else {
        Some(tact_llm::get_provider())
    }
}

fn open_effort_picker(
    app: &mut App,
    model: String,
    efforts: Vec<tact_llm::OpenAiReasoningEffort>,
    subagent: bool,
) {
    let msgs = app.msgs();
    // Default highlight: current session effort if listed, else first tier.
    let current = if subagent {
        tact::config::try_settings()
            .and_then(|s| s.agent.subagent)
            .and_then(|sa| sa.reasoning_effort)
    } else {
        tact::config::try_settings().and_then(|s| s.agent.reasoning_effort)
    };
    let selected = current
        .and_then(|effort| efforts.iter().position(|e| *e == effort))
        .unwrap_or(0);
    let options: Vec<String> = efforts
        .iter()
        .map(|effort| effort_label(&msgs, *effort))
        .collect();
    let kind = if subagent {
        SelectKind::SubagentModelProfileEffortPick { model, efforts }
    } else {
        SelectKind::ModelProfileEffortPick { model, efforts }
    };
    app.select_kind = kind;
    app.select.set_local(
        msgs.model_effort_prompt.to_string(),
        options,
        selected,
        false,
    );
    app.input_mode = InputMode::Select;
}

fn open_budget_picker(app: &mut App, model: String, budgets: Vec<usize>, subagent: bool) {
    let msgs = app.msgs();
    // Current value differs per target: subagent budget vs main agent budget.
    let thinking_budget = if subagent {
        tact::config::try_settings()
            .and_then(|s| s.agent.subagent.as_ref().map(|sa| sa.thinking_budget))
            .unwrap_or_default()
    } else {
        tact::config::try_settings()
            .map(|settings| settings.agent.thinking_budget)
            .unwrap_or_default()
    };
    let selected = nearest_budget_index(&budgets, thinking_budget);
    let options = budget_option_labels(&msgs, &budgets);
    let kind = if subagent {
        SelectKind::SubagentThinkBudgetPick {
            model,
            budgets: budgets.clone(),
        }
    } else {
        SelectKind::ThinkBudgetPick { model, budgets }
    };
    app.select_kind = kind;
    app.select.set_local(
        msgs.model_thinking_budget_prompt.to_string(),
        options,
        selected,
        false,
    );
    app.input_mode = InputMode::Select;
}

fn apply_model_and_budget_pick(app: &mut App, model: String, thinking_budget: usize) {
    let msgs = app.msgs();
    if model.trim().is_empty() {
        app.add_system_message(
            msgs.model_switch_failed_tmpl
                .replace("{}", "model must not be empty"),
        );
        app.input_mode = InputMode::Normal;
        return;
    }
    let _ = app.user_cmd_tx.send(UserCommand::SetModel(model.clone()));
    tact::config::update_llm_model_and_thinking_budget(model.clone(), thinking_budget);
    app.status_bar.model_name = model.clone();
    if let Some(settings) = tact::config::try_settings() {
        // Keep out/think in sync immediately; agent may still be busy so
        // SetModel / SetThinkingBudget (and their ModelInfo) can arrive later.
        app.status_bar.model_max_tokens = settings.agent.max_tokens;
    }
    app.status_bar.model_thinking_budget = (thinking_budget > 0).then_some(thinking_budget as u32);
    app.status_bar.model_reasoning_effort = None; // budget semantics: no derived effort
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

/// Effort-semantic apply (openai / deepseek / kimi k3): model + effort, no budget.
fn apply_model_and_effort_pick(
    app: &mut App,
    model: String,
    effort: tact_llm::OpenAiReasoningEffort,
) {
    let msgs = app.msgs();
    let _ = app.user_cmd_tx.send(UserCommand::SetModel(model.clone()));
    tact::config::update_llm_model_and_reasoning_effort(model.clone(), Some(effort));
    app.status_bar.model_name = model.clone();
    if let Some(settings) = tact::config::try_settings() {
        app.status_bar.model_max_tokens = settings.agent.max_tokens;
    }
    app.status_bar.model_reasoning_effort = Some(effort.as_str().to_string());
    app.status_bar.model_thinking_budget = None; // effort semantics: budget not shown
    app.add_system_message(
        msgs.model_effort_switched_tmpl
            .replace("{}", &model)
            .replace("{}", effort.as_str()),
    );
    let _ = app.user_cmd_tx.send(UserCommand::SetReasoningEffort(Some(
        effort.as_str().to_string(),
    )));

    open_effort_persist_prompt(
        app,
        model.clone(),
        effort,
        SelectKind::PersistModelAndEffort { model, effort },
    );
}

/// Effort-semantic subagent apply: model + effort (session level).
fn apply_subagent_model_and_effort_pick(
    app: &mut App,
    model: String,
    effort: tact_llm::OpenAiReasoningEffort,
) {
    let msgs = app.msgs();
    let current_budget = tact::config::try_settings()
        .and_then(|s| s.agent.subagent.as_ref().map(|sa| sa.thinking_budget))
        .unwrap_or_default();
    tact::config::update_subagent_model(model.clone(), current_budget);
    tact::config::update_subagent_reasoning_effort(Some(effort));

    app.add_system_message(
        msgs.model_effort_switched_tmpl
            .replace("{}", &model)
            .replace("{}", effort.as_str()),
    );

    open_effort_persist_prompt(
        app,
        model.clone(),
        effort,
        SelectKind::SubagentPersistModelAndEffort { model, effort },
    );
}

/// Shared effort persist flow: if no config file, session-only message; else
/// ask whether to persist model + effort. `persist_kind` carries the target
/// (main vs subagent) and the model/effort to persist.
fn open_effort_persist_prompt(
    app: &mut App,
    model: String,
    effort: tact_llm::OpenAiReasoningEffort,
    persist_kind: SelectKind,
) {
    let msgs = app.msgs();
    let Some(settings) = tact::config::try_settings() else {
        app.input_mode = InputMode::Normal;
        return;
    };
    if settings.config_path.is_none() {
        app.add_system_message(
            msgs.model_session_only_with_effort_tmpl
                .replace("{}", &model)
                .replace("{}", effort.as_str()),
        );
        app.input_mode = InputMode::Normal;
        return;
    }
    app.select_kind = persist_kind;
    app.select.set_local(
        msgs.model_persist_with_effort_prompt.to_string(),
        vec![
            msgs.model_persist_yes.to_string(),
            msgs.model_persist_no.to_string(),
        ],
        1,
        false,
    );
    app.input_mode = InputMode::Select;
}

/// Shared persist-effort confirmation. `persist` is the main vs subagent API.
fn finish_effort_persist(
    app: &mut App,
    chosen: &str,
    model: &str,
    effort: tact_llm::OpenAiReasoningEffort,
    persist: impl FnOnce(&str, &str) -> anyhow::Result<()>,
) {
    let msgs = app.msgs();
    let effort_str = effort.as_str();
    if chosen == msgs.model_persist_yes {
        match persist(model, effort_str) {
            Ok(()) => app.add_system_message(
                msgs.model_persisted_with_effort_tmpl
                    .replace("{}", model)
                    .replace("{}", effort_str),
            ),
            Err(err) => app.add_system_message(
                msgs.model_persist_failed_tmpl
                    .replace("{}", &err.to_string()),
            ),
        }
    } else {
        app.add_system_message(
            msgs.model_session_only_with_effort_tmpl
                .replace("{}", model)
                .replace("{}", effort_str),
        );
    }
    app.input_mode = InputMode::Normal;
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

fn finish_persist_effort_prompt(
    app: &mut App,
    chosen: &str,
    model: &str,
    effort: tact_llm::OpenAiReasoningEffort,
) {
    finish_effort_persist(app, chosen, model, effort, |model, effort_str| {
        tact::config::persist_active_provider_model_and_reasoning_effort(model, effort_str)
    });
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

fn finish_subagent_persist_prompt(
    app: &mut App,
    chosen: &str,
    model: &str,
    thinking_budget: usize,
) {
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

fn finish_subagent_persist_effort_prompt(
    app: &mut App,
    chosen: &str,
    model: &str,
    effort: tact_llm::OpenAiReasoningEffort,
) {
    finish_effort_persist(app, chosen, model, effort, |model, effort_str| {
        tact::config::persist_subagent_model_and_reasoning_effort(model, effort_str)
    });
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
                model_profiles: Default::default(),
                responses_compact_threshold: None,
            },
            agent: tact::config::AgentSettings {
                model: current.into(),
                reasoning_effort: None,
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
                rtk_filter: false,
            },
            voice: tact::config::VoiceSettings::disabled_defaults(),
            permission_mode: None,
            tokio_console: false,
            config_path: None,
        });
        tact_llm::init_provider(ProviderInfo {
            provider: ProviderKind::Kimi,
            protocol: tact_llm::OpenAiProtocol::default(),
            responses_compact_threshold: None,
            api_key: "sk-test".into(),
            base_url: "https://api.moonshot.cn/v1".into(),
            model: current.into(),
        });
    }

    fn install_models_config_with_budget(models: Vec<&str>, current: &str, thinking_budget: usize) {
        install_models_config(models, current);
        let mut cfg = tact::config::settings();
        cfg.agent.thinking_budget = thinking_budget;
        tact::config::install_or_override(cfg);
    }

    fn install_models_config_with_subagent(
        models: Vec<&str>,
        current: &str,
        subagent_model: &str,
        subagent_budget: usize,
        subagent_effort: Option<tact_llm::OpenAiReasoningEffort>,
    ) {
        install_models_config(models, current);
        let mut cfg = tact::config::settings();
        cfg.agent.subagent = Some(tact::config::SubagentSettings {
            provider: tact_llm::ProviderInfo {
                provider: ProviderKind::Kimi,
                protocol: tact_llm::OpenAiProtocol::default(),
                responses_compact_threshold: None,
                api_key: "sk-test".into(),
                base_url: "https://api.moonshot.cn/v1".into(),
                model: subagent_model.into(),
            },
            max_tokens: 4000,
            thinking_budget: subagent_budget,
            reasoning_effort: subagent_effort,
            models: vec![subagent_model.to_string()],
        });
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

        assert_eq!(tact::config::settings().llm.model, "kimi-for-coding");
        assert_eq!(tact::config::settings().agent.model, "kimi-for-coding");
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
        assert_eq!(nearest_budget_index(&THINKING_BUDGETS, 48_000), 2);
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

        assert_eq!(tact::config::settings().llm.model, "kimi-k2.5");
        assert_eq!(tact::config::settings().agent.model, "kimi-k2.5");
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
            matches!(app.select_kind, SelectKind::ThinkBudgetPick { ref model, .. } if model == "kimi-for-coding")
        );
        assert_eq!(tact::config::settings().llm.model, "kimi-k2.5");
        assert_eq!(tact::config::settings().agent.thinking_budget, 32_000);
        assert_eq!(app.select.options.len(), 5);
        assert_eq!(app.select.selected, 2);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn subagent_budget_picker_highlights_subagent_budget_not_main_budget() {
        // Regression: open_budget_picker must read the subagent's own
        // thinking_budget for the highlight, not the main agent's.
        let _lock = MODELS_TEST_LOCK.lock().await;
        tact_llm::clear_models_cache_for_tests();
        install_models_config_with_subagent(
            vec!["kimi-k2.5", "kimi-for-coding"],
            "kimi-k2.5",
            "kimi-for-coding",
            64_000, // subagent budget
            None,
        );
        // Seed the models cache so ensure_api_model_ids() does not hit the
        // network (which would race other tests on the process-global cache).
        tact_llm::seed_models_cache_for_tests(
            "https://api.moonshot.cn/v1",
            "sk-test",
            vec!["kimi-for-coding".into()],
        );
        // Main agent budget differs (8_000); a bug would highlight index 2.
        let mut cfg = tact::config::settings();
        cfg.agent.thinking_budget = 8_000;
        tact::config::install_or_override(cfg);

        let mut app = make_app();
        start_subagent_model_picker(&mut app);
        handle_select_mode(&mut app, key(KeyCode::Char('j')));
        handle_select_mode(&mut app, key(KeyCode::Enter));

        assert!(matches!(
            app.select_kind,
            SelectKind::SubagentThinkBudgetPick { ref model, .. } if model == "kimi-for-coding"
        ));
        // 64_000 is the 4th of the 5 default budgets (0, 8K, 32K, 64K, 128K).
        assert_eq!(app.select.selected, 3);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn effort_picker_highlights_current_session_effort() {
        let _lock = MODELS_TEST_LOCK.lock().await;
        tact_llm::clear_models_cache_for_tests();
        // Kimi K3 model → effort-semantic second step.
        install_models_config(vec!["k3", "k3-256k"], "k3");
        tact_llm::seed_models_cache_for_tests(
            "https://api.moonshot.cn/v1",
            "sk-test",
            vec!["k3".into(), "k3-256k".into()],
        );
        let mut cfg = tact::config::settings();
        cfg.agent.reasoning_effort = Some(tact_llm::OpenAiReasoningEffort::Max);
        tact::config::install_or_override(cfg);

        let mut app = make_app();
        start_model_picker(&mut app);
        handle_select_mode(&mut app, key(KeyCode::Char('j'))); // k3-256k
        handle_select_mode(&mut app, key(KeyCode::Enter));

        assert!(matches!(
            app.select_kind,
            SelectKind::ModelProfileEffortPick { .. }
        ));
        // Low / High / Max → Max is index 2.
        assert_eq!(app.select.selected, 2);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn applying_effort_pick_updates_config_level_effort() {
        // Regression: apply_model_and_effort_pick must keep config-level
        // agent.reasoning_effort in sync so re-opening the picker (and
        // subagent inheritance) sees the new value.
        let _lock = MODELS_TEST_LOCK.lock().await;
        tact_llm::clear_models_cache_for_tests();
        install_models_config(vec!["k3"], "k3");
        tact_llm::seed_models_cache_for_tests(
            "https://api.moonshot.cn/v1",
            "sk-test",
            vec!["k3".into()],
        );
        let mut app = make_app();
        start_model_picker(&mut app);
        handle_select_mode(&mut app, key(KeyCode::Enter)); // k3 (single model)

        assert!(matches!(
            app.select_kind,
            SelectKind::ModelProfileEffortPick { .. }
        ));
        handle_select_mode(&mut app, key(KeyCode::Enter)); // first effort (Low)

        // No config path → session-only, back to normal mode.
        assert!(matches!(app.input_mode, InputMode::Normal));
        assert_eq!(
            tact::config::settings().agent.reasoning_effort,
            Some(tact_llm::OpenAiReasoningEffort::Low)
        );
        assert_eq!(tact::config::settings().llm.model, "k3");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn applying_effort_pick_clears_stale_thinking_budget() {
        // Regression: an effort pick for an effort-semantic model (k3) must
        // clear a stale thinking_budget left over from a budget-semantic model
        // (kimi-for-coding) — otherwise the bottom bar renders `think high(32K)`.
        let _lock = MODELS_TEST_LOCK.lock().await;
        tact_llm::clear_models_cache_for_tests();
        install_models_config_with_budget(vec!["k3"], "k3", 32_000);
        tact_llm::seed_models_cache_for_tests(
            "https://api.moonshot.cn/v1",
            "sk-test",
            vec!["k3".into()],
        );
        let mut app = make_app();
        start_model_picker(&mut app);
        handle_select_mode(&mut app, key(KeyCode::Enter)); // k3 (single model)
        handle_select_mode(&mut app, key(KeyCode::Enter)); // first effort (Low)

        assert_eq!(
            tact::config::settings().agent.reasoning_effort,
            Some(tact_llm::OpenAiReasoningEffort::Low)
        );
        assert_eq!(
            tact::config::settings().agent.thinking_budget,
            0,
            "effort pick must clear stale thinking budget"
        );
        assert_eq!(app.status_bar.model_thinking_budget, None);
        assert_eq!(
            app.status_bar.model_reasoning_effort,
            Some("low".to_string())
        );
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

        assert_eq!(tact::config::settings().llm.model, "kimi-for-coding");
        assert_eq!(tact::config::settings().agent.model, "kimi-for-coding");
        assert_eq!(tact::config::settings().agent.thinking_budget, 64_000);
        assert!(tact::config::settings().agent.max_tokens > 64_000);
        assert_eq!(app.status_bar.model_name, "kimi-for-coding");
        assert_eq!(app.status_bar.model_thinking_budget, Some(64_000));
        assert!(app.status_bar.model_max_tokens > 64_000);
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
        assert_eq!(tact::config::settings().llm.model, "kimi-k2.5");
        assert_eq!(tact::config::settings().agent.thinking_budget, 32_000);

        start_model_picker(&mut app);
        handle_select_mode(&mut app, key(KeyCode::Char('j')));
        handle_select_mode(&mut app, key(KeyCode::Enter));
        handle_select_mode(&mut app, key(KeyCode::Esc));

        assert_eq!(tact::config::settings().llm.model, "kimi-k2.5");
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

        assert_eq!(tact::config::settings().llm.model, "kimi-for-coding");
        assert_eq!(tact::config::settings().agent.model, "kimi-for-coding");
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
