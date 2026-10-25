//! The view's ports, implemented over extension services.
//!
//! `tui` declares what it needs (see `tui::ports`); this module is the one place
//! that knows how those questions are answered by the runtime. Keeping the
//! adapter here — not in `tui` — is what lets the view compile without the
//! extension crate: the view names `SkillCatalog`, and only this file mentions
//! `tact::skill`.

use std::sync::Arc;

use tui::ports::SkillCatalog;

/// The skill catalog the runtime owns, seen through the view's port.
///
/// Holds the same `SharedSkillRegistry` the agent's `ToolContext` holds, so a
/// reload or a disable reaches both sides at once — the view must never show a
/// catalog the agent does not have.
pub struct RuntimeSkillCatalog {
    registry: tact::skill::SharedSkillRegistry,
    work_dir: std::path::PathBuf,
}

impl RuntimeSkillCatalog {
    /// Adapt the shared registry for one workspace.
    ///
    /// `work_dir` is kept because a reload re-derives the search roots from it:
    /// tests set the workspace late, and a reload must see the current one.
    pub fn new(registry: tact::skill::SharedSkillRegistry, work_dir: std::path::PathBuf) -> Self {
        Self { registry, work_dir }
    }
}

fn project(reg: &tact::skill::SkillRegistry) -> Vec<tui::SkillEntry> {
    reg.skills()
        .values()
        .map(|doc| tui::SkillEntry {
            name: doc.manifest.name.clone(),
            description: doc.manifest.description.clone(),
            body: doc.body.clone(),
            dir: doc.dir.display().to_string(),
            disabled: reg.is_disabled(&doc.manifest.name),
        })
        .collect()
}

impl SkillCatalog for RuntimeSkillCatalog {
    fn entries(&self) -> Vec<tui::SkillEntry> {
        project(&tact::skill::lock_skills(&self.registry))
    }

    fn reload(&self) -> anyhow::Result<Vec<tui::SkillEntry>> {
        let mut reg = tact::skill::lock_skills(&self.registry);
        // Keep the search roots in sync with the current workspace: a test (or a
        // session that changed directory) must reload from where it is now.
        *reg = tact::skill::get_skill_registry(&self.work_dir)?;
        Ok(project(&reg))
    }

    fn disabled(&self) -> Vec<String> {
        let reg = tact::skill::lock_skills(&self.registry);
        let mut names: Vec<String> = reg.disabled().iter().cloned().collect();
        names.sort();
        names
    }

    fn set_disabled(&self, names: Vec<String>) -> anyhow::Result<Vec<tui::SkillEntry>> {
        let mut reg = tact::skill::lock_skills(&self.registry);
        reg.set_disabled(names);
        Ok(project(&reg))
    }

    fn description(&self) -> String {
        tact::skill::lock_skills(&self.registry).describe_available()
    }
}

/// Wrap a registry for the view.
pub fn skill_catalog(
    registry: tact::skill::SharedSkillRegistry,
    work_dir: std::path::PathBuf,
) -> Arc<dyn SkillCatalog> {
    Arc::new(RuntimeSkillCatalog::new(registry, work_dir))
}

/// The runtime's interaction broker, seen through the view's port.
///
/// Holds the same `UiResponder` the tool contexts use, so an answer given in the
/// popup reaches the waiting tool — a separate responder would answer a waiter
/// nobody has.
pub struct RuntimeInteractionPort {
    responder: tact::ui_responder::UiResponder,
}

impl RuntimeInteractionPort {
    pub fn new(responder: tact::ui_responder::UiResponder) -> Self {
        Self { responder }
    }
}

impl tui::ports::InteractionPort for RuntimeInteractionPort {
    fn pending(&self) -> Vec<tui::ports::PendingPrompt> {
        self.responder
            .snapshot()
            .into_iter()
            .map(|pending| tui::ports::PendingPrompt {
                request_id: pending.request_id,
                prompt: pending.prompt,
                options: pending.options,
                multi: matches!(
                    pending.kind,
                    tact::ui_responder::PendingUiRequestKind::MultiSelect
                ),
                log_confirm: pending.log_confirm,
                from_subagent: pending.from_subagent,
            })
            .collect()
    }

    fn respond(&self, response: tact_protocol::UiResponse) -> bool {
        self.responder.respond(response)
    }

    fn withdraw(&self, request_id: u64) -> bool {
        self.responder.withdraw(request_id)
    }
}

/// Wrap a responder for the view.
pub fn interaction_port(
    responder: tact::ui_responder::UiResponder,
) -> std::sync::Arc<dyn tui::ports::InteractionPort> {
    std::sync::Arc::new(RuntimeInteractionPort::new(responder))
}

/// The runtime's model queries, seen through the view's port.
///
/// Wraps the two failures a discovery call can have (no endpoint for this
/// provider, no network) into an empty id list: a picker that cannot reach the
/// endpoint still offers the configured models, which is what a user needs to
/// switch between what they already have.
pub struct RuntimeModelCatalog;

impl RuntimeModelCatalog {
    pub fn new() -> Self {
        Self
    }
}

impl Default for RuntimeModelCatalog {
    fn default() -> Self {
        Self::new()
    }
}

/// The runtime's command catalog, seen through the view's narrow port.
///
/// It holds the built catalog rather than rebuilding one per keystroke: the
/// palette asks on every input change, and constructing a catalog allocates a
/// map of every contributed command.
pub struct RuntimeCommandDirectory {
    catalog: tact::command::CommandCatalog,
}

impl RuntimeCommandDirectory {
    pub fn new(catalog: tact::command::CommandCatalog) -> Self {
        Self { catalog }
    }

    /// Install this directory into a view slot.
    ///
    /// The view comes up before the runtime, so the host fills the slot once the
    /// agent exists; this is the one place that knows how.
    pub fn install(self, slot: &tui::ports::SharedCommandDirectory) {
        *slot.lock().unwrap_or_else(|error| error.into_inner()) = Some(std::sync::Arc::new(self));
    }
}

/// Build the runtime's catalog and install it into a view slot.
///
/// Built from the same wiring the driver uses, so the palette and `/stats`
/// cannot disagree about which commands exist or what they are called.
pub fn install_command_directory(
    agent: &tact::agent::Agent,
    slot: &tui::ports::SharedCommandDirectory,
) {
    let session = agent.runtime.session_id.clone();
    let catalog = tact::command::builtin_catalog(
        agent.runtime.stats.clone(),
        Some(tact::command::BackgroundWiring {
            manager: std::sync::Arc::new(agent.tool_context.background_manager.clone()),
            session: std::sync::Arc::new(move || session.clone()),
        }),
        Some(tact::command::GoalWiring {
            goal_manager: agent.tool_context.goal_manager.clone(),
            task_manager: agent.tool_context.task_manager.clone(),
            limits: crate::driver::agent_goal_limits(agent),
        }),
        // The palette offers `/compact` only when the runtime can run it.
        agent
            .chat_service()
            .map(|chat| chat as std::sync::Arc<dyn tact::command::ChatService>),
        Some(tact::command::HooksWiring {
            work_dir: agent.tool_context.work_dir.clone(),
            render: std::sync::Arc::new(crate::hooks_cli::render_hooks_listing),
        }),
        // The palette lists `/mcp`. The listing itself runs in the command loop,
        // which holds the live router; here the catalog only needs to exist so
        // the command is offered.
        Some(tact::command::McpWiring {
            catalog: std::sync::Arc::new(crate::driver::EmptyMcpCatalog),
            render: std::sync::Arc::new(crate::mcp_cli::render_live_listing),
        }),
        Some(agent.runtime.permission_manager.clone()),
    );
    RuntimeCommandDirectory::new(catalog).install(slot);
}

impl tui::ports::CommandDirectory for RuntimeCommandDirectory {
    fn commands(&self) -> Vec<tui::ports::CommandEntry> {
        self.catalog
            .descriptors()
            .into_iter()
            .map(|meta| tui::ports::CommandEntry {
                name: meta.name,
                summary: meta.summary,
                needs_args: meta.needs_args,
            })
            .collect()
    }

    fn resolve(&self, name: &str) -> Option<tui::ports::CommandEntry> {
        let command = self.catalog.get(name)?;
        let meta = command.meta();
        Some(tui::ports::CommandEntry {
            name: meta.name.clone(),
            summary: meta.summary.clone(),
            needs_args: meta.needs_args,
        })
    }
}

/// Ask the endpoint for its model ids when the runtime allows it.
///
/// `block_in_place` because callers hold the UI loop; without a runtime (sync
/// unit tests) the config-only list is returned, which is the documented
/// behaviour of the picker in that situation.
fn api_model_ids(provider: Option<&tact_llm::ProviderInfo>) -> Vec<String> {
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        return Vec::new();
    };
    tokio::task::block_in_place(|| match provider {
        Some(provider) => handle.block_on(tact_llm::ensure_api_model_ids_for_provider(provider)),
        None => handle.block_on(tact_llm::ensure_api_model_ids()),
    })
}

impl tui::ports::ModelCatalog for RuntimeModelCatalog {
    fn candidates(&self, configured: &[String]) -> Vec<String> {
        if !tact_llm::is_models_query_supported() {
            return tact_llm::merge_model_candidates(configured, &[]);
        }
        let api = api_model_ids(None);
        tact_llm::merge_model_candidates(configured, &api)
    }

    fn candidates_for(&self, configured: &[String], provider_label: &str) -> Vec<String> {
        // The subagent target is resolved from its own provider entry, so the
        // label the caller already has is enough to pick the endpoint.
        let provider = tact_llm::try_get_provider().filter(|info| {
            info.provider.as_str() == provider_label || info.base_url.contains(provider_label)
        });
        let api = api_model_ids(provider.as_ref());
        tact_llm::merge_model_candidates(configured, &api)
    }

    fn uses_effort(&self, model: &str) -> bool {
        match tact_llm::try_get_provider() {
            Some(info) => tact_llm::model_uses_effort(model, &info),
            // No provider installed (a test): the budget semantics are the
            // historical default, so the picker still opens.
            None => false,
        }
    }

    fn provider_label(&self) -> String {
        tact_llm::try_get_provider()
            .map(|info| info.provider.as_str().to_string())
            .unwrap_or_default()
    }

    fn family(&self) -> tui::ports::ProviderFamily {
        tact_llm::try_get_provider()
            .map(|info| family_of(&info.provider))
            .unwrap_or_default()
    }

    fn subagent_family(&self) -> Option<tui::ports::ProviderFamily> {
        // Read from the resolved configuration, not the process provider: a
        // subagent may target a different endpoint than the main run, and the
        // picker's tiers follow the target.
        let config = tact::config::try_settings()?;
        let subagent = config.agent.subagent?;
        Some(family_of(&subagent.provider.provider))
    }
}

/// Map the model library's provider kind onto the view's family.
///
/// One `match` in one place: the view never sees `ProviderKind`, so adding a
/// provider kind fails to compile here rather than silently rendering as the
/// openai tier list.
fn family_of(kind: &tact_llm::ProviderKind) -> tui::ports::ProviderFamily {
    use tui::ports::ProviderFamily;
    match kind {
        tact_llm::ProviderKind::OpenAi => ProviderFamily::OpenAi,
        tact_llm::ProviderKind::DeepSeek => ProviderFamily::DeepSeek,
        tact_llm::ProviderKind::Kimi => ProviderFamily::Kimi,
        tact_llm::ProviderKind::Anthropic => ProviderFamily::Anthropic,
        tact_llm::ProviderKind::Custom(_) => ProviderFamily::Custom,
    }
}

/// The model catalog for the view.
pub fn model_catalog() -> std::sync::Arc<dyn tui::ports::ModelCatalog> {
    std::sync::Arc::new(RuntimeModelCatalog::new())
}

/// The runtime's session store, seen through the view's history port.
///
/// Reads the persisted transcript and projects each row onto what the log
/// renders: the role, the text blocks, and the two "this is derived context"
/// flags. The projection lives here because only the reader can see the stored
/// kind; the view still checks the tag text as well, for rows written before
/// the kind was recorded.
pub struct RuntimeSessionHistory {
    store: tact::store::DynSessionStore,
}

impl RuntimeSessionHistory {
    pub fn new(store: tact::store::DynSessionStore) -> Self {
        Self { store }
    }
}

impl tui::ports::SessionHistory for RuntimeSessionHistory {
    fn turns(&self, session_id: &str) -> anyhow::Result<Vec<tui::ports::HistoryTurn>> {
        let messages = tokio::runtime::Handle::try_current()
            .map(|handle| {
                tokio::task::block_in_place(|| handle.block_on(self.store.load_session(session_id)))
            })
            .unwrap_or_else(|_| {
                // No runtime: nothing can read the database here, and pretending
                // an empty transcript is better than panicking in a view.
                Ok(Vec::new())
            })?;
        Ok(messages
            .into_iter()
            .map(|message| tui::ports::HistoryTurn {
                role: match message.role {
                    tact_llm::Role::User => tui::ports::HistoryRole::User,
                    tact_llm::Role::Assistant => tui::ports::HistoryRole::Assistant,
                },
                text: message_text(&message),
                is_hook_context: message.is_hook_context(),
                is_goal_context: message.is_goal_context(),
            })
            .collect())
    }

    fn latest_request_body(&self, session_id: &str) -> anyhow::Result<Option<String>> {
        let handle = tokio::runtime::Handle::try_current()?;
        let body = tokio::task::block_in_place(|| {
            handle.block_on(self.store.load_latest_request_body(session_id))
        })?;
        // The stored body is bytes; the popup renders text, so a body that is
        // not UTF-8 is reported as absent rather than mojibake.
        Ok(body.and_then(|bytes| String::from_utf8(bytes).ok()))
    }
}

/// The text a turn carries, joining text blocks with newlines.
///
/// Tool calls and results are not rendered by the log, so they are dropped here
/// rather than handed to the view to ignore.
fn message_text(message: &tact_llm::Message) -> String {
    match &message.content {
        tact_llm::MessageContent::Text { content } => content.clone(),
        tact_llm::MessageContent::Blocks { content } => content
            .iter()
            .filter_map(|block| match block {
                tact_llm::ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

/// Wrap a store for the view.
pub fn session_history(
    store: tact::store::DynSessionStore,
) -> std::sync::Arc<dyn tui::ports::SessionHistory> {
    std::sync::Arc::new(RuntimeSessionHistory::new(store))
}

/// The runtime's config writes, seen through the view's port.
///
/// Every method funnels into the `tact::config` helper that owns the file format
/// and the in-memory update, so the view states *what* changed and this adapter
/// keeps knowing *how* it is stored.
pub struct RuntimeSettingsWriter;

impl RuntimeSettingsWriter {
    pub fn new() -> Self {
        Self
    }
}

impl Default for RuntimeSettingsWriter {
    fn default() -> Self {
        Self::new()
    }
}

impl tui::ports::SettingsWriter for RuntimeSettingsWriter {
    fn apply_model_choice(
        &self,
        target: tui::ports::ModelTarget,
        choice: &tui::ports::ModelChoice,
    ) -> anyhow::Result<()> {
        use tui::ports::{ModelChoice, ModelTarget};
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
                // The subagent update carries a budget, so the effort is applied
                // separately: applying both at once would fight over the same
                // wire field.
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
        target: tui::ports::ModelTarget,
        choice: &tui::ports::ModelChoice,
    ) -> anyhow::Result<()> {
        use tui::ports::{ModelChoice, ModelTarget};
        match (target, choice) {
            (ModelTarget::Main, ModelChoice::Effort { model, effort }) => {
                tact::config::persist_active_provider_model_and_reasoning_effort(
                    model,
                    effort.as_str(),
                )
            }
            (ModelTarget::Main, ModelChoice::Budget { model, budget }) => {
                tact::config::persist_active_provider_model_and_thinking_budget(model, *budget)
            }
            (ModelTarget::Subagent, ModelChoice::Effort { model, effort }) => {
                tact::config::persist_subagent_model_and_reasoning_effort(model, effort.as_str())
            }
            (ModelTarget::Subagent, ModelChoice::Budget { model, budget }) => {
                tact::config::persist_subagent_model(model, *budget)
            }
        }
    }

    fn persist_ui(&self, setting: tui::ports::UiSetting) -> anyhow::Result<()> {
        use tui::ports::UiSetting;
        match setting {
            UiSetting::Theme(theme) => tact::config::persist_theme(&theme),
            UiSetting::Language(language) => tact::config::persist_language(&language),
            UiSetting::HookOutput(enabled) => tact::config::persist_hook_output(enabled),
        }
    }

    fn persist_disabled_skills(&self, names: &[String]) -> anyhow::Result<()> {
        tact::config::persist_skill_disable(names)
    }

    fn current_settings(&self) -> Option<tui::ports::ViewSettings> {
        tact::config::try_settings().map(|config| project_settings(&config))
    }
}

/// Project the runtime configuration onto what the view reads.
///
/// One function, so a field the view stops using disappears here instead of
/// being carried forever, and the eight-value list stays visible in one place.
pub fn project_settings(config: &tact::config::ResolvedConfig) -> tui::ports::ViewSettings {
    use std::collections::HashMap;
    let mut model_profiles = HashMap::new();
    for (model, profile) in &config.llm.model_profiles {
        model_profiles.insert(
            model.clone(),
            tui::ports::ModelProfile {
                reasoning_efforts: profile.reasoning_efforts.clone(),
                thinking_budgets: profile.thinking_budgets.clone(),
            },
        );
    }
    tui::ports::ViewSettings {
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
            .map(|sa| tui::ports::SubagentSettings {
                model: sa.provider.model.clone(),
                models: sa.models.clone(),
                reasoning_effort: sa.reasoning_effort,
                thinking_budget: sa.thinking_budget,
                provider_label: sa.provider.provider.as_str().to_string(),
                family: family_of(&sa.provider.provider),
            }),
    }
}

/// The settings writer for the view.
pub fn settings_writer() -> std::sync::Arc<dyn tui::ports::SettingsWriter> {
    std::sync::Arc::new(RuntimeSettingsWriter::new())
}

/// Start the voice worker and adapt it for the view.
///
/// `None` when voice is disabled, which is the common case: nothing is spawned
/// and the view draws no button.
pub fn voice_worker(settings: tact::config::VoiceSettings) -> Option<tui::VoiceWorker> {
    if !settings.enabled {
        return None;
    }
    let service: std::sync::Arc<dyn tact::voice::VoiceService> =
        std::sync::Arc::new(tact::voice::RuntimeVoiceService::spawn(settings));
    Some(voice_worker_from(service))
}

/// Adapt an already-spawned service for the view.
///
/// The service owns the worker; the view gets a port over it plus the event stream,
/// which it takes here because it is the stream's single consumer. The plugin is
/// what lets both halves exist without the view owning the worker.
pub fn voice_worker_from(
    service: std::sync::Arc<dyn tact::voice::VoiceService>,
) -> tui::VoiceWorker {
    let events = service
        .take_events()
        .expect("the event stream is taken once, and this is the consumer");
    tui::VoiceWorker {
        port: std::sync::Arc::new(ServiceVoicePort {
            service: service.clone(),
        }),
        events,
        missing_api_key: service.missing_api_key(),
    }
}

/// The view's voice port, over a domain service.
///
/// Replaces the old port that held the worker handle: the domain owns the worker
/// now, and this only forwards.
struct ServiceVoicePort {
    service: std::sync::Arc<dyn tact::voice::VoiceService>,
}

impl tui::ports::VoicePort for ServiceVoicePort {
    fn send(&self, command: tact_protocol::VoiceCommand) -> bool {
        let mapped = match command {
            tact_protocol::VoiceCommand::Start => tact::voice::VoiceCommand::Start,
            tact_protocol::VoiceCommand::Stop => tact::voice::VoiceCommand::Stop,
            tact_protocol::VoiceCommand::Cancel => tact::voice::VoiceCommand::Cancel,
            tact_protocol::VoiceCommand::Shutdown => tact::voice::VoiceCommand::Shutdown,
        };
        self.service.send(mapped)
    }

    fn is_enabled(&self) -> bool {
        self.service.is_enabled()
    }

    fn missing_api_key(&self) -> bool {
        self.service.missing_api_key()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The directory lists what the catalog holds, in the catalog's order.
    ///
    /// The palette renders this verbatim, so a name or summary that went
    /// missing here is a command a user cannot find.
    #[test]
    fn the_directory_lists_the_catalogs_commands() {
        let catalog = tact::command::builtin_catalog(
            Arc::new(std::sync::RwLock::new(tact::stats::SessionStats::default())),
            None,
            None,
            None,
            None,
            None,
            None,
        );
        let expected: Vec<String> = catalog
            .descriptors()
            .into_iter()
            .map(|meta| meta.name)
            .collect();
        assert!(!expected.is_empty(), "the built-ins contribute something");

        let directory = RuntimeCommandDirectory::new(catalog);
        let listed: Vec<String> = tui::ports::CommandDirectory::commands(&directory)
            .into_iter()
            .map(|entry| entry.name)
            .collect();
        assert_eq!(listed, expected);
    }

    /// Resolving by name answers with the command the catalog routes to.
    ///
    /// An unknown name is `None`, not an error: the view still owns local
    /// commands and falls back to them.
    #[test]
    fn resolve_answers_with_the_routed_command() {
        let catalog = tact::command::builtin_catalog(
            Arc::new(std::sync::RwLock::new(tact::stats::SessionStats::default())),
            None,
            None,
            None,
            None,
            None,
            None,
        );
        let directory = RuntimeCommandDirectory::new(catalog);
        let stats = tui::ports::CommandDirectory::resolve(&directory, "stats")
            .expect("stats is a built-in");
        assert_eq!(stats.name, "stats");
        assert!(
            tui::ports::CommandDirectory::resolve(&directory, "no-such-command").is_none(),
            "an unknown name is not a catalog command"
        );
    }

    /// Installing fills the slot the view reads.
    #[test]
    fn installing_fills_the_view_slot() {
        let slot = tui::ports::shared_command_directory();
        assert!(slot.lock().unwrap().is_none(), "starts empty");

        let catalog = tact::command::builtin_catalog(
            Arc::new(std::sync::RwLock::new(tact::stats::SessionStats::default())),
            None,
            None,
            None,
            None,
            None,
            None,
        );
        RuntimeCommandDirectory::new(catalog).install(&slot);

        let installed = slot.lock().unwrap().clone().expect("the slot is filled");
        assert!(!installed.commands().is_empty());
    }

    fn catalog_with(skills: &[(&str, bool)]) -> (tempfile::TempDir, Arc<dyn SkillCatalog>) {
        let dir = tempfile::tempdir().unwrap();
        for (name, _disabled) in skills {
            let skill_dir = dir.path().join(".tact/skills").join(name);
            std::fs::create_dir_all(&skill_dir).unwrap();
            std::fs::write(
                skill_dir.join("SKILL.md"),
                format!("---\nname: {name}\ndescription: {name} desc\n---\n{name} body"),
            )
            .unwrap();
        }
        let registry = tact::skill::shared_skill_registry(dir.path()).unwrap();
        if skills.iter().any(|(_, disabled)| *disabled) {
            let disabled: Vec<String> = skills
                .iter()
                .filter(|(_, disabled)| *disabled)
                .map(|(name, _)| (*name).to_string())
                .collect();
            tact::skill::lock_skills(&registry).set_disabled(disabled);
        }
        let catalog = skill_catalog(registry, dir.path().to_path_buf());
        (dir, catalog)
    }

    /// The port reports what the registry holds, disabled rows included.
    ///
    /// Asserted by presence, not by count: the registry also scans the user's
    /// global skill directory, so a developer machine has far more entries than
    /// this fixture creates. Pinning the count would make the test pass only in
    /// a clean environment.
    #[test]
    fn entries_mirror_the_registry() {
        let (_dir, catalog) = catalog_with(&[("alpha", false), ("beta", true)]);
        let entries = catalog.entries();
        assert!(
            entries
                .iter()
                .any(|entry| entry.name == "alpha" && !entry.disabled),
            "the enabled skill is listed: {entries:?}"
        );
        assert!(
            entries
                .iter()
                .any(|entry| entry.name == "beta" && entry.disabled),
            "a disabled skill stays listed and is marked: {entries:?}"
        );
        assert_eq!(
            catalog.disabled(),
            vec!["beta".to_string()],
            "only the fixture's skill is disabled"
        );
    }

    /// Disabling replaces the whole set rather than appending.
    ///
    /// The list is what gets written to config, so a partial write would
    /// silently re-enable everything it left out.
    #[test]
    fn set_disabled_replaces_the_whole_set() {
        let (_dir, catalog) = catalog_with(&[("alpha", false), ("beta", false)]);
        catalog
            .set_disabled(vec!["alpha".to_string()])
            .expect("disable alpha");
        assert_eq!(catalog.disabled(), vec!["alpha".to_string()]);

        catalog
            .set_disabled(vec!["beta".to_string()])
            .expect("disable beta instead");
        assert_eq!(
            catalog.disabled(),
            vec!["beta".to_string()],
            "alpha must be re-enabled, not left behind"
        );
    }

    /// A reload sees a skill created after the registry was built.
    #[test]
    fn reload_picks_up_a_new_skill() {
        let (dir, catalog) = catalog_with(&[("alpha", false)]);
        assert!(
            catalog.entries().iter().any(|entry| entry.name == "alpha"),
            "the fixture skill is there before the reload"
        );

        let new_dir = dir.path().join(".tact/skills/gamma");
        std::fs::create_dir_all(&new_dir).unwrap();
        std::fs::write(
            new_dir.join("SKILL.md"),
            "---\nname: gamma\ndescription: gamma desc\n---\nbody",
        )
        .unwrap();

        let entries = catalog.reload().expect("reload");
        assert!(
            entries.iter().any(|entry| entry.name == "gamma"),
            "reload must rescan the workspace: {entries:?}"
        );
    }
}
