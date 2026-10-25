//! Building a test `App`.
//!
//! Six modules under `handlers/` each grew their own copy of this: five
//! `unbounded_channel`s, then a twelve-argument `App::new`. They differed only
//! in which receiver the test wanted back, so [`TestApp`] hands back both and
//! the caller splits off the one it asserts on.
//!
//! `render`'s harness builds the same app under the `ink` theme, because its
//! tests assert the colours a theme produces and cannot take the default. That
//! is the only reason there are two constructors.

#![allow(dead_code)]

use std::path::PathBuf;

use tact_protocol::{AgentUpdate, StepResult, StepStatus, ToolPresentationInfo, UserCommand};
use tact_protocol::{PluginEvent, PluginRequest};
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

use crate::widgets::state::App;

/// An in-memory skill catalog for tests.
///
/// The view talks to a port now, so a fixture does not need the extension
/// crate's registry (which would also scan the developer's global skill
/// directory). Entries are whatever the test installs, and `reload` returns the
/// same set unless the test replaces it.
#[derive(Default, Clone)]
pub struct FakeSkillCatalog {
    current: std::sync::Arc<std::sync::Mutex<Vec<crate::SkillEntry>>>,
    off: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    /// What the next `reload` discovers.
    ///
    /// Separate from `current` so a test can say "the filesystem gained a
    /// skill" without the catalog already showing it: a reload that returned
    /// `current` would make every reload assertion vacuous.
    next_reload: std::sync::Arc<std::sync::Mutex<Option<Vec<crate::SkillEntry>>>>,
}

impl FakeSkillCatalog {
    /// A catalog holding `names`, none disabled.
    pub fn with_skills(names: &[&str]) -> Self {
        let catalog = Self::default();
        let entries = names
            .iter()
            .map(|name| crate::SkillEntry {
                name: (*name).to_string(),
                description: format!("{name} description"),
                body: String::new(),
                dir: format!("/skills/{name}"),
                disabled: false,
            })
            .collect();
        *catalog.current.lock().expect("entries lock") = entries;
        catalog
    }

    /// Replace the entries, marking the ones named in `disabled`.
    pub fn set_skills(&self, names: &[&str], disabled: &[&str]) {
        let entries = names
            .iter()
            .map(|name| crate::SkillEntry {
                name: (*name).to_string(),
                description: format!("{name} description"),
                body: String::new(),
                dir: format!("/skills/{name}"),
                disabled: disabled.contains(name),
            })
            .collect();
        *self.current.lock().expect("entries lock") = entries;
        *self.off.lock().expect("disabled lock") =
            disabled.iter().map(|name| (*name).to_string()).collect();
    }

    /// The entries this catalog currently holds.
    ///
    /// Inherent rather than trait-only so a test can read them without bringing
    /// `SkillCatalog` into scope just to call a method.
    pub fn entries(&self) -> Vec<crate::SkillEntry> {
        self.current.lock().expect("entries lock").clone()
    }

    /// Make the next `reload` discover `names` (none disabled).
    pub fn set_next_reload(&self, names: &[&str]) {
        let entries = names
            .iter()
            .map(|name| crate::SkillEntry {
                name: (*name).to_string(),
                description: format!("{name} description"),
                body: String::new(),
                dir: format!("/skills/{name}"),
                disabled: false,
            })
            .collect();
        *self.next_reload.lock().expect("next reload lock") = Some(entries);
    }

    /// Whether `name` is currently marked disabled.
    pub fn is_disabled(&self, name: &str) -> bool {
        self.off
            .lock()
            .expect("disabled lock")
            .iter()
            .any(|disabled| disabled == name)
    }
}

impl crate::ports::SkillCatalog for FakeSkillCatalog {
    fn entries(&self) -> Vec<crate::SkillEntry> {
        Self::entries(self)
    }

    fn reload(&self) -> anyhow::Result<Vec<crate::SkillEntry>> {
        if let Some(entries) = self.next_reload.lock().expect("next reload lock").take() {
            *self.current.lock().expect("entries lock") = entries;
        }
        Ok(self.entries())
    }

    fn disabled(&self) -> Vec<String> {
        let mut names = self.off.lock().expect("disabled lock").clone();
        names.sort();
        names
    }

    fn set_disabled(&self, names: Vec<String>) -> anyhow::Result<Vec<crate::SkillEntry>> {
        *self.off.lock().expect("disabled lock") = names.clone();
        let mut entries = self.current.lock().expect("entries lock");
        for entry in entries.iter_mut() {
            entry.disabled = names.iter().any(|name| name == &entry.name);
        }
        Ok(entries.clone())
    }

    fn description(&self) -> String {
        Self::entries(self)
            .iter()
            .map(|entry| format!("- {}: {}", entry.name, entry.description))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// An interaction port for tests.
///
/// Records what the view asked and answers whatever the test staged, so a test
/// can assert the popup reconciled against a snapshot without standing up the
/// runtime's responder.
#[derive(Default, Clone)]
pub struct FakeInteraction {
    pending: std::sync::Arc<std::sync::Mutex<Vec<crate::ports::PendingPrompt>>>,
    answers: std::sync::Arc<std::sync::Mutex<Vec<tact_protocol::UiResponse>>>,
}

impl FakeInteraction {
    /// A port holding `pending`.
    pub fn with_pending(pending: Vec<crate::ports::PendingPrompt>) -> Self {
        let port = Self::default();
        *port.pending.lock().expect("pending lock") = pending;
        port
    }

    /// One select prompt, as the runtime would report it.
    pub fn select(request_id: u64, prompt: &str, options: &[&str]) -> crate::ports::PendingPrompt {
        crate::ports::PendingPrompt {
            request_id,
            prompt: prompt.to_string(),
            options: options.iter().map(|option| (*option).to_string()).collect(),
            multi: false,
            log_confirm: false,
            from_subagent: false,
        }
    }

    /// The answers the view has given.
    pub fn answers(&self) -> Vec<tact_protocol::UiResponse> {
        self.answers.lock().expect("answers lock").clone()
    }

    /// Remove a pending request, as an answered one would disappear.
    pub fn clear(&self) {
        self.pending.lock().expect("pending lock").clear();
    }
}

impl crate::ports::InteractionPort for FakeInteraction {
    fn pending(&self) -> Vec<crate::ports::PendingPrompt> {
        self.pending.lock().expect("pending lock").clone()
    }

    fn respond(&self, response: tact_protocol::UiResponse) -> bool {
        // Answering removes the entry, like the runtime's responder: a fixture
        // that kept it would let the next reconcile reopen a popup the user just
        // answered.
        let request_id = response.request_id();
        let mut pending = self.pending.lock().expect("pending lock");
        let before = pending.len();
        pending.retain(|entry| entry.request_id != request_id);
        let delivered = pending.len() != before;
        drop(pending);
        self.answers.lock().expect("answers lock").push(response);
        delivered
    }

    fn withdraw(&self, request_id: u64) -> bool {
        let mut pending = self.pending.lock().expect("pending lock");
        let before = pending.len();
        pending.retain(|entry| entry.request_id != request_id);
        pending.len() != before
    }
}

/// A model catalog for tests.
///
/// Answers from what the test staged, so a picker test does not depend on the
/// model library's process-wide cache or on a live endpoint.
#[derive(Default, Clone)]
pub struct FakeModelCatalog {
    models: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    effort_models: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    provider: std::sync::Arc<std::sync::Mutex<String>>,
    family: crate::ports::ProviderFamily,
    subagent_family: Option<crate::ports::ProviderFamily>,
}

impl FakeModelCatalog {
    /// A catalog whose endpoint reports `models`.
    pub fn with_models(models: &[&str]) -> Self {
        let catalog = Self::default();
        *catalog.models.lock().expect("models lock") =
            models.iter().map(|model| (*model).to_string()).collect();
        catalog
    }

    /// Treat `models` as effort-semantic rather than budget-semantic.
    pub fn with_effort_models(self, models: &[&str]) -> Self {
        *self.effort_models.lock().expect("effort lock") =
            models.iter().map(|model| (*model).to_string()).collect();
        self
    }

    /// The label the picker prompt shows.
    pub fn with_provider(self, provider: &str) -> Self {
        *self.provider.lock().expect("provider lock") = provider.to_string();
        self
    }

    /// The provider family the effort tiers come from.
    pub fn with_family(mut self, family: crate::ports::ProviderFamily) -> Self {
        self.family = family;
        self
    }

    /// A family for the subagent target, so its picker opens.
    pub fn with_subagent_family(mut self, family: crate::ports::ProviderFamily) -> Self {
        self.subagent_family = Some(family);
        self
    }
}

impl crate::ports::ModelCatalog for FakeModelCatalog {
    fn candidates(&self, configured: &[String]) -> Vec<String> {
        // Same rule the runtime uses: configured first, then whatever the
        // endpoint reported that is not already there.
        let mut out = configured.to_vec();
        for id in self.models.lock().expect("models lock").iter() {
            if !out.iter().any(|known| known == id) {
                out.push(id.clone());
            }
        }
        out
    }

    fn candidates_for(&self, configured: &[String], _provider_label: &str) -> Vec<String> {
        self.candidates(configured)
    }

    fn uses_effort(&self, model: &str) -> bool {
        self.effort_models
            .lock()
            .expect("effort lock")
            .iter()
            .any(|effort_model| effort_model == model)
    }

    fn provider_label(&self) -> String {
        self.provider.lock().expect("provider lock").clone()
    }

    fn family(&self) -> crate::ports::ProviderFamily {
        self.family
    }

    fn subagent_family(&self) -> Option<crate::ports::ProviderFamily> {
        // Default to the main family so a subagent picker opens in a test that
        // did not think about it — the same shape as a config where the
        // subagent inherits the main provider.
        Some(self.subagent_family.unwrap_or(self.family))
    }
}

/// A settings writer for tests.
///
/// Records every write and applies nothing, so a test asserts *what the view
/// asked for* without a config file. The in-memory `tact::config` state is left
/// alone: a test that needs it should say so, because a writer that silently
/// mutates process state is what made the old assertions racy.
#[derive(Default, Clone)]
pub struct FakeSettingsWriter {
    applied: std::sync::Arc<
        std::sync::Mutex<Vec<(crate::ports::ModelTarget, crate::ports::ModelChoice)>>,
    >,
    persisted: std::sync::Arc<
        std::sync::Mutex<Vec<(crate::ports::ModelTarget, crate::ports::ModelChoice)>>,
    >,
    ui: std::sync::Arc<std::sync::Mutex<Vec<crate::ports::UiSetting>>>,
    disabled_skills: std::sync::Arc<std::sync::Mutex<Vec<Vec<String>>>>,
    settings: std::sync::Arc<std::sync::Mutex<Option<crate::ports::ViewSettings>>>,
}

impl FakeSettingsWriter {
    pub fn new() -> Self {
        Self::default()
    }

    /// The model choices applied in memory, in order.
    pub fn applied(&self) -> Vec<(crate::ports::ModelTarget, crate::ports::ModelChoice)> {
        self.applied.lock().expect("applied lock").clone()
    }

    /// The model choices written to the config file, in order.
    pub fn persisted(&self) -> Vec<(crate::ports::ModelTarget, crate::ports::ModelChoice)> {
        self.persisted.lock().expect("persisted lock").clone()
    }

    /// The `[ui]` writes, in order.
    pub fn ui(&self) -> Vec<crate::ports::UiSetting> {
        self.ui.lock().expect("ui lock").clone()
    }

    /// The skill-disable lists written, in order.
    pub fn disabled_skills(&self) -> Vec<Vec<String>> {
        self.disabled_skills.lock().expect("skills lock").clone()
    }
}

impl FakeSettingsWriter {
    /// Make the next projection return `settings`.
    pub fn set_settings(&self, settings: crate::ports::ViewSettings) {
        *self.settings.lock().expect("settings lock") = Some(settings);
    }

    /// The projection this writer would report.
    pub fn settings(&self) -> Option<crate::ports::ViewSettings> {
        self.settings.lock().expect("settings lock").clone()
    }
}

impl crate::ports::SettingsWriter for FakeSettingsWriter {
    fn apply_model_choice(
        &self,
        target: crate::ports::ModelTarget,
        choice: &crate::ports::ModelChoice,
    ) -> anyhow::Result<()> {
        self.applied
            .lock()
            .expect("applied lock")
            .push((target, choice.clone()));
        Ok(())
    }

    fn persist_model_choice(
        &self,
        target: crate::ports::ModelTarget,
        choice: &crate::ports::ModelChoice,
    ) -> anyhow::Result<()> {
        self.persisted
            .lock()
            .expect("persisted lock")
            .push((target, choice.clone()));
        Ok(())
    }

    fn persist_ui(&self, setting: crate::ports::UiSetting) -> anyhow::Result<()> {
        self.ui.lock().expect("ui lock").push(setting);
        Ok(())
    }

    fn persist_disabled_skills(&self, names: &[String]) -> anyhow::Result<()> {
        self.disabled_skills
            .lock()
            .expect("skills lock")
            .push(names.to_vec());
        Ok(())
    }

    fn current_settings(&self) -> Option<crate::ports::ViewSettings> {
        self.settings.lock().expect("settings lock").clone()
    }
}

/// A settings writer that reaches the real config helpers.
///
/// For the rare test whose subject *is* the validation inside those helpers (a
/// subagent model the pinned context window cannot fit). It does write to the
/// process-global config, so a test using it must be the only one asserting on
/// that state.
#[derive(Default, Clone, Copy)]
pub struct RealConfigWriter;

impl RealConfigWriter {
    pub fn new() -> Self {
        Self
    }

    /// The current configuration projected for the view.
    pub fn current_settings(&self) -> Option<crate::ports::ViewSettings> {
        <Self as crate::ports::SettingsWriter>::current_settings(self)
    }
}

impl crate::ports::SettingsWriter for RealConfigWriter {
    fn apply_model_choice(
        &self,
        target: crate::ports::ModelTarget,
        choice: &crate::ports::ModelChoice,
    ) -> anyhow::Result<()> {
        use crate::ports::{ModelChoice, ModelTarget};
        match (target, choice) {
            (ModelTarget::Main, ModelChoice::Effort { model, effort }) => {
                tact::config::update_llm_model_and_reasoning_effort(model.clone(), Some(*effort));
                Ok(())
            }
            (ModelTarget::Main, ModelChoice::Budget { model, budget }) => {
                tact::config::update_llm_model_and_thinking_budget(model.clone(), *budget);
                Ok(())
            }
            (ModelTarget::Subagent, ModelChoice::Budget { model, budget }) => {
                tact::config::update_subagent_model(model.clone(), *budget)
            }
            (ModelTarget::Subagent, ModelChoice::Effort { model, effort }) => {
                let budget = tact::config::try_settings()
                    .and_then(|config| config.agent.subagent.map(|sa| sa.thinking_budget))
                    .unwrap_or_default();
                tact::config::update_subagent_model(model.clone(), budget)?;
                tact::config::update_subagent_reasoning_effort(Some(*effort));
                Ok(())
            }
        }
    }

    fn persist_model_choice(
        &self,
        _target: crate::ports::ModelTarget,
        _choice: &crate::ports::ModelChoice,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    fn persist_ui(&self, _setting: crate::ports::UiSetting) -> anyhow::Result<()> {
        Ok(())
    }

    fn persist_disabled_skills(&self, _names: &[String]) -> anyhow::Result<()> {
        Ok(())
    }

    fn current_settings(&self) -> Option<crate::ports::ViewSettings> {
        // Built from the real configuration, so a test that asserts the derived
        // values (a raised `max_tokens`) sees what production would.
        let config = tact::config::try_settings()?;
        let mut model_profiles = std::collections::HashMap::new();
        for (model, profile) in &config.llm.model_profiles {
            model_profiles.insert(
                model.clone(),
                crate::ports::ModelProfile {
                    reasoning_efforts: profile.reasoning_efforts.clone(),
                    thinking_budgets: profile.thinking_budgets.clone(),
                },
            );
        }
        Some(crate::ports::ViewSettings {
            model: config.llm.model.clone(),
            provider_label: config.llm.provider.as_str().to_string(),
            models: config.llm.models.clone(),
            model_profiles,
            reasoning_effort: config.agent.reasoning_effort,
            thinking_budget: config.agent.thinking_budget,
            max_tokens: config.agent.max_tokens,
            has_config_path: config.config_path.is_some(),
            subagent: config
                .agent
                .subagent
                .as_ref()
                .map(|sa| crate::ports::SubagentSettings {
                    model: sa.provider.model.clone(),
                    models: sa.models.clone(),
                    reasoning_effort: sa.reasoning_effort,
                    thinking_budget: sa.thinking_budget,
                    provider_label: sa.provider.provider.as_str().to_string(),
                    family: crate::ports::ProviderFamily::Kimi,
                }),
        })
    }
}

/// A voice worker for tests.
///
/// Records the commands the view sends and answers the two state questions, so a
/// test asserts what the view asked for without a spawned task.
#[derive(Default, Clone)]
pub struct FakeVoice {
    commands: std::sync::Arc<std::sync::Mutex<Vec<tact_protocol::VoiceCommand>>>,
    missing_api_key: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl FakeVoice {
    pub fn new() -> Self {
        Self::default()
    }

    /// Mark the configured provider as lacking an API key.
    pub fn without_api_key(self) -> Self {
        self.missing_api_key
            .store(true, std::sync::atomic::Ordering::Relaxed);
        self
    }

    /// The commands sent so far.
    pub fn commands(&self) -> Vec<tact_protocol::VoiceCommand> {
        self.commands.lock().expect("commands lock").clone()
    }
}

impl crate::ports::VoicePort for FakeVoice {
    fn send(&self, command: tact_protocol::VoiceCommand) -> bool {
        self.commands.lock().expect("commands lock").push(command);
        true
    }

    fn is_enabled(&self) -> bool {
        true
    }

    fn missing_api_key(&self) -> bool {
        self.missing_api_key
            .load(std::sync::atomic::Ordering::Relaxed)
    }
}

/// An app under test, still holding the channels it talks through.
pub struct TestApp {
    /// The app itself. Public so a render test can take it and drop the rest.
    pub app: App,
    /// What the app asked the agent to do (`/mcp`, `/hooks`, `/skill`, …).
    user_cmds: UnboundedReceiver<UserCommand>,
    /// What the app asked the plugin host to do (`/plugin reload`, …).
    plugin_requests: UnboundedReceiver<PluginRequest>,
}

impl Default for TestApp {
    fn default() -> Self {
        Self::new()
    }
}

impl TestApp {
    /// The `handlers` fixture: session `test-session`, theme `retro`.
    pub fn new() -> Self {
        Self::with_identity("test-session", "retro")
    }

    /// The same app under a named session and theme.
    pub fn with_identity(session_id: &str, theme: &str) -> Self {
        // Both senders are dropped here: the receivers are what the app holds,
        // and a fixture that kept an agent channel alive would let a test hang
        // waiting on a stream nothing can close.
        let (_agent_tx, agent_rx) = unbounded_channel::<AgentUpdate>();
        let (user_cmd_tx, user_cmds) = unbounded_channel::<UserCommand>();
        let (plugin_tx, plugin_requests) = unbounded_channel::<PluginRequest>();
        let (_plugin_event_tx, plugin_rx) = unbounded_channel::<PluginEvent>();
        let (history_tx, _history_rx) = unbounded_channel::<(String, String)>();
        let mut app = App::new(
            agent_rx,
            None,
            plugin_rx,
            plugin_tx,
            user_cmd_tx,
            PathBuf::from("."),
            Vec::new(),
            session_id.to_string(),
            history_tx,
            theme.to_string(),
            String::new(),
            Vec::new(),
        );
        // The host seeds these from its resolved configuration and provider; a
        // fixture that skipped them would exercise a different `/model` path
        // than production and hide a regression in the handoff. The model
        // catalog starts empty (no discovered ids) so a test that cares states
        // what the endpoint reports.
        // The fixture's writer mirrors the installed configuration, so a test
        // that installs one after building the app can re-project through it —
        // the same route production takes.
        let writer = std::sync::Arc::new(FakeSettingsWriter::new());
        // Default to the installed configuration so a picker test sees the same
        // values production would; a test that cares calls `set_settings`.
        writer.set_settings(
            RealConfigWriter::new()
                .current_settings()
                .unwrap_or_default(),
        );
        app.set_config_snapshot(writer.settings());
        app.models = Some(std::sync::Arc::new(FakeModelCatalog::default()));
        app.settings_writer = Some(writer);
        Self {
            app,
            user_cmds,
            plugin_requests,
        }
    }

    /// Split into the app and the receiver of the commands it emitted.
    pub fn into_commands(self) -> (App, UnboundedReceiver<UserCommand>) {
        (self.app, self.user_cmds)
    }

    /// Split into the app and the receiver of the plugin requests it emitted.
    pub fn into_plugin_requests(self) -> (App, UnboundedReceiver<PluginRequest>) {
        (self.app, self.plugin_requests)
    }
}

/// A tool call, as the host reports it: the `StepStarted` it emits when the
/// call begins and the `StepFinished` when it ends.
///
/// The literals this replaces ran to eight and sixteen lines and appeared 78
/// times across the render and handler tests. The defaults are the ones those
/// literals overwhelmingly used, so a test that does not care about a field
/// does not have to say anything about it — and a test that *does* care sets it
/// explicitly, which is the point: the field it cares about is the one visible
/// in the call.
pub struct StepCall {
    idx: usize,
    tool_id: String,
    tool: String,
    arg_summary: String,
    arg_full: Option<String>,
    status: StepStatus,
    message: String,
    detail: Option<String>,
    duration_us: u64,
    permission_label: Option<String>,
    presentation: Option<ToolPresentationInfo>,
}

impl StepCall {
    /// A call to `tool` identified as `tool_id`, at position `idx` in the turn.
    ///
    /// `arg` is both the summary and the full argument, which is what the host
    /// sends for every tool whose input is one value.
    pub fn new(
        idx: usize,
        tool_id: impl Into<String>,
        tool: impl Into<String>,
        arg: impl Into<String>,
    ) -> Self {
        let arg = arg.into();
        Self {
            idx,
            tool_id: tool_id.into(),
            tool: tool.into(),
            arg_summary: arg.clone(),
            arg_full: Some(arg),
            status: StepStatus::Success,
            message: "ok".to_string(),
            detail: None,
            duration_us: 1,
            permission_label: None,
            presentation: None,
        }
    }

    /// The full argument, when it differs from the summary.
    pub fn arg_full(mut self, arg_full: impl Into<String>) -> Self {
        self.arg_full = Some(arg_full.into());
        self
    }

    /// No full argument — what the host sends when the summary is all there is.
    pub fn no_arg_full(mut self) -> Self {
        self.arg_full = None;
        self
    }

    pub fn status(mut self, status: StepStatus) -> Self {
        self.status = status;
        self
    }

    pub fn message(mut self, message: impl Into<String>) -> Self {
        self.message = message.into();
        self
    }

    /// The output the card renders, or its absence when a call produced none.
    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub fn duration_us(mut self, duration_us: u64) -> Self {
        self.duration_us = duration_us;
        self
    }

    pub fn permission_label(mut self, label: impl Into<String>) -> Self {
        self.permission_label = Some(label.into());
        self
    }

    /// The tool's presentation, when the generic one for `tool` is not what the
    /// test is about (`task_presentation()`, a subagent card, …).
    pub fn presentation(mut self, presentation: ToolPresentationInfo) -> Self {
        self.presentation = Some(presentation);
        self
    }

    pub fn started(&self) -> AgentUpdate {
        AgentUpdate::StepStarted {
            idx: self.idx,
            tool_id: self.tool_id.clone(),
            tool_name: self.tool.clone(),
            arg_summary: self.arg_summary.clone(),
            arg_full: self.arg_full.clone().unwrap_or_default(),
            presentation: self.presentation_info(),
        }
    }

    pub fn finished(&self) -> AgentUpdate {
        AgentUpdate::StepFinished {
            idx: self.idx,
            tool_id: self.tool_id.clone(),
            result: StepResult {
                tool: self.tool.clone(),
                arg_summary: self.arg_summary.clone(),
                arg_full: self.arg_full.clone(),
                status: self.status,
                message: self.message.clone(),
                detail: self.detail.clone(),
                duration_us: Some(self.duration_us),
                permission_label: self.permission_label.clone(),
                presentation: self.presentation_info(),
            },
        }
    }

    fn presentation_info(&self) -> ToolPresentationInfo {
        self.presentation
            .clone()
            .unwrap_or_else(|| ToolPresentationInfo::generic(&self.tool))
    }
}
