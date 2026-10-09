use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tact_protocol::{InteractionResponse, RequestId};
use tact_view::UserCommand;

use crate::i18n::Language;
use crate::widgets::state::app::config::theme_label;
use crate::widgets::state::{App, InputMode, ModelTarget, SelectKind};

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
///
/// A *local* pick (`/model`, `/theme`, …) is also filterable: `/model` unions
/// the config list with `/v1/models`, which is long enough to need it. Agent
/// prompts are not — see [`SelectPopup::filterable`].
pub(crate) fn handle_select_mode(app: &mut App, key: KeyEvent) {
    if app.select.filterable() {
        match key.code {
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                app.select.push_query(c);
                return;
            }
            KeyCode::Backspace => {
                app.select.pop_query();
                return;
            }
            // Esc means "clear the filter" while there is one, and "cancel"
            // only once it is empty.
            KeyCode::Esc if !app.select.query.is_empty() => {
                app.select.clear_query();
                return;
            }
            _ => {}
        }
    }

    match key.code {
        KeyCode::Char(' ') if app.select.multi => {
            app.select.toggle_checked();
        }
        KeyCode::Enter => {
            // Nothing to confirm: no options at all, or a filter that matches
            // none of them. `confirm()` would otherwise hand back the stale
            // index of a row that is not on screen.
            if app.select.filtered_indices().is_empty() {
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
                let request_id = app.select.take_request_id();
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
                        if let Some(id) = request_id {
                            app.respond_ui(InteractionResponse::Selected {
                                request_id: RequestId::from(id.to_string()),
                                values: chosen.clone(),
                            });
                        }
                        if log_confirm {
                            let msgs = app.msgs();
                            app.add_system_message(msgs.selected_tmpl.replace("{}", &label));
                        }
                        app.input_mode = InputMode::Normal;
                        dequeue_next_agent_select(app);
                    }
                    // Multi is only opened for agent ask_user; local flows stay
                    // single-select, so any non-agent kind just closes cleanly.
                    _ => {
                        app.input_mode = InputMode::Normal;
                    }
                }
                return;
            }

            let idx = app.select.confirm().unwrap_or(0);
            let request_id = app.select.take_request_id();
            let chosen = app
                .select
                .options
                .get(idx)
                .cloned()
                .unwrap_or_else(|| "?".to_string());

            match std::mem::replace(&mut app.select_kind, SelectKind::Agent) {
                SelectKind::Agent => {
                    if let Some(id) = request_id {
                        app.respond_ui(InteractionResponse::Selected {
                            request_id: RequestId::from(id.to_string()),
                            values: vec![chosen.clone()],
                        });
                    }
                    if log_confirm {
                        let msgs = app.msgs();
                        app.add_system_message(msgs.selected_tmpl.replace("{}", &chosen));
                    }
                    app.input_mode = InputMode::Normal;
                    dequeue_next_agent_select(app);
                }
                SelectKind::ViewSystemPrompt => {
                    let content = if idx == 0 {
                        Some((
                            "Raw system prompt template",
                            include_str!(
                                "../../../tact_extensions/src/prompt/system_prompt_template.md"
                            )
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
                            .and_then(|body| crate::system_prompt::assemble_prompt_view(&body).ok())
                            .map(|content| ("Assembled current system prompt", content))
                        })
                    };
                    let (title, content) = content.unwrap_or_else(|| (
                        "Assembled current system prompt",
                        "## Unavailable\n\nNo persisted LLM request with a system prompt is available for this session.".to_string(),
                    ));
                    app.system_prompt_popup = Some(crate::widgets::state::SystemPromptPopup {
                        title: title.to_string(),
                        source: content,
                        scroll: 0,
                    });
                    app.input_mode = InputMode::Normal;
                }
                SelectKind::PermissionModePick => {
                    let msgs = app.msgs();
                    let (mode_str, display_label) = match idx {
                        0 => ("default", msgs.permission_option_default),
                        1 => ("plan", msgs.permission_option_plan),
                        _ => ("auto", msgs.permission_option_auto),
                    };
                    app.status_bar_mut().permission_mode = mode_str.to_string();
                    app.add_system_message(msgs.permission_set_tmpl.replace("{}", display_label));
                    let _ = app
                        .user_cmd_tx
                        .send(UserCommand::SetPermissionMode(mode_str.to_string()));
                    app.input_mode = InputMode::Normal;
                }
                SelectKind::ThemePick => {
                    // The options are `ThemeName::all()` in order, so the index
                    // is the theme; the label carries a " *" marker for the
                    // current one, which is why the name is not parsed back out
                    // of `chosen`.
                    let name = crate::theme::ThemeName::all()
                        .get(idx)
                        .copied()
                        .unwrap_or(app.theme.name);
                    app.apply_theme(name);
                    open_theme_persist_step(app, name, app.ui_config_available());
                }
                SelectKind::PersistTheme { name } => {
                    finish_theme_persist(app, &chosen, name);
                }
                SelectKind::PersistLang { language } => {
                    finish_language_persist(app, &chosen, language);
                }
                SelectKind::ModelPick(target) => {
                    open_second_step(app, strip_current_marker(&chosen), target);
                }
                SelectKind::ModelProfileEffortPick {
                    target,
                    model,
                    efforts,
                } => {
                    let effort = efforts.get(idx).copied().unwrap_or_else(|| {
                        efforts
                            .last()
                            .copied()
                            .unwrap_or(tact_llm::OpenAiReasoningEffort::Medium)
                    });
                    apply_model_and_effort_pick(app, target, model, effort);
                }
                SelectKind::ThinkBudgetPick {
                    target,
                    model,
                    budgets,
                } => {
                    let thinking_budget = budgets
                        .get(idx)
                        .copied()
                        .unwrap_or(*budgets.last().unwrap_or(&0));
                    apply_model_and_budget_pick(app, target, model, thinking_budget);
                }
                SelectKind::PersistModelAndBudget {
                    target,
                    model,
                    thinking_budget,
                } => {
                    finish_persist_budget(app, target, &chosen, &model, thinking_budget);
                }
                SelectKind::PersistModelAndEffort {
                    target,
                    model,
                    effort,
                } => {
                    finish_persist_effort(app, target, &chosen, &model, effort);
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
            let was_agent = app.select.request_id.is_some();
            if let Some(response) = app.select.cancel() {
                app.respond_ui(response);
            }
            let msgs = app.msgs();
            match std::mem::replace(&mut app.select_kind, SelectKind::Agent) {
                SelectKind::PersistModelAndBudget {
                    target,
                    model,
                    thinking_budget,
                } => {
                    let budget_label = format_thinking_budget(thinking_budget);
                    let tmpl = match target {
                        ModelTarget::Main => msgs.model_session_only_with_budget_tmpl,
                        ModelTarget::Subagent => msgs.model_subagent_session_only_with_budget_tmpl,
                    };
                    app.add_system_message(format_model_and_budget(tmpl, &model, &budget_label));
                }
                SelectKind::PersistModelAndEffort { model, effort, .. } => {
                    // Both targets share the effort session-only template.
                    app.add_system_message(
                        msgs.model_session_only_with_effort_tmpl
                            .replace("{}", &model)
                            .replace("{}", effort.as_str()),
                    );
                }
                // Esc on the "save to config?" step answers "no" rather than
                // discarding the theme: it is already applied, and the model
                // flows report the same thing on Esc.
                SelectKind::PersistTheme { name } => {
                    let label = theme_label(&msgs, name);
                    app.add_system_message(msgs.theme_session_only_tmpl.replace("{}", label));
                }
                SelectKind::PersistLang { language } => {
                    let label = language.label();
                    app.add_system_message(msgs.lang_session_only_tmpl.replace("{}", label));
                }
                SelectKind::Agent
                | SelectKind::ModelPick(_)
                | SelectKind::ModelProfileEffortPick { .. }
                | SelectKind::ThinkBudgetPick { .. }
                | SelectKind::ViewSystemPrompt
                | SelectKind::ThemePick
                | SelectKind::PermissionModePick => {
                    app.add_system_message(msgs.selection_cancelled.to_string());
                }
            }
            app.input_mode = InputMode::Normal;
            if was_agent {
                dequeue_next_agent_select(app);
            }
        }
        _ => {}
    }
}

fn strip_current_marker(label: &str) -> String {
    label.strip_suffix(" *").unwrap_or(label).to_string()
}

/// Pop the next queued agent select (if any) and show it. Called after the
/// current agent-originated select is confirmed or cancelled so concurrent
/// subagent permission prompts are served one at a time instead of hanging.
fn dequeue_next_agent_select(app: &mut App) {
    // Broker mode: the broker snapshot is the queue. Reconcile picks the next
    // request (if any) and keeps the legacy VecDeque unused.
    if app.pending_ui.is_some() {
        app.reconcile_pending_ui();
        return;
    }

    let Some(req) = app.pending_agent_selects.pop_front() else {
        return;
    };
    app.select_kind = SelectKind::Agent;
    if req.multi {
        app.select
            .set_multi(req.prompt, req.options, req.request_id, req.log_confirm);
    } else {
        app.select
            .set(req.prompt, req.options, req.request_id, req.log_confirm);
    }
    app.input_mode = InputMode::Select;
}

/// `/model` second step: branch by the selected model's semantics.
fn open_second_step(app: &mut App, model: String, target: ModelTarget) {
    let provider = provider_for_second_step(target);
    let Some(provider) = provider else {
        app.input_mode = InputMode::Normal;
        return;
    };
    let profile = tact_extensions::config::try_settings()
        .and_then(|s| s.llm.model_profiles.get(&model).cloned());

    if tact_llm::model_uses_effort(&model, &provider) {
        let efforts = profile
            .and_then(|p| (!p.reasoning_efforts.is_empty()).then_some(p.reasoning_efforts))
            .unwrap_or_else(|| default_effort_tiers(&provider));
        open_effort_picker(app, target, model, efforts);
    } else {
        let budgets = profile
            .and_then(|p| (!p.thinking_budgets.is_empty()).then_some(p.thinking_budgets))
            .unwrap_or_else(|| THINKING_BUDGETS.to_vec());
        open_budget_picker(app, target, model, budgets);
    }
}

/// Provider identity used to decide the second step (main vs subagent).
fn provider_for_second_step(target: ModelTarget) -> Option<tact_llm::ProviderInfo> {
    match target {
        ModelTarget::Subagent => tact_extensions::config::try_settings()
            .and_then(|s| s.agent.subagent)
            .map(|sa| sa.provider),
        ModelTarget::Main => Some(tact_llm::get_provider()),
    }
}

fn open_effort_picker(
    app: &mut App,
    target: ModelTarget,
    model: String,
    efforts: Vec<tact_llm::OpenAiReasoningEffort>,
) {
    let msgs = app.msgs();
    // Default highlight: current session effort if listed, else first tier.
    let current = match target {
        ModelTarget::Subagent => tact_extensions::config::try_settings()
            .and_then(|s| s.agent.subagent)
            .and_then(|sa| sa.reasoning_effort),
        ModelTarget::Main => {
            tact_extensions::config::try_settings().and_then(|s| s.agent.reasoning_effort)
        }
    };
    let selected = current
        .and_then(|effort| efforts.iter().position(|e| *e == effort))
        .unwrap_or(0);
    let options: Vec<String> = efforts
        .iter()
        .map(|effort| effort_label(&msgs, *effort))
        .collect();
    app.select_kind = SelectKind::ModelProfileEffortPick {
        target,
        model,
        efforts,
    };
    app.select.set_local(
        msgs.model_effort_prompt.to_string(),
        options,
        selected,
        false,
    );
    app.input_mode = InputMode::Select;
}

fn open_budget_picker(app: &mut App, target: ModelTarget, model: String, budgets: Vec<usize>) {
    let msgs = app.msgs();
    // Current value differs per target: subagent budget vs main agent budget.
    let thinking_budget = match target {
        ModelTarget::Subagent => tact_extensions::config::try_settings()
            .and_then(|s| s.agent.subagent.as_ref().map(|sa| sa.thinking_budget))
            .unwrap_or_default(),
        ModelTarget::Main => tact_extensions::config::try_settings()
            .map(|settings| settings.agent.thinking_budget)
            .unwrap_or_default(),
    };
    let selected = nearest_budget_index(&budgets, thinking_budget);
    let options = budget_option_labels(&msgs, &budgets);
    app.select_kind = SelectKind::ThinkBudgetPick {
        target,
        model,
        budgets,
    };
    app.select.set_local(
        msgs.model_thinking_budget_prompt.to_string(),
        options,
        selected,
        false,
    );
    app.input_mode = InputMode::Select;
}

/// Budget-semantic apply (model + thinking budget), parameterized by target.
///
/// Main agent: updates the LLM provider config + status bar and informs the
/// agent through `UserCommand`s. Subagent: updates only the subagent config.
/// Both then share the same "persist to config?" tail.
fn apply_model_and_budget_pick(
    app: &mut App,
    target: ModelTarget,
    model: String,
    thinking_budget: usize,
) {
    let msgs = app.msgs();
    if target == ModelTarget::Main && model.trim().is_empty() {
        app.add_system_message(
            msgs.model_switch_failed_tmpl
                .replace("{}", "model must not be empty"),
        );
        app.input_mode = InputMode::Normal;
        return;
    }
    let budget_label = format_thinking_budget(thinking_budget);

    match target {
        ModelTarget::Main => {
            let _ = app.user_cmd_tx.send(UserCommand::SetModel(model.clone()));
            tact_extensions::config::update_llm_model_and_thinking_budget(
                model.clone(),
                thinking_budget,
            );
            app.status_bar_mut().model_name = model.clone();
            if let Some(settings) = tact_extensions::config::try_settings() {
                // Keep out/think in sync immediately; agent may still be busy so
                // SetModel / SetThinkingBudget (and their ModelInfo) can arrive later.
                app.status_bar_mut().model_max_tokens = settings.agent.max_tokens;
            }
            app.status_bar_mut().model_thinking_budget =
                (thinking_budget > 0).then_some(thinking_budget as u32);
            app.status_bar_mut().model_reasoning_effort = None; // budget semantics: no derived effort
            app.add_system_message(format_model_and_budget(
                msgs.model_switched_with_budget_tmpl,
                &model,
                &budget_label,
            ));
            let _ = app
                .user_cmd_tx
                .send(UserCommand::SetThinkingBudget(thinking_budget));
        }
        ModelTarget::Subagent => {
            tact_extensions::config::update_subagent_model(model.clone(), thinking_budget);
            app.add_system_message(format_model_and_budget(
                msgs.model_subagent_switched_with_budget_tmpl,
                &model,
                &budget_label,
            ));
        }
    }

    let Some(settings) = tact_extensions::config::try_settings() else {
        app.input_mode = InputMode::Normal;
        return;
    };
    let (session_only_tmpl, persist_prompt) = match target {
        ModelTarget::Main => (
            msgs.model_session_only_with_budget_tmpl,
            msgs.model_persist_with_budget_prompt,
        ),
        ModelTarget::Subagent => (
            msgs.model_subagent_session_only_with_budget_tmpl,
            msgs.model_subagent_persist_with_budget_prompt,
        ),
    };
    if settings.config_path.is_none() {
        app.add_system_message(format_model_and_budget(
            session_only_tmpl,
            &model,
            &budget_label,
        ));
        app.input_mode = InputMode::Normal;
        return;
    }

    app.select_kind = SelectKind::PersistModelAndBudget {
        target,
        model,
        thinking_budget,
    };
    app.select.set_local(
        persist_prompt.to_string(),
        vec![msgs.persist_yes.to_string(), msgs.persist_no.to_string()],
        1,
        false,
    );
    app.input_mode = InputMode::Select;
}

/// Effort-semantic apply (openai / deepseek / kimi k3): model + effort, no
/// budget. Parameterized by target like [`apply_model_and_budget_pick`].
fn apply_model_and_effort_pick(
    app: &mut App,
    target: ModelTarget,
    model: String,
    effort: tact_llm::OpenAiReasoningEffort,
) {
    let msgs = app.msgs();
    match target {
        ModelTarget::Main => {
            let _ = app.user_cmd_tx.send(UserCommand::SetModel(model.clone()));
            tact_extensions::config::update_llm_model_and_reasoning_effort(
                model.clone(),
                Some(effort),
            );
            app.status_bar_mut().model_name = model.clone();
            if let Some(settings) = tact_extensions::config::try_settings() {
                app.status_bar_mut().model_max_tokens = settings.agent.max_tokens;
            }
            app.status_bar_mut().model_reasoning_effort = Some(effort.as_str().to_string());
            app.status_bar_mut().model_thinking_budget = None; // effort semantics: budget not shown
            let _ = app.user_cmd_tx.send(UserCommand::SetReasoningEffort(Some(
                effort.as_str().to_string(),
            )));
        }
        ModelTarget::Subagent => {
            let current_budget = tact_extensions::config::try_settings()
                .and_then(|s| s.agent.subagent.as_ref().map(|sa| sa.thinking_budget))
                .unwrap_or_default();
            tact_extensions::config::update_subagent_model(model.clone(), current_budget);
            tact_extensions::config::update_subagent_reasoning_effort(Some(effort));
        }
    }
    app.add_system_message(
        msgs.model_effort_switched_tmpl
            .replace("{}", &model)
            .replace("{}", effort.as_str()),
    );

    open_effort_persist_prompt(app, target, model, effort);
}

/// Shared effort persist flow: if no config file, session-only message; else
/// ask whether to persist model + effort. `target` selects the persist API
/// (main agent vs subagent) used once the user answers.
fn open_effort_persist_prompt(
    app: &mut App,
    target: ModelTarget,
    model: String,
    effort: tact_llm::OpenAiReasoningEffort,
) {
    let msgs = app.msgs();
    let Some(settings) = tact_extensions::config::try_settings() else {
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
    app.select_kind = SelectKind::PersistModelAndEffort {
        target,
        model,
        effort,
    };
    app.select.set_local(
        msgs.model_persist_with_effort_prompt.to_string(),
        vec![msgs.persist_yes.to_string(), msgs.persist_no.to_string()],
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
    if chosen == msgs.persist_yes {
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

fn finish_persist_budget(
    app: &mut App,
    target: ModelTarget,
    chosen: &str,
    model: &str,
    thinking_budget: usize,
) {
    let msgs = app.msgs();
    let budget_label = format_thinking_budget(thinking_budget);
    let (persisted_tmpl, session_only_tmpl) = match target {
        ModelTarget::Main => (
            msgs.model_persisted_with_budget_tmpl,
            msgs.model_session_only_with_budget_tmpl,
        ),
        ModelTarget::Subagent => (
            msgs.model_subagent_persisted_with_budget_tmpl,
            msgs.model_subagent_session_only_with_budget_tmpl,
        ),
    };
    if chosen == msgs.persist_yes {
        let result = match target {
            ModelTarget::Main => {
                tact_extensions::config::persist_active_provider_model_and_thinking_budget(
                    model,
                    thinking_budget,
                )
            }
            ModelTarget::Subagent => {
                tact_extensions::config::persist_subagent_model(model, thinking_budget)
            }
        };
        match result {
            Ok(()) => app.add_system_message(format_model_and_budget(
                persisted_tmpl,
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
            session_only_tmpl,
            model,
            &budget_label,
        ));
    }
    app.input_mode = InputMode::Normal;
}

/// `/theme` second step: offer to write `[ui] theme` to the config file.
///
/// No config file means the choice stays session-only, and that is reported
/// here rather than by opening a picker whose answer could not be honoured.
/// `config_available` is a parameter so both branches are testable without
/// installing process-global settings; production passes
/// [`App::ui_config_available`].
fn open_theme_persist_step(app: &mut App, name: crate::theme::ThemeName, config_available: bool) {
    let msgs = app.msgs();
    if !config_available {
        let label = theme_label(&msgs, name);
        app.add_system_message(msgs.theme_session_only_tmpl.replace("{}", label));
        app.input_mode = InputMode::Normal;
        return;
    }
    app.select_kind = SelectKind::PersistTheme { name };
    app.select.set_local(
        msgs.theme_persist_prompt.to_string(),
        vec![msgs.persist_yes.to_string(), msgs.persist_no.to_string()],
        // Default to No: writing a file is the deliberate choice, and the
        // theme is already applied either way.
        1,
        false,
    );
    app.input_mode = InputMode::Select;
}

/// Apply the answer to the theme's "save to config?" step.
fn finish_theme_persist(app: &mut App, chosen: &str, name: crate::theme::ThemeName) {
    let msgs = app.msgs();
    let label = theme_label(&msgs, name);
    if chosen == msgs.persist_yes {
        match tact_extensions::config::persist_theme(name.as_str()) {
            Ok(()) => {
                app.add_system_message(msgs.theme_persisted_tmpl.replace("{}", name.as_str()))
            }
            Err(error) => app.add_system_message(
                msgs.theme_persist_failed_tmpl
                    .replace("{}", &error.to_string()),
            ),
        }
    } else {
        app.add_system_message(msgs.theme_session_only_tmpl.replace("{}", label));
    }
    app.input_mode = InputMode::Normal;
}

/// `/lang`: flip the language, then ask whether the choice should be saved.
///
/// One entry point because the flip and its persist step are one flow — the
/// same reason `/theme`'s picker and its persist step share a caller. The
/// config probe stays inside so no caller can ask without a file to ask about.
pub(crate) fn start_language_toggle(app: &mut App) {
    app.apply_language(app.language.next());
    let language = app.language;
    open_language_persist_step(app, language, app.ui_config_available());
}

/// `/lang` second step: offer to write `[ui] language` to the config file.
///
/// No config file means the locale reverts on the next launch, and saying so
/// here is the point — a silent revert would look like the write failed.
/// `config_available` is a parameter so both branches are testable without
/// installing process-global settings.
fn open_language_persist_step(app: &mut App, language: Language, config_available: bool) {
    let msgs = app.msgs();
    if !config_available {
        let label = language.label();
        app.add_system_message(msgs.lang_session_only_tmpl.replace("{}", label));
        app.input_mode = InputMode::Normal;
        return;
    }
    app.select_kind = SelectKind::PersistLang { language };
    app.select.set_local(
        msgs.lang_persist_prompt.to_string(),
        vec![msgs.persist_yes.to_string(), msgs.persist_no.to_string()],
        // Default to No: writing a file is the deliberate choice, and the
        // language is already applied either way.
        1,
        false,
    );
    app.input_mode = InputMode::Select;
}

/// Apply the answer to the language's "save to config?" step.
///
/// The name written is [`Language::as_str`], never the display label — the
/// label is `中文` for the language a Chinese user just picked, which is not a
/// value the resolver could read back.
fn finish_language_persist(app: &mut App, chosen: &str, language: Language) {
    let msgs = app.msgs();
    let label = language.label();
    if chosen == msgs.persist_yes {
        match tact_extensions::config::persist_language(language.as_str()) {
            Ok(()) => {
                app.add_system_message(msgs.lang_persisted_tmpl.replace("{}", language.as_str()))
            }
            Err(error) => app.add_system_message(
                msgs.lang_persist_failed_tmpl
                    .replace("{}", &error.to_string()),
            ),
        }
    } else {
        app.add_system_message(msgs.lang_session_only_tmpl.replace("{}", label));
    }
    app.input_mode = InputMode::Normal;
}

fn finish_persist_effort(
    app: &mut App,
    target: ModelTarget,
    chosen: &str,
    model: &str,
    effort: tact_llm::OpenAiReasoningEffort,
) {
    finish_effort_persist(
        app,
        chosen,
        model,
        effort,
        |model, effort_str| match target {
            ModelTarget::Main => {
                tact_extensions::config::persist_active_provider_model_and_reasoning_effort(
                    model, effort_str,
                )
            }
            ModelTarget::Subagent => {
                tact_extensions::config::persist_subagent_model_and_reasoning_effort(
                    model, effort_str,
                )
            }
        },
    );
}

/// Open the `/model` SelectPopup from palette / slash command.
pub(crate) fn start_model_picker(app: &mut App) {
    let msgs = app.msgs();
    let Some(settings) = tact_extensions::config::try_settings() else {
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
    app.select_kind = SelectKind::ModelPick(ModelTarget::Main);
    app.select.set_local(prompt, options, selected, false);
    app.input_mode = InputMode::Select;
}

/// Open the `/model-subagent` SelectPopup from palette / slash command.
pub(crate) fn start_subagent_model_picker(app: &mut App) {
    let msgs = app.msgs();
    let Some(settings) = tact_extensions::config::try_settings() else {
        app.add_system_message(msgs.model_subagent_not_configured.to_string());
        return;
    };
    let Some(subagent) = &settings.agent.subagent else {
        app.add_system_message(msgs.model_subagent_not_configured.to_string());
        return;
    };

    let api_ids = match tokio::runtime::Handle::try_current() {
        Ok(handle) => tokio::task::block_in_place(|| {
            handle.block_on(tact_llm::ensure_api_model_ids_for_provider(
                &subagent.provider,
            ))
        }),
        Err(_) => Vec::new(),
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
    app.select_kind = SelectKind::ModelPick(ModelTarget::Subagent);
    app.select.set_local(prompt, options, selected, false);
    app.input_mode = InputMode::Select;
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use tact_llm::{ProviderInfo, ProviderKind};
    use tempfile::TempDir;

    use super::*;
    use crate::render::test_harness::make_app;
    use tact_protocol::{InteractionResponse, RuntimeCommand, RuntimeEvent};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::empty())
    }

    /// `/theme` → the "save to config?" step, as if a config file existed.
    #[test]
    fn theme_persist_step_asks_and_defaults_to_no() {
        let mut app = make_app();

        open_theme_persist_step(&mut app, crate::theme::ThemeName::Nord, true);

        assert!(matches!(app.select_kind, SelectKind::PersistTheme { .. }));
        assert!(matches!(app.input_mode, InputMode::Select));
        assert_eq!(
            app.select.options,
            vec!["Yes".to_string(), "No".to_string()]
        );
        assert_eq!(
            app.select.selected, 1,
            "No is the default: writing a file is the deliberate answer"
        );
        assert_eq!(app.select.prompt, "Save theme to config?");
    }

    /// Without a config file there is nothing to offer — say so instead of
    /// asking a question whose "yes" cannot be honoured.
    #[test]
    fn theme_persist_step_reports_session_only_without_a_config_file() {
        let mut app = make_app();

        open_theme_persist_step(&mut app, crate::theme::ThemeName::Nord, false);

        assert!(matches!(app.input_mode, InputMode::Normal));
        assert!(!matches!(app.select_kind, SelectKind::PersistTheme { .. }));
        assert!(
            app.log
                .items
                .iter()
                .any(|item| item.raw.contains("session only")),
            "{:?}",
            app.log.items
        );
    }

    #[test]
    fn declining_to_persist_reports_a_session_only_theme() {
        let mut app = make_app();

        finish_theme_persist(&mut app, "No", crate::theme::ThemeName::Nord);

        assert!(matches!(app.input_mode, InputMode::Normal));
        assert!(
            app.log
                .items
                .iter()
                .any(|item| item.raw.contains("Nord") && item.raw.contains("session only")),
            "{:?}",
            app.log.items
        );
    }

    /// `/lang` → the "save to config?" step, as if a config file existed.
    #[test]
    fn language_persist_step_asks_and_defaults_to_no() {
        let mut app = make_app();

        open_language_persist_step(&mut app, Language::Chinese, true);

        assert!(matches!(app.select_kind, SelectKind::PersistLang { .. }));
        assert!(matches!(app.input_mode, InputMode::Select));
        assert_eq!(
            app.select.options,
            vec!["Yes".to_string(), "No".to_string()]
        );
        assert_eq!(
            app.select.selected, 1,
            "No is the default: writing a file is the deliberate answer"
        );
        assert_eq!(app.select.prompt, "Save language to config?");
    }

    /// Without a config file there is nothing to offer — say so instead of
    /// asking a question whose "yes" cannot be honoured.
    #[test]
    fn language_persist_step_reports_session_only_without_a_config_file() {
        let mut app = make_app();

        open_language_persist_step(&mut app, Language::Chinese, false);

        assert!(matches!(app.input_mode, InputMode::Normal));
        assert!(!matches!(app.select_kind, SelectKind::PersistLang { .. }));
        assert!(
            app.log
                .items
                .iter()
                .any(|item| item.raw.contains("session only")),
            "{:?}",
            app.log.items
        );
    }

    #[test]
    fn declining_to_persist_reports_a_session_only_language() {
        let mut app = make_app();

        finish_language_persist(&mut app, "No", Language::Chinese);

        assert!(matches!(app.input_mode, InputMode::Normal));
        assert!(
            app.log
                .items
                .iter()
                .any(|item| item.raw.contains("中文") && item.raw.contains("session only")),
            "{:?}",
            app.log.items
        );
    }

    /// The whole flow against a real file: the value written is the locale tag,
    /// never the label. `中文` is what the user just picked and what the UI
    /// draws, but the resolver reads `zh` back — a label in the file would be a
    /// silent no-op on the next launch.
    #[tokio::test(flavor = "multi_thread")]
    async fn confirming_save_writes_the_locale_tag_and_leaves_the_other_tables() {
        let _lock = MODELS_TEST_LOCK.lock().await;
        let (_temp_dir, path) = install_models_config_with_path(vec![], "kimi-k2.5", 0);
        let mut app = make_app();
        // The persist gate is the path captured at startup, not a global read.
        app.set_ui_config_path(Some(path.clone()));

        start_language_toggle(&mut app);
        assert_eq!(app.language, Language::Chinese);
        assert!(matches!(app.select_kind, SelectKind::PersistLang { .. }));

        // Default is "No"; step up to "Yes" and confirm. Driving by key rather
        // than calling the finisher keeps the option string the one the user
        // actually sees, in the language the toggle just switched to.
        handle_select_mode(&mut app, key(KeyCode::Up));
        handle_select_mode(&mut app, key(KeyCode::Enter));

        let config = std::fs::read_to_string(&path).unwrap();
        assert!(config.contains("language = \"zh\""), "{config}");
        assert!(
            config.contains("[llm.providers.kimi]"),
            "a [ui] write must not disturb the tables around it:\n{config}"
        );
        assert!(
            app.log
                .items
                .iter()
                .any(|item| item.raw.contains("language = \"zh\"")),
            "{:?}",
            app.log.items
        );
    }

    fn seed_select(app: &mut App) -> tokio::sync::mpsc::UnboundedReceiver<UserCommand> {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        // Swap in an observable command channel so tests can assert the
        // InteractionResponse the confirm/cancel path emits.
        app.user_cmd_tx = tx;
        app.select_kind = SelectKind::Agent;
        app.input_mode = InputMode::Select;
        app.select.set(
            "Pick one".into(),
            vec!["Allow once".into(), "Deny".into()],
            1,
            true,
        );
        rx
    }

    fn install_models_config(models: Vec<&str>, current: &str) {
        tact_extensions::config::install_or_override(tact_extensions::config::ResolvedConfig {
            llm: tact_extensions::config::LlmSettings {
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
            agent: tact_extensions::config::AgentSettings {
                model: current.into(),
                reasoning_effort: None,
                max_tokens: 8000,
                thinking_budget: 0,
                model_context_window: 500_000,
                notifications_enabled: false,
                snapshot_max_items: 80,
                max_token_usage_bodies:
                    tact_extensions::store::session_store::MAX_TOKEN_USAGE_BODIES,
                micro_compact_enabled: true,
                memory_enabled: true,
                skill_body_auto_inject: false,
                skill_dirs: Vec::new(),
                instruction_sources: tact_extensions::config::InstructionSources::default(),
                subagent: None,
            },
            ui: tact_extensions::config::UiSettings {
                theme: "retro".into(),
                language: "en".into(),
                vision_image: tact_extensions::config::VisionImageSettings {
                    compress: true,
                    max_edge: 1280,
                    jpeg_quality: 80,
                },
                hook_output: true,
            },
            tools: tact_extensions::config::ToolSettings {
                bash_timeout_secs: tact_extensions::config::ToolSettings::DEFAULT_BASH_TIMEOUT_SECS,
                bash_nice: tact_extensions::config::ToolSettings::DEFAULT_BASH_NICE,
                rtk_filter: false,
                sandbox: false,
            },
            voice: tact_extensions::config::VoiceSettings::disabled_defaults(),
            mcp: tact_extensions::config::McpSettings::default(),
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
        let mut cfg = tact_extensions::config::settings();
        cfg.agent.thinking_budget = thinking_budget;
        tact_extensions::config::install_or_override(cfg);
    }

    fn install_models_config_with_subagent(
        models: Vec<&str>,
        current: &str,
        subagent_model: &str,
        subagent_budget: usize,
        subagent_effort: Option<tact_llm::OpenAiReasoningEffort>,
    ) {
        install_models_config(models, current);
        let mut cfg = tact_extensions::config::settings();
        cfg.agent.subagent = Some(tact_extensions::config::SubagentSettings {
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
        tact_extensions::config::install_or_override(cfg);
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
        let mut cfg = tact_extensions::config::settings();
        cfg.config_path = Some(path.clone());
        tact_extensions::config::install_or_override(cfg);
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
    fn typing_filters_a_local_pick() {
        let mut app = make_app();
        app.input_mode = InputMode::Select;
        app.select.set_local(
            "Select model".into(),
            vec!["kimi-for-coding".into(), "claude-sonnet".into()],
            0,
            true,
        );

        for c in "cl".chars() {
            handle_select_mode(&mut app, key(KeyCode::Char(c)));
        }

        assert_eq!(app.select.query, "cl");
        assert_eq!(app.select.filtered_indices(), vec![1]);
        assert_eq!(app.select.selected, 1, "the cursor follows the filter");
    }

    #[test]
    fn escape_clears_the_filter_before_it_cancels() {
        let mut app = make_app();
        app.input_mode = InputMode::Select;
        app.select
            .set_local("Select model".into(), vec!["a".into(), "b".into()], 0, true);

        handle_select_mode(&mut app, key(KeyCode::Char('b')));
        assert_eq!(app.select.query, "b");

        handle_select_mode(&mut app, key(KeyCode::Esc));
        assert!(
            app.select.query.is_empty(),
            "the first Esc clears the filter"
        );
        assert!(
            matches!(app.input_mode, InputMode::Select),
            "…and leaves the popup open"
        );

        handle_select_mode(&mut app, key(KeyCode::Esc));
        assert!(matches!(app.input_mode, InputMode::Normal));
    }

    #[test]
    fn enter_with_a_filter_that_matches_nothing_confirms_nothing() {
        let mut app = make_app();
        app.input_mode = InputMode::Select;
        app.select
            .set_local("Select model".into(), vec!["a".into(), "b".into()], 0, true);

        handle_select_mode(&mut app, key(KeyCode::Char('z')));
        assert!(app.select.filtered_indices().is_empty());
        handle_select_mode(&mut app, key(KeyCode::Enter));

        assert!(matches!(app.input_mode, InputMode::Normal));
        let expected = app.msgs().no_options;
        assert!(
            app.log.items.iter().any(|item| item.raw.contains(expected)),
            "an empty filter must report no options rather than confirming a \
             row that is not on screen, got {:?}",
            app.log.items
        );
    }

    #[test]
    fn an_agent_prompt_ignores_typed_characters() {
        let mut app = make_app();
        let _rx = seed_select(&mut app);
        assert!(!app.select.filterable());

        // `Deny` is the second row; typing must neither filter nor move.
        handle_select_mode(&mut app, key(KeyCode::Char('D')));
        assert!(
            app.select.query.is_empty(),
            "a keystroke must not hide the choices an agent is waiting on"
        );
        assert_eq!(app.select.selected, 0);
        assert_eq!(app.select.filtered_indices(), vec![0, 1]);

        // `j`/`k` are still navigation there.
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
        match rx.try_recv() {
            Ok(UserCommand::Runtime(RuntimeCommand::RespondInteraction {
                response: InteractionResponse::Selected { request_id, values },
            })) if request_id.as_str() == "1" && values == ["Deny"] => {}
            other => panic!("expected protocol Select response, got {other:?}"),
        }
        assert!(
            app.log.items.iter().any(|item| {
                item.raw.contains("Deny")
                    || item.raw.contains("Selected")
                    || item.raw.contains("已选择")
            }),
            "log_confirm should render selection in the log: {:?}",
            app.log.items
        );
    }

    #[test]
    fn broker_mode_enter_wakes_registered_waiter() {
        let mut app = make_app();
        let (tx, mut user_rx) = tokio::sync::mpsc::unbounded_channel();
        app.user_cmd_tx = tx;
        let responder = tact_extensions::ui_responder::UiResponder::new();
        app.set_pending_ui(responder.clone());

        let (request_id, mut waiter) = responder.register_select(
            "Allow write?".into(),
            vec!["Allow once".into(), "Deny".into()],
            false,
        );
        app.reconcile_pending_ui();
        assert_eq!(app.select.request_id, Some(request_id));

        handle_select_mode(&mut app, key(KeyCode::Enter));

        let UserCommand::Runtime(RuntimeCommand::RespondInteraction { response }) =
            user_rx.try_recv().unwrap()
        else {
            panic!("expected Runtime interaction response")
        };
        assert!(responder.respond(response));
        match waiter.try_recv() {
            Ok(tact_protocol::InteractionResponse::Selected {
                request_id: id,
                values,
            }) => {
                assert_eq!(id.as_str(), request_id.to_string());
                assert_eq!(values, vec!["Allow once".to_string()]);
            }
            other => panic!("expected broker Selected response, got {other:?}"),
        }
        assert!(responder.snapshot().is_empty());
    }

    #[test]
    fn broker_mode_cancel_answers_pending_select_with_none() {
        let mut app = make_app();
        let (tx, mut user_rx) = tokio::sync::mpsc::unbounded_channel();
        app.user_cmd_tx = tx;
        let responder = tact_extensions::ui_responder::UiResponder::new();
        app.set_pending_ui(responder.clone());

        let (request_id, mut waiter) = responder.register_select(
            "Allow write?".into(),
            vec!["Allow once".into(), "Deny".into()],
            false,
        );
        app.reconcile_pending_ui();
        assert_eq!(app.select.request_id, Some(request_id));

        app.cancel_task();

        let UserCommand::Runtime(RuntimeCommand::RespondInteraction { response }) =
            user_rx.try_recv().unwrap()
        else {
            panic!("expected protocol cancellation response")
        };
        assert!(responder.respond(response));
        match waiter.try_recv() {
            Ok(tact_protocol::InteractionResponse::Cancelled { request_id: id }) => {
                assert_eq!(id.as_str(), request_id.to_string());
            }
            other => panic!("expected cancelled broker response, got {other:?}"),
        }
        match user_rx.try_recv() {
            Ok(UserCommand::Cancel) => {}
            other => panic!("expected UserCommand::Cancel, got {other:?}"),
        }
    }

    #[test]
    fn esc_cancels_and_sends_none() {
        let mut app = make_app();
        let mut rx = seed_select(&mut app);

        handle_select_mode(&mut app, key(KeyCode::Esc));

        assert!(matches!(app.input_mode, InputMode::Normal));
        match rx.try_recv() {
            Ok(UserCommand::Runtime(RuntimeCommand::RespondInteraction {
                response: InteractionResponse::Cancelled { request_id },
            })) if request_id.as_str() == "1" => {}
            other => panic!("expected protocol cancellation response, got {other:?}"),
        }
    }

    #[test]
    fn concurrent_agent_selects_queue_and_drain_in_order() {
        let mut app = make_app();
        let mut rx = seed_select(&mut app); // request_id = 1

        // A second agent select arrives while the first is open → queued, not
        // overwritten (the overwrite would hang the first subagent's waiter).
        app.handle_runtime_event(RuntimeEvent::InteractionRequested {
            request: tact_protocol::InteractionRequest::Select {
                prompt: "Second".into(),
                options: vec!["Yes".into(), "No".into()],
                request_id: tact_protocol::RequestId::from(2.to_string()),
                log_confirm: false,
            },
        });
        assert_eq!(app.select.request_id, Some(1), "first select stays open");
        assert_eq!(app.pending_agent_selects.len(), 1, "second select queued");

        // Confirm the first → emits Select(1) and dequeues the second.
        handle_select_mode(&mut app, key(KeyCode::Enter));
        match rx.try_recv() {
            Ok(UserCommand::Runtime(RuntimeCommand::RespondInteraction {
                response: InteractionResponse::Selected { request_id, .. },
            })) if request_id.as_str() == "1" => {}
            other => panic!("expected first protocol Select response, got {other:?}"),
        }
        assert_eq!(app.select.request_id, Some(2), "second select dequeued");
        assert!(matches!(app.input_mode, InputMode::Select));

        // Confirm the second → emits Select(2) and returns to Normal.
        handle_select_mode(&mut app, key(KeyCode::Enter));
        match rx.try_recv() {
            Ok(UserCommand::Runtime(RuntimeCommand::RespondInteraction {
                response: InteractionResponse::Selected { request_id, .. },
            })) if request_id.as_str() == "2" => {}
            other => panic!("expected second protocol Select response, got {other:?}"),
        }
        assert!(matches!(app.input_mode, InputMode::Normal));
        assert!(app.pending_agent_selects.is_empty());
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
            app.log
                .items
                .iter()
                .any(|item| item.raw.contains("models") || item.raw.contains("models =")),
            "expected empty-models hint, got {:?}",
            app.log.items
        );

        install_models_config(vec!["kimi-k2.5", "kimi-for-coding"], "kimi-k2.5");
        start_model_picker(&mut app);
        assert!(matches!(app.input_mode, InputMode::Select));
        assert!(matches!(
            app.select_kind,
            SelectKind::ModelPick(ModelTarget::Main)
        ));

        handle_select_mode(&mut app, key(KeyCode::Down));
        handle_select_mode(&mut app, key(KeyCode::Enter));
        handle_select_mode(&mut app, key(KeyCode::Enter));

        assert_eq!(
            tact_extensions::config::settings().llm.model,
            "kimi-for-coding"
        );
        assert_eq!(
            tact_extensions::config::settings().agent.model,
            "kimi-for-coding"
        );
        assert_eq!(app.status_bar_mut().model_name, "kimi-for-coding");
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
        app.status_bar_mut().model_name = "status-before".to_string();
        app.status_bar_mut().model_thinking_budget = Some(32_000);
        start_model_picker(&mut app);
        handle_select_mode(&mut app, key(KeyCode::Down));
        handle_select_mode(&mut app, key(KeyCode::Enter));
        handle_select_mode(&mut app, key(KeyCode::Enter));

        assert_eq!(tact_extensions::config::settings().llm.model, "kimi-k2.5");
        assert_eq!(tact_extensions::config::settings().agent.model, "kimi-k2.5");
        assert_eq!(
            tact_extensions::config::settings().agent.thinking_budget,
            32_000
        );
        assert_eq!(app.status_bar_mut().model_name, "status-before");
        assert_eq!(app.status_bar_mut().model_thinking_budget, Some(32_000));
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
        handle_select_mode(&mut app, key(KeyCode::Down));
        handle_select_mode(&mut app, key(KeyCode::Enter));

        assert!(
            matches!(app.select_kind, SelectKind::ThinkBudgetPick { ref model, .. } if model == "kimi-for-coding")
        );
        assert_eq!(tact_extensions::config::settings().llm.model, "kimi-k2.5");
        assert_eq!(
            tact_extensions::config::settings().agent.thinking_budget,
            32_000
        );
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
        let mut cfg = tact_extensions::config::settings();
        cfg.agent.thinking_budget = 8_000;
        tact_extensions::config::install_or_override(cfg);

        let mut app = make_app();
        start_subagent_model_picker(&mut app);
        handle_select_mode(&mut app, key(KeyCode::Down));
        handle_select_mode(&mut app, key(KeyCode::Enter));

        assert!(matches!(
            app.select_kind,
            SelectKind::ThinkBudgetPick { target: ModelTarget::Subagent, ref model, .. } if model == "kimi-for-coding"
        ));
        // 64_000 is the 4th of the 5 default budgets (0, 8K, 32K, 64K, 128K).
        assert_eq!(app.select.selected, 3);
    }

    // --- B1: target-parameterized model flow ---------------------------------
    // The old code had two parallel variant families (main vs `Subagent*`) and
    // three parallel matches. These pin the behaviour preserved by folding them
    // into a single `ModelTarget`-parameterized flow.

    #[tokio::test(flavor = "multi_thread")]
    async fn subagent_budget_flow_uses_subagent_persist_templates() {
        let _lock = MODELS_TEST_LOCK.lock().await;
        tact_llm::clear_models_cache_for_tests();
        install_models_config_with_subagent(
            vec!["kimi-k2.5", "kimi-for-coding"],
            "kimi-k2.5",
            "kimi-for-coding",
            32_000, // subagent budget
            None,
        );
        tact_llm::seed_models_cache_for_tests(
            "https://api.moonshot.cn/v1",
            "sk-test",
            vec!["kimi-for-coding".into()],
        );
        // A config path makes the "persist?" prompt fire.
        let mut cfg = tact_extensions::config::settings();
        cfg.config_path = Some(std::path::PathBuf::from("/nonexistent/config.toml"));
        tact_extensions::config::install_or_override(cfg);

        let mut app = make_app();
        start_subagent_model_picker(&mut app);
        handle_select_mode(&mut app, key(KeyCode::Down)); // kimi-for-coding
        handle_select_mode(&mut app, key(KeyCode::Enter)); // open budget picker
        handle_select_mode(&mut app, key(KeyCode::Enter)); // confirm prefocused 32K

        // Must be the *subagent* persist prompt, not the main-agent one.
        assert!(
            matches!(
                app.select_kind,
                SelectKind::PersistModelAndBudget {
                    target: ModelTarget::Subagent,
                    ..
                }
            ),
            "expected subagent persist prompt, got {:?}",
            app.select_kind
        );
        assert_eq!(
            app.select.prompt,
            app.msgs().model_subagent_persist_with_budget_prompt
        );

        // Esc → session-only message uses the subagent template.
        handle_select_mode(&mut app, key(KeyCode::Esc));
        assert!(app.log.items.iter().any(|item| {
            item.raw
                .contains("Subagent model kimi-for-coding and thinking budget 32K")
        }));
        assert!(matches!(app.input_mode, InputMode::Normal));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn subagent_effort_pick_applies_subagent_reasoning_effort() {
        let _lock = MODELS_TEST_LOCK.lock().await;
        tact_llm::clear_models_cache_for_tests();
        // k3 is effort-semantic; subagent has no config path → session-only.
        install_models_config_with_subagent(vec!["k3"], "kimi-k2.5", "k3", 0, None);
        tact_llm::seed_models_cache_for_tests(
            "https://api.moonshot.cn/v1",
            "sk-test",
            vec!["k3".into()],
        );

        let mut app = make_app();
        start_subagent_model_picker(&mut app);
        handle_select_mode(&mut app, key(KeyCode::Enter)); // k3 (single model)
        assert!(matches!(
            app.select_kind,
            SelectKind::ModelProfileEffortPick {
                target: ModelTarget::Subagent,
                ..
            }
        ));
        handle_select_mode(&mut app, key(KeyCode::Enter)); // first effort (Low)

        assert_eq!(
            tact_extensions::config::settings()
                .agent
                .subagent
                .as_ref()
                .unwrap()
                .reasoning_effort,
            Some(tact_llm::OpenAiReasoningEffort::Low)
        );
        assert!(matches!(app.input_mode, InputMode::Normal));
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
        let mut cfg = tact_extensions::config::settings();
        cfg.agent.reasoning_effort = Some(tact_llm::OpenAiReasoningEffort::Max);
        tact_extensions::config::install_or_override(cfg);

        let mut app = make_app();
        start_model_picker(&mut app);
        handle_select_mode(&mut app, key(KeyCode::Down)); // k3-256k
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
            tact_extensions::config::settings().agent.reasoning_effort,
            Some(tact_llm::OpenAiReasoningEffort::Low)
        );
        assert_eq!(tact_extensions::config::settings().llm.model, "k3");
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
            tact_extensions::config::settings().agent.reasoning_effort,
            Some(tact_llm::OpenAiReasoningEffort::Low)
        );
        assert_eq!(
            tact_extensions::config::settings().agent.thinking_budget,
            0,
            "effort pick must clear stale thinking budget"
        );
        assert_eq!(app.status_bar_mut().model_thinking_budget, None);
        assert_eq!(
            app.status_bar_mut().model_reasoning_effort,
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
        handle_select_mode(&mut app, key(KeyCode::Down));
        handle_select_mode(&mut app, key(KeyCode::Enter));
        handle_select_mode(&mut app, key(KeyCode::Down));
        handle_select_mode(&mut app, key(KeyCode::Enter));

        assert_eq!(
            tact_extensions::config::settings().llm.model,
            "kimi-for-coding"
        );
        assert_eq!(
            tact_extensions::config::settings().agent.model,
            "kimi-for-coding"
        );
        assert_eq!(
            tact_extensions::config::settings().agent.thinking_budget,
            64_000
        );
        assert!(tact_extensions::config::settings().agent.max_tokens > 64_000);
        assert_eq!(app.status_bar_mut().model_name, "kimi-for-coding");
        assert_eq!(app.status_bar_mut().model_thinking_budget, Some(64_000));
        assert!(app.status_bar_mut().model_max_tokens > 64_000);
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

    /// `Ctrl+T` and `Ctrl+L` used to apply and announce only, so the choice was
    /// gone on the next launch. They now write the same `[ui]` keys the pickers'
    /// persist steps write, without asking — a question on every press would
    /// defeat the shortcut.
    #[tokio::test(flavor = "multi_thread")]
    async fn ctrl_t_and_ctrl_l_write_the_ui_preferences() {
        let _lock = MODELS_TEST_LOCK.lock().await;
        let (_temp_dir, path) = install_models_config_with_path(vec!["kimi-k2.5"], "kimi-k2.5", 0);

        let mut app = make_app();
        app.set_ui_config_path(Some(path.clone()));
        assert!(app.ui_config_available());

        app.toggle_theme();
        let theme = app.theme.name.as_str().to_string();
        app.toggle_language();
        let language = app.language.as_str().to_string();

        let config = std::fs::read_to_string(&path).unwrap();
        assert!(
            config.contains(&format!("theme = \"{theme}\"")),
            "[ui] theme must be written, got:\n{config}"
        );
        assert!(
            config.contains(&format!("language = \"{language}\"")),
            "[ui] language must be written, got:\n{config}"
        );
        // The rest of the file is the user's, and must survive.
        assert!(
            config.contains("model = \"kimi-k2.5\""),
            "the write must not clobber the rest of the config:\n{config}"
        );
    }

    /// With no config file the toggles must say so instead of pretending to
    /// save.
    #[test]
    fn ctrl_t_without_a_config_file_stays_session_only() {
        let mut app = make_app();
        assert!(!app.ui_config_available(), "the test default is no file");

        app.toggle_theme();

        let label = theme_label(&app.msgs(), app.theme.name);
        assert!(
            app.log
                .items
                .iter()
                .any(|item| item.raw.contains(label) && item.raw.contains("session")),
            "expected the session-only line, got {:?}",
            app.log.items
        );
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
        assert_eq!(tact_extensions::config::settings().llm.model, "kimi-k2.5");
        assert_eq!(
            tact_extensions::config::settings().agent.thinking_budget,
            32_000
        );

        start_model_picker(&mut app);
        handle_select_mode(&mut app, key(KeyCode::Down));
        handle_select_mode(&mut app, key(KeyCode::Enter));
        handle_select_mode(&mut app, key(KeyCode::Esc));

        assert_eq!(tact_extensions::config::settings().llm.model, "kimi-k2.5");
        assert_eq!(
            tact_extensions::config::settings().agent.thinking_budget,
            32_000
        );
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
        handle_select_mode(&mut app, key(KeyCode::Down));
        handle_select_mode(&mut app, key(KeyCode::Enter));
        handle_select_mode(&mut app, key(KeyCode::Down));
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
        handle_select_mode(&mut app, key(KeyCode::Down));
        handle_select_mode(&mut app, key(KeyCode::Enter));
        handle_select_mode(&mut app, key(KeyCode::Down));
        handle_select_mode(&mut app, key(KeyCode::Enter));
        assert!(matches!(
            app.select_kind,
            SelectKind::PersistModelAndBudget { .. }
        ));

        handle_select_mode(&mut app, key(KeyCode::Esc));

        assert_eq!(
            tact_extensions::config::settings().llm.model,
            "kimi-for-coding"
        );
        assert_eq!(
            tact_extensions::config::settings().agent.model,
            "kimi-for-coding"
        );
        assert_eq!(
            tact_extensions::config::settings().agent.thinking_budget,
            64_000
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
        assert!(app.log.items.iter().any(|message| {
            message.raw.contains(
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
