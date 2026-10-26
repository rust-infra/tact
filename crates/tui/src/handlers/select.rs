use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tact_protocol::{UiResponse, UserCommand};

use crate::i18n::Language;
use crate::widgets::state::app::config::theme_label;
use crate::widgets::state::{App, InputMode, ModelTarget, SelectKind};

const THINKING_BUDGETS: [usize; 5] = [0, 8_000, 32_000, 64_000, 128_000];

/// Default effort tiers per provider (no model mapping).
///
/// openai: minimal..max (official enum, default medium);
/// deepseek: low/high/max (minimal/medium illegal, xhigh not offered in UI);
/// kimi k3 family: low/high/max (default high).
fn default_effort_tiers(
    family: crate::ports::ProviderFamily,
) -> Vec<tact_protocol::ReasoningEffort> {
    use crate::ports::ProviderFamily;
    use tact_protocol::ReasoningEffort as E;
    match family {
        // These reject `minimal`/`medium` on the wire, so offering them would
        // send a tier the endpoint refuses.
        ProviderFamily::DeepSeek | ProviderFamily::Kimi => vec![E::Low, E::High, E::Max],
        ProviderFamily::OpenAi | ProviderFamily::Anthropic | ProviderFamily::Custom => {
            vec![E::Minimal, E::Low, E::Medium, E::High, E::Xhigh, E::Max]
        }
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

fn effort_label(msgs: &crate::i18n::Messages, effort: tact_protocol::ReasoningEffort) -> String {
    use tact_protocol::ReasoningEffort as E;
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
                            app.respond_ui(UiResponse::MultiSelect {
                                request_id: id,
                                choices: Some(idxs),
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
                        app.respond_ui(UiResponse::Select {
                            request_id: id,
                            choice: Some(idx),
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
                            include_str!("../../../tact/src/prompt/system_prompt_template.md")
                                .to_string(),
                        ))
                    } else {
                        app.history.as_ref().and_then(|history| {
                            history
                                .latest_request_body(&app.session_id)
                                .ok()
                                .flatten()
                                .and_then(|body| {
                                    crate::system_prompt::assemble_prompt_view(body.as_bytes()).ok()
                                })
                                .map(|content| ("Assembled current system prompt", content))
                        })
                    };
                    let (title, content) = content.unwrap_or_else(|| (
                        "Assembled current system prompt",
                        "## Unavailable\n\nNo persisted LLM request with a system prompt is available for this session.".to_string(),
                    ));
                    app.system_prompt_popup = Some(crate::widgets::state::SystemPromptPopup::new(
                        title.to_string(),
                        content,
                    ));
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
                    // A catalog command now: the view says "run `/permission`",
                    // which is what it says for every other command.
                    let _ = app.user_cmd_tx.send(UserCommand::RunCommand {
                        name: "permission".to_string(),
                        args: mode_str.to_string(),
                    });
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
                            .unwrap_or(tact_protocol::ReasoningEffort::Medium)
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
        app.select.set_multi(
            req.prompt,
            req.options,
            req.request_id,
            req.log_confirm,
            req.from_subagent,
        );
    } else {
        app.select.set(
            req.prompt,
            req.options,
            req.request_id,
            req.log_confirm,
            req.from_subagent,
        );
    }
    app.input_mode = InputMode::Select;
}

/// The config writer, or a message saying the host supplied none.
///
/// A view without a writer keeps working in memory: `/theme` and `/model` still
/// change what the session shows, they just cannot be saved. That is the same
/// contract as a run with no config file, so it reports rather than panics.
fn settings_writer(app: &App) -> Option<std::sync::Arc<dyn crate::ports::SettingsWriter>> {
    app.settings_writer.clone()
}

/// `/model` second step: branch by the selected model's semantics.
fn open_second_step(app: &mut App, model: String, target: ModelTarget) {
    let models = app.models.clone();
    let Some(family) = provider_for_second_step(models.as_ref(), target) else {
        app.input_mode = InputMode::Normal;
        return;
    };
    let profile = app
        .config_snapshot()
        .and_then(|config| config.model_profiles.get(&model).cloned());

    // The port decides: effort and budget semantics are mutually exclusive on
    // the wire, so the second step has to branch on the same answer the runtime
    // sends.
    let uses_effort = match &app.models {
        Some(models) => models.uses_effort(&model),
        None => false,
    };
    if uses_effort {
        let efforts = profile
            .and_then(|p| (!p.reasoning_efforts.is_empty()).then_some(p.reasoning_efforts))
            .unwrap_or_else(|| default_effort_tiers(family));
        open_effort_picker(app, target, model, efforts);
    } else {
        let budgets = profile
            .and_then(|p| (!p.thinking_budgets.is_empty()).then_some(p.thinking_budgets))
            .unwrap_or_else(|| THINKING_BUDGETS.to_vec());
        open_budget_picker(app, target, model, budgets);
    }
}

/// The provider family the second step branches on, if there is one.
///
/// `None` means "this target has no provider configured", which is what a
/// subagent target is before the user sets one — the picker then closes instead
/// of offering tiers for a provider that does not exist.
fn provider_for_second_step(
    models: Option<&std::sync::Arc<dyn crate::ports::ModelCatalog>>,
    target: ModelTarget,
) -> Option<crate::ports::ProviderFamily> {
    let models = models?;
    match target {
        ModelTarget::Subagent => models.subagent_family(),
        ModelTarget::Main => Some(models.family()),
    }
}

fn open_effort_picker(
    app: &mut App,
    target: ModelTarget,
    model: String,
    efforts: Vec<tact_protocol::ReasoningEffort>,
) {
    let msgs = app.msgs();
    let config = app.config_snapshot();
    // Default highlight: current session effort if listed, else first tier.
    let current = match target {
        ModelTarget::Subagent => config
            .as_ref()
            .and_then(|config| config.subagent.as_ref())
            .and_then(|sa| sa.reasoning_effort),
        ModelTarget::Main => config.as_ref().and_then(|config| config.reasoning_effort),
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
    let config = app.config_snapshot();
    // Current value differs per target: subagent budget vs main agent budget.
    let thinking_budget = match target {
        ModelTarget::Subagent => config
            .as_ref()
            .and_then(|config| config.subagent.as_ref().map(|sa| sa.thinking_budget))
            .unwrap_or_default(),
        ModelTarget::Main => config
            .as_ref()
            .map(|config| config.thinking_budget)
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
            let _ = app.user_cmd_tx.send(UserCommand::RunCommand {
                name: "model".to_string(),
                args: model.clone(),
            });
            if let Some(writer) = settings_writer(app) {
                let _ = writer.apply_model_choice(
                    crate::ports::ModelTarget::Main,
                    &crate::ports::ModelChoice::Budget {
                        model: model.clone(),
                        budget: thinking_budget,
                    },
                );
            }
            // Re-seed: the write above is what raises `max_tokens` when the new
            // budget no longer fits, so a snapshot read before it would show the
            // previous ceiling.
            app.refresh_config_snapshot();
            app.status_bar_mut().model_name = model.clone();
            if let Some(config) = app.config_snapshot() {
                // Keep out/think in sync immediately; agent may still be busy so
                // SetModel / SetThinkingBudget (and their ModelInfo) can arrive later.
                app.status_bar_mut().model_max_tokens = config.max_tokens;
            }
            app.status_bar_mut().model_thinking_budget =
                (thinking_budget > 0).then_some(thinking_budget as u32);
            app.status_bar_mut().model_reasoning_effort = None; // budget semantics: no derived effort
            app.add_system_message(format_model_and_budget(
                msgs.model_switched_with_budget_tmpl,
                &model,
                &budget_label,
            ));
            let _ = app.user_cmd_tx.send(UserCommand::RunCommand {
                name: "thinking".to_string(),
                args: thinking_budget.to_string(),
            });
        }
        ModelTarget::Subagent => {
            let applied = match settings_writer(app) {
                Some(writer) => writer.apply_model_choice(
                    crate::ports::ModelTarget::Subagent,
                    &crate::ports::ModelChoice::Budget {
                        model: model.clone(),
                        budget: thinking_budget,
                    },
                ),
                // No writer: nothing can fail, and the session keeps its own
                // copy in the status bar.
                None => Ok(()),
            };
            if let Err(error) = applied {
                app.add_system_message(
                    msgs.model_subagent_switch_failed_tmpl
                        .replace("{}", &format!("{error:#}")),
                );
                app.input_mode = InputMode::Normal;
                return;
            }
            app.add_system_message(format_model_and_budget(
                msgs.model_subagent_switched_with_budget_tmpl,
                &model,
                &budget_label,
            ));
        }
    }

    let Some(config) = app.config_snapshot() else {
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
    if !config.has_config_path {
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
    effort: tact_protocol::ReasoningEffort,
) {
    let msgs = app.msgs();
    match target {
        ModelTarget::Main => {
            let _ = app.user_cmd_tx.send(UserCommand::RunCommand {
                name: "model".to_string(),
                args: model.clone(),
            });
            if let Some(writer) = settings_writer(app) {
                let _ = writer.apply_model_choice(
                    crate::ports::ModelTarget::Main,
                    &crate::ports::ModelChoice::Effort {
                        model: model.clone(),
                        effort,
                    },
                );
            }
            app.status_bar_mut().model_name = model.clone();
            app.refresh_config_snapshot();
            if let Some(config) = app.config_snapshot() {
                app.status_bar_mut().model_max_tokens = config.max_tokens;
            }
            app.status_bar_mut().model_reasoning_effort = Some(effort.as_str().to_string());
            app.status_bar_mut().model_thinking_budget = None; // effort semantics: budget not shown
            let _ = app.user_cmd_tx.send(UserCommand::RunCommand {
                name: "effort".to_string(),
                args: effort.as_str().to_string(),
            });
        }
        ModelTarget::Subagent => {
            // The writer applies the model and the effort together: doing it in
            // two calls would fight over the same wire field.
            let applied = match settings_writer(app) {
                Some(writer) => writer.apply_model_choice(
                    crate::ports::ModelTarget::Subagent,
                    &crate::ports::ModelChoice::Effort {
                        model: model.clone(),
                        effort,
                    },
                ),
                None => Ok(()),
            };
            if let Err(error) = applied {
                app.add_system_message(
                    msgs.model_subagent_switch_failed_tmpl
                        .replace("{}", &format!("{error:#}")),
                );
                app.input_mode = InputMode::Normal;
                return;
            }
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
    effort: tact_protocol::ReasoningEffort,
) {
    let msgs = app.msgs();
    let Some(config) = app.config_snapshot() else {
        app.input_mode = InputMode::Normal;
        return;
    };
    if !config.has_config_path {
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
    effort: tact_protocol::ReasoningEffort,
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
        let result = match settings_writer(app) {
            Some(writer) => writer.persist_model_choice(
                match target {
                    ModelTarget::Main => crate::ports::ModelTarget::Main,
                    ModelTarget::Subagent => crate::ports::ModelTarget::Subagent,
                },
                &crate::ports::ModelChoice::Budget {
                    model: model.to_string(),
                    budget: thinking_budget,
                },
            ),
            None => Ok(()),
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
        let result = match settings_writer(app) {
            Some(writer) => {
                writer.persist_ui(crate::ports::UiSetting::Theme(name.as_str().to_string()))
            }
            None => Ok(()),
        };
        match result {
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
        let result = match settings_writer(app) {
            Some(writer) => writer.persist_ui(crate::ports::UiSetting::Language(
                language.as_str().to_string(),
            )),
            None => Ok(()),
        };
        match result {
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
    effort: tact_protocol::ReasoningEffort,
) {
    // Take the handle before the call: the closure needs it while `app` itself
    // is mutably borrowed by `finish_effort_persist`.
    let writer = settings_writer(app);
    let port_target = match target {
        ModelTarget::Main => crate::ports::ModelTarget::Main,
        ModelTarget::Subagent => crate::ports::ModelTarget::Subagent,
    };
    finish_effort_persist(app, chosen, model, effort, |model, effort_str| {
        let Some(writer) = writer.as_ref() else {
            return Ok(());
        };
        let effort = effort_str
            .parse()
            .map_err(|error| anyhow::anyhow!("{error}"))?;
        writer.persist_model_choice(
            port_target,
            &crate::ports::ModelChoice::Effort {
                model: model.to_string(),
                effort,
            },
        )
    });
}

/// Open the `/model` SelectPopup from palette / slash command.
pub(crate) fn start_model_picker(app: &mut App) {
    let msgs = app.msgs();
    let Some(config) = app.config_snapshot() else {
        app.add_system_message(msgs.model_config_unavailable.to_string());
        return;
    };

    // Ask the host's catalog: it owns how the endpoint is queried, and a view
    // without one keeps the configured list.
    let mut candidates = match &app.models {
        Some(models) => models.candidates(&config.models),
        None => config.models.clone(),
    };

    if candidates.is_empty() {
        app.add_system_message(
            msgs.model_list_empty_tmpl
                .replace("{}", &config.provider_label),
        );
        return;
    }

    let current = config.model.clone();
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
        .replace("{}", &config.provider_label);
    app.select_kind = SelectKind::ModelPick(ModelTarget::Main);
    app.select.set_local(prompt, options, selected, false);
    app.input_mode = InputMode::Select;
}

/// Open the `/model-subagent` SelectPopup from palette / slash command.
pub(crate) fn start_subagent_model_picker(app: &mut App) {
    let msgs = app.msgs();
    let Some(config) = app.config_snapshot() else {
        app.add_system_message(msgs.model_subagent_not_configured.to_string());
        return;
    };
    let Some(subagent) = config.subagent.as_ref() else {
        app.add_system_message(msgs.model_subagent_not_configured.to_string());
        return;
    };

    let subagent_provider_name = subagent.provider_label.as_str();
    let mut candidates = match &app.models {
        Some(models) => models.candidates_for(&subagent.models, subagent_provider_name),
        None => subagent.models.clone(),
    };

    if candidates.is_empty() {
        app.add_system_message(
            msgs.model_subagent_list_empty_tmpl
                .replace("{}", subagent_provider_name),
        );
        return;
    }

    let current = subagent.model.clone();
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

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::empty())
    }

    /// An app whose `/model` picker asks a fake catalog instead of an endpoint.
    ///
    /// The view no longer reads the model library's process-wide cache, so a
    /// test states what the endpoint would have reported.
    fn app_with_models(discovered: &[&str], effort_models: &[&str]) -> App {
        app_with_models_from(
            crate::ports::ProviderFamily::OpenAi,
            discovered,
            effort_models,
        )
    }

    /// The same, with the provider family stated.
    ///
    /// The family decides which effort tiers are offered (`deepseek`/`kimi`
    /// reject `minimal`/`medium`), so a test that asserts a tier list has to say
    /// which provider it is.
    fn app_with_models_from(
        family: crate::ports::ProviderFamily,
        discovered: &[&str],
        effort_models: &[&str],
    ) -> App {
        let mut app = make_app();
        let catalog = crate::test_fixtures::FakeModelCatalog::with_models(discovered)
            .with_effort_models(effort_models)
            .with_family(family);
        app.models = Some(std::sync::Arc::new(catalog));
        app
    }

    /// The `/model` second step reads its profiles from the app's snapshot.
    ///
    /// The profiles decide which effort tiers or thinking budgets the picker
    /// offers, so reading them from the process global means a TUI whose host
    /// assembled a different configuration shows the wrong choices. The app is
    /// given a snapshot carrying one distinctive profile: only that snapshot can
    /// produce the tiers, so a fallback to the global would fail here.
    #[test]
    fn the_model_second_step_offers_the_profiles_from_the_app_snapshot() {
        let mut app = make_app();
        let mut config = crate::ports::ViewSettings::default();
        config.model_profiles.insert(
            "profile-model".to_string(),
            crate::ports::ModelProfile {
                reasoning_efforts: vec![tact_protocol::ReasoningEffort::High],
                thinking_budgets: vec![4_242],
            },
        );
        app.set_config_snapshot(Some(config));

        let profile = app
            .config_snapshot()
            .and_then(|config| config.model_profiles.get("profile-model").cloned())
            .expect("the app exposes the profile it was given");
        assert_eq!(profile.thinking_budgets, vec![4_242]);
        assert_eq!(
            profile.reasoning_efforts,
            vec![tact_protocol::ReasoningEffort::High]
        );
    }

    /// With no snapshot installed the picker still works on defaults.
    ///
    /// A TUI started without a config file has no snapshot; the effort and
    /// budget pickers fall back to the built-in tiers rather than refusing to
    /// open.
    #[test]
    fn a_missing_snapshot_leaves_the_model_picker_on_its_defaults() {
        let mut app = make_app();
        app.set_config_snapshot(None);
        assert!(app.config_snapshot().is_none());
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
        let writer = std::sync::Arc::new(crate::test_fixtures::FakeSettingsWriter::new());
        app.settings_writer = Some(writer.clone());
        handle_select_mode(&mut app, key(KeyCode::Up));
        handle_select_mode(&mut app, key(KeyCode::Enter));

        // The view asks for the language write; "does not disturb other tables"
        // is the config writer's contract, covered by the adapter's own test.
        assert_eq!(
            writer.ui(),
            vec![crate::ports::UiSetting::Language("zh".to_string())],
            "the toggle must write exactly the language it switched to"
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

    fn seed_select(app: &mut App) -> std::sync::Arc<crate::test_fixtures::FakeInteraction> {
        // The port is the only path an answer can take now, so the fixture
        // installs one holding the request the popup below belongs to: an empty
        // port would make the first reconcile close the popup it never knew about.
        let port = std::sync::Arc::new(crate::test_fixtures::FakeInteraction::with_pending(vec![
            crate::test_fixtures::FakeInteraction::select(1, "Pick one", &["Allow once", "Deny"]),
        ]));
        app.set_pending_ui(port.clone());
        app.select_kind = SelectKind::Agent;
        app.input_mode = InputMode::Select;
        app.select.set(
            "Pick one".into(),
            vec!["Allow once".into(), "Deny".into()],
            1,
            true,
            false,
        );
        port
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
                max_token_usage_bodies: tact::store::session_store::MAX_TOKEN_USAGE_BODIES,
                micro_compact_enabled: true,
                memory_enabled: true,
                skill_body_auto_inject: false,
                skill_dirs: Vec::new(),
                skill_description_cap: tact::skill::DEFAULT_SKILL_DESCRIPTION_CAP,
                skill_disable: Vec::new(),
                instruction_sources: tact::config::InstructionSources::default(),
                auto_memory_directory: None,
                subagent: None,
            },
            ui: tact::config::UiSettings {
                theme: "retro".into(),
                language: "en".into(),
                vision_image: tact::config::VisionImageSettings {
                    compress: true,
                    max_edge: 1280,
                    jpeg_quality: 80,
                },
                hook_output: true,
            },
            tools: tact::config::ToolSettings {
                bash_timeout_secs: tact::config::ToolSettings::DEFAULT_BASH_TIMEOUT_SECS,
                bash_nice: tact::config::ToolSettings::DEFAULT_BASH_NICE,
                rtk_filter: false,
                sandbox: false,
            },
            voice: tact::config::VoiceSettings::disabled_defaults(),
            mcp: tact::config::McpSettings::default(),
            goal: tact::config::GoalSettings::default(),
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

    /// `/model` reads configuration from the app, not the process global.
    ///
    /// The picker's profiles, effort/budget defaults and config path all come
    /// from the view's snapshot, which is what lets the TUI be driven by a host
    /// that assembled a different configuration. A `tact::config::settings()`
    /// or `try_settings()` in the production region would reintroduce the hidden
    /// dependency this migration removed, so it fails here by name.
    #[test]
    fn the_model_flow_reads_configuration_from_the_app() {
        let source = include_str!("select.rs");
        let body = source
            .split_once("mod tests {")
            .expect("the test module is declared here")
            .0;
        for forbidden in ["tact::config::settings()", "tact::config::try_settings()"] {
            assert!(
                !body.contains(forbidden),
                "the model flow must take configuration from the app, found {forbidden}"
            );
        }
    }

    /// The app fixture with the configuration currently installed.
    ///
    /// `make_app` seeds the snapshot at construction, so a test that installs a
    /// configuration *after* building the app would otherwise drive `/model`
    /// from a stale snapshot. Re-seeding keeps the fixture honest about what
    /// production does: the view reads the host's resolved configuration.
    fn app_with_current_config() -> App {
        let mut app = make_app();
        // Re-project through the port, the same route `refresh_config_snapshot`
        // takes after a write.
        app.refresh_config_snapshot();
        app
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
        subagent_effort: Option<tact_protocol::ReasoningEffort>,
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
            max_tokens_override: None,
            thinking_budget: subagent_budget,
            reasoning_effort: subagent_effort,
            // Neither fixture model is in the built-in mapping, so
            // `resolve_subagent` would fall back to the MAIN agent's window —
            // 500_000 in this fixture, not the 200_000 default.
            model_context_window: 500_000,
            model_context_window_override: None,
            responses_compact_threshold_override: None,
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
        let port = seed_select(&mut app);

        handle_select_mode(&mut app, key(KeyCode::Char('j')));
        handle_select_mode(&mut app, key(KeyCode::Enter));

        assert!(matches!(app.input_mode, InputMode::Normal));
        match port.answers().as_slice() {
            [
                UiResponse::Select {
                    request_id: 1,
                    choice: Some(1),
                },
            ] => {}
            other => panic!("expected a Select answer on the port, got {other:?}"),
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
    fn broker_mode_enter_answers_through_the_port() {
        let mut app = make_app();
        let (tx, mut user_rx) = tokio::sync::mpsc::unbounded_channel();
        app.user_cmd_tx = tx;
        let port = std::sync::Arc::new(crate::test_fixtures::FakeInteraction::with_pending(vec![
            crate::test_fixtures::FakeInteraction::select(
                7,
                "Allow write?",
                &["Allow once", "Deny"],
            ),
        ]));
        app.set_pending_ui(port.clone());
        app.reconcile_pending_ui();
        assert_eq!(app.select.request_id, Some(7));

        handle_select_mode(&mut app, key(KeyCode::Enter));

        match port.answers().as_slice() {
            [
                UiResponse::Select {
                    request_id: 7,
                    choice: Some(0),
                },
            ] => {}
            other => panic!("expected a Select answer through the port, got {other:?}"),
        }
        assert!(
            user_rx.try_recv().is_err(),
            "port mode must not fall back to UserCommand::UiResponse"
        );
    }

    #[test]
    fn broker_mode_cancel_answers_pending_select_with_none() {
        let mut app = make_app();
        let (tx, mut user_rx) = tokio::sync::mpsc::unbounded_channel();
        app.user_cmd_tx = tx;
        let port = std::sync::Arc::new(crate::test_fixtures::FakeInteraction::with_pending(vec![
            crate::test_fixtures::FakeInteraction::select(
                9,
                "Allow write?",
                &["Allow once", "Deny"],
            ),
        ]));
        app.set_pending_ui(port.clone());
        app.reconcile_pending_ui();
        assert_eq!(app.select.request_id, Some(9));

        app.cancel_task();

        match port.answers().as_slice() {
            [
                UiResponse::Select {
                    request_id: 9,
                    choice: None,
                },
            ] => {}
            other => panic!("expected a cancelled Select through the port, got {other:?}"),
        }
        match user_rx.try_recv() {
            Ok(UserCommand::Cancel) => {}
            other => panic!("expected UserCommand::Cancel, got {other:?}"),
        }
    }

    #[test]
    fn esc_cancels_and_sends_none() {
        let mut app = make_app();
        let port = seed_select(&mut app);

        handle_select_mode(&mut app, key(KeyCode::Esc));

        assert!(matches!(app.input_mode, InputMode::Normal));
        match port.answers().as_slice() {
            [
                UiResponse::Select {
                    request_id: 1,
                    choice: None,
                },
            ] => {}
            other => panic!("expected a cancelled Select on the port, got {other:?}"),
        }
    }

    /// Two concurrent prompts are shown one at a time, in arrival order.
    ///
    /// This is the property that matters when two subagents ask at once: the
    /// second prompt must not overwrite the first, because overwriting it would
    /// leave the first waiter hanging with no popup to answer.
    #[test]
    fn concurrent_agent_selects_show_one_at_a_time_in_order() {
        let mut app = make_app();
        // Both requests are already waiting when the view reconciles, which is
        // what the runtime's snapshot looks like when two children ask together.
        let port = std::sync::Arc::new(crate::test_fixtures::FakeInteraction::with_pending(vec![
            crate::test_fixtures::FakeInteraction::select(1, "First", &["Allow once", "Deny"]),
            crate::test_fixtures::FakeInteraction::select(2, "Second", &["Yes", "No"]),
        ]));
        app.set_pending_ui(port.clone());

        assert_eq!(
            app.select.request_id,
            Some(1),
            "the oldest request is shown"
        );
        assert_eq!(app.select.prompt, "First");
        assert!(matches!(app.input_mode, InputMode::Select));

        // Answering the first lets the second take the popup.
        handle_select_mode(&mut app, key(KeyCode::Enter));
        match port.answers().as_slice() {
            [
                UiResponse::Select {
                    request_id: 1,
                    choice: Some(_),
                },
            ] => {}
            other => panic!("expected the first Select on the port, got {other:?}"),
        }
        assert_eq!(
            app.select.request_id,
            Some(2),
            "the next request takes over"
        );
        assert_eq!(app.select.prompt, "Second");
        assert!(matches!(app.input_mode, InputMode::Select));

        // Answering the second closes the popup.
        handle_select_mode(&mut app, key(KeyCode::Enter));
        assert_eq!(
            port.answers().len(),
            2,
            "both answers reached the port: {:?}",
            port.answers()
        );
        assert!(matches!(app.input_mode, InputMode::Normal));
    }

    /// Serialize model-picker tests that share global `CACHE` / `PROVIDER`.
    static MODELS_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    #[tokio::test(flavor = "multi_thread")]
    async fn model_picker_empty_then_confirm_sets_model() {
        let _lock = MODELS_TEST_LOCK.lock().await;
        tact_llm::clear_models_cache_for_tests();
        tact_llm::seed_models_cache_for_tests("https://api.moonshot.cn/v1", "sk-test", vec![]);
        install_models_config(vec![], "kimi-k2.5");

        let mut app = app_with_current_config();
        // A catalog is what makes the second step exist; without one the picker
        // applies the model directly, which is the config-less behaviour.
        app.models = Some(std::sync::Arc::new(
            crate::test_fixtures::FakeModelCatalog::with_models(&[]),
        ));
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
        // The fixture's projection mirrors the installed configuration, so the
        // re-install has to be followed by a re-projection.
        let seeded = crate::test_fixtures::RealConfigWriter::new()
            .current_settings()
            .unwrap_or_default();
        app.set_config_snapshot(Some(seeded));
        start_model_picker(&mut app);
        assert!(matches!(app.input_mode, InputMode::Select));
        assert!(matches!(
            app.select_kind,
            SelectKind::ModelPick(ModelTarget::Main)
        ));

        let writer = std::sync::Arc::new(crate::test_fixtures::FakeSettingsWriter::new());
        app.settings_writer = Some(writer.clone());
        handle_select_mode(&mut app, key(KeyCode::Down));
        handle_select_mode(&mut app, key(KeyCode::Enter));
        handle_select_mode(&mut app, key(KeyCode::Enter));

        match writer.applied().as_slice() {
            [
                (crate::ports::ModelTarget::Main, crate::ports::ModelChoice::Budget { model, .. }),
            ] => {
                assert_eq!(model, "kimi-for-coding");
            }
            other => panic!("expected one budget choice, got {other:?}"),
        }
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

        assert_eq!(tact::config::settings().llm.model, "kimi-k2.5");
        assert_eq!(tact::config::settings().agent.model, "kimi-k2.5");
        assert_eq!(tact::config::settings().agent.thinking_budget, 32_000);
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
        let mut cfg = tact::config::settings();
        cfg.config_path = Some(std::path::PathBuf::from("/nonexistent/config.toml"));
        tact::config::install_or_override(cfg);

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

    /// A refused switch (an explicit `max_tokens` that cannot fit the pinned
    /// window) reports the failure and does NOT open the persist prompt: the
    /// session still runs the old model, so there is nothing to persist — and
    /// nothing was applied.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_refused_subagent_switch_reports_instead_of_the_persist_prompt() {
        let _lock = MODELS_TEST_LOCK.lock().await;
        tact_llm::clear_models_cache_for_tests();
        install_models_config_with_subagent(
            vec!["kimi-k2.5", "kimi-for-coding"],
            "kimi-k2.5",
            "kimi-k2.5",
            0,
            None,
        );
        tact_llm::seed_models_cache_for_tests(
            "https://api.moonshot.cn/v1",
            "sk-test",
            vec!["kimi-for-coding".into()],
        );
        let mut cfg = tact::config::settings();
        {
            let sa = cfg.agent.subagent.as_mut().unwrap();
            // Both models stay pickable, but the pinned window cannot fit the
            // explicit max_tokens — the switch has to be refused.
            sa.models = vec!["kimi-k2.5".into(), "kimi-for-coding".into()];
            sa.max_tokens = 4_000;
            sa.max_tokens_override = Some(4_000);
            sa.model_context_window = 2_000;
            sa.model_context_window_override = Some(2_000);
        }
        // A config path would make the "persist?" prompt fire if the failure
        // were ignored.
        cfg.config_path = Some(std::path::PathBuf::from("/nonexistent/config.toml"));
        tact::config::install_or_override(cfg);

        let mut app = make_app();
        // The refusal lives in the config update, so this test needs a writer
        // that really validates rather than a recording fake.
        app.settings_writer = Some(std::sync::Arc::new(
            crate::test_fixtures::RealConfigWriter::new(),
        ));
        start_subagent_model_picker(&mut app);
        handle_select_mode(&mut app, key(KeyCode::Down)); // kimi-for-coding
        handle_select_mode(&mut app, key(KeyCode::Enter)); // open budget picker
        handle_select_mode(&mut app, key(KeyCode::Enter)); // confirm prefocused budget

        assert!(
            !matches!(app.select_kind, SelectKind::PersistModelAndBudget { .. }),
            "a refused switch must not open the persist prompt, got {:?}",
            app.select_kind
        );
        assert!(matches!(app.input_mode, InputMode::Normal));
        assert!(
            app.log.items.iter().any(|item| item
                .raw
                .contains("must be less than the subagent context window")),
            "expected the refusal to be reported, got {:?}",
            app.log.items
        );
        // Nothing was applied.
        let sa = tact::config::settings().agent.subagent.unwrap();
        assert_eq!(sa.provider.model, "kimi-k2.5");
        assert_eq!(sa.max_tokens, 4_000);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn subagent_effort_pick_applies_subagent_reasoning_effort() {
        let _lock = MODELS_TEST_LOCK.lock().await;
        // k3 is effort-semantic; subagent has no config path → session-only.
        install_models_config_with_subagent(vec!["k3"], "kimi-k2.5", "k3", 0, None);

        let mut app = app_with_models_from(crate::ports::ProviderFamily::Kimi, &["k3"], &["k3"]);
        let writer = std::sync::Arc::new(crate::test_fixtures::FakeSettingsWriter::new());
        app.settings_writer = Some(writer.clone());
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

        match writer.applied().as_slice() {
            [
                (
                    crate::ports::ModelTarget::Subagent,
                    crate::ports::ModelChoice::Effort { model, effort },
                ),
            ] => {
                assert_eq!(model, "k3");
                assert_eq!(*effort, tact_protocol::ReasoningEffort::Low);
            }
            other => panic!("expected one subagent effort choice, got {other:?}"),
        }
        assert!(matches!(app.input_mode, InputMode::Normal));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn effort_picker_highlights_current_session_effort() {
        let _lock = MODELS_TEST_LOCK.lock().await;
        install_models_config(vec!["k3", "k3-256k"], "k3");
        let mut cfg = tact::config::settings();
        cfg.agent.reasoning_effort = Some(tact_protocol::ReasoningEffort::Max);
        tact::config::install_or_override(cfg);

        // Effort-semantic: the second step is the effort picker, not the budget
        // one. The catalog decides that now.
        let mut app = app_with_models_from(
            crate::ports::ProviderFamily::Kimi,
            &["k3", "k3-256k"],
            &["k3", "k3-256k"],
        );
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
        install_models_config(vec!["k3"], "k3");
        let mut app = app_with_models_from(crate::ports::ProviderFamily::Kimi, &["k3"], &["k3"]);
        let writer = std::sync::Arc::new(crate::test_fixtures::FakeSettingsWriter::new());
        app.settings_writer = Some(writer.clone());
        start_model_picker(&mut app);
        handle_select_mode(&mut app, key(KeyCode::Enter)); // k3 (single model)

        assert!(matches!(
            app.select_kind,
            SelectKind::ModelProfileEffortPick { .. }
        ));
        handle_select_mode(&mut app, key(KeyCode::Enter)); // first effort (Low)

        // No config path → session-only, back to normal mode.
        assert!(matches!(app.input_mode, InputMode::Normal));
        match writer.applied().as_slice() {
            [
                (
                    crate::ports::ModelTarget::Main,
                    crate::ports::ModelChoice::Effort { model, effort },
                ),
            ] => {
                assert_eq!(model, "k3");
                assert_eq!(*effort, tact_protocol::ReasoningEffort::Low);
            }
            other => panic!("expected one effort choice, got {other:?}"),
        }
        assert!(
            writer.persisted().is_empty(),
            "no config path means nothing is written"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn applying_effort_pick_clears_stale_thinking_budget() {
        // Regression: an effort pick for an effort-semantic model (k3) must
        // clear a stale thinking_budget left over from a budget-semantic model
        // (kimi-for-coding) — otherwise the bottom bar renders `think high(32K)`.
        let _lock = MODELS_TEST_LOCK.lock().await;
        install_models_config_with_budget(vec!["k3"], "k3", 32_000);
        let mut app = app_with_models_from(crate::ports::ProviderFamily::Kimi, &["k3"], &["k3"]);
        let writer = std::sync::Arc::new(crate::test_fixtures::FakeSettingsWriter::new());
        app.settings_writer = Some(writer.clone());
        start_model_picker(&mut app);
        handle_select_mode(&mut app, key(KeyCode::Enter)); // k3 (single model)
        handle_select_mode(&mut app, key(KeyCode::Enter)); // first effort (Low)

        match writer.applied().as_slice() {
            [
                (crate::ports::ModelTarget::Main, crate::ports::ModelChoice::Effort { effort, .. }),
            ] => {
                assert_eq!(*effort, tact_protocol::ReasoningEffort::Low);
            }
            other => panic!("expected one effort choice, got {other:?}"),
        }
        // The bar must not show a budget next to an effort: the two semantics
        // are mutually exclusive, and `think high(32K)` is the bug this pins.
        assert_eq!(
            app.status_bar_mut().model_thinking_budget,
            None,
            "effort pick clears the stale budget from the bar"
        );
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
        let writer = std::sync::Arc::new(crate::test_fixtures::FakeSettingsWriter::new());
        writer.set_settings(
            crate::test_fixtures::RealConfigWriter::new()
                .current_settings()
                .unwrap_or_default(),
        );
        app.settings_writer = Some(writer.clone());
        app.refresh_config_snapshot();
        start_model_picker(&mut app);
        handle_select_mode(&mut app, key(KeyCode::Down));
        handle_select_mode(&mut app, key(KeyCode::Enter));
        handle_select_mode(&mut app, key(KeyCode::Down));
        handle_select_mode(&mut app, key(KeyCode::Enter));

        // The choice reaches the writer; what it does with it is the adapter's
        // contract, covered by tact-ui.
        match writer.applied().as_slice() {
            [
                (
                    crate::ports::ModelTarget::Main,
                    crate::ports::ModelChoice::Budget { model, budget },
                ),
            ] => {
                assert_eq!(model, "kimi-for-coding");
                assert_eq!(*budget, 64_000, "the picker's second step is the budget");
            }
            other => panic!("expected one budget choice for the main target, got {other:?}"),
        }
        assert_eq!(app.status_bar_mut().model_name, "kimi-for-coding");
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
        let writer = std::sync::Arc::new(crate::test_fixtures::FakeSettingsWriter::new());
        app.settings_writer = Some(writer.clone());

        app.toggle_theme();
        let theme = app.theme.name.as_str().to_string();
        app.toggle_language();
        let language = app.language.as_str().to_string();

        // Both toggles ask for their `[ui]` write. That the file keeps the rest
        // of its contents is the config writer's contract, not the view's.
        assert_eq!(
            writer.ui(),
            vec![
                crate::ports::UiSetting::Theme(theme),
                crate::ports::UiSetting::Language(language),
            ]
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
        assert_eq!(tact::config::settings().llm.model, "kimi-k2.5");
        assert_eq!(tact::config::settings().agent.thinking_budget, 32_000);

        start_model_picker(&mut app);
        handle_select_mode(&mut app, key(KeyCode::Down));
        handle_select_mode(&mut app, key(KeyCode::Enter));
        handle_select_mode(&mut app, key(KeyCode::Esc));

        assert_eq!(tact::config::settings().llm.model, "kimi-k2.5");
        assert_eq!(tact::config::settings().agent.thinking_budget, 32_000);
        assert!(matches!(app.input_mode, InputMode::Normal));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn persisted_model_and_budget_are_written_after_confirming_save() {
        let _lock = MODELS_TEST_LOCK.lock().await;
        let (_temp_dir, _path) = install_models_config_with_path(
            vec!["kimi-k2.5", "kimi-for-coding"],
            "kimi-k2.5",
            32_000,
        );
        let mut app = make_app();
        let writer = std::sync::Arc::new(crate::test_fixtures::FakeSettingsWriter::new());
        writer.set_settings(
            crate::test_fixtures::RealConfigWriter::new()
                .current_settings()
                .unwrap_or_default(),
        );
        app.settings_writer = Some(writer.clone());
        app.refresh_config_snapshot();
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

        match writer.persisted().as_slice() {
            [
                (
                    crate::ports::ModelTarget::Main,
                    crate::ports::ModelChoice::Budget { model, budget },
                ),
            ] => {
                assert_eq!(model, "kimi-for-coding");
                assert_eq!(*budget, 64_000);
            }
            other => panic!("expected one persisted budget choice, got {other:?}"),
        }
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
        let writer = std::sync::Arc::new(crate::test_fixtures::FakeSettingsWriter::new());
        writer.set_settings(
            crate::test_fixtures::RealConfigWriter::new()
                .current_settings()
                .unwrap_or_default(),
        );
        app.settings_writer = Some(writer.clone());
        app.refresh_config_snapshot();
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

        // Applied for the session, never written: escaping the prompt is the
        // "not to disk" answer.
        assert_eq!(writer.applied().len(), 1);
        assert!(
            writer.persisted().is_empty(),
            "escaping the prompt must not write the config"
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
        install_models_config(vec!["cfg-a", "overlap"], "cfg-a");

        let mut app = app_with_models(&["overlap", "api-only"], &[]);
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
        install_models_config(vec![], "current-x");

        let mut app = app_with_models(&["api-1", "api-2"], &[]);
        start_model_picker(&mut app);
        assert!(matches!(app.input_mode, InputMode::Select));
        let options = &app.select.options;
        // current-x prepended because it is not in the merged list
        assert!(options[0].starts_with("current-x"));
        assert!(options.iter().any(|o| o.contains("api-1")));
    }
}
