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
            .map(|info| family_of(info.provider))
            .unwrap_or_default()
    }

    fn subagent_family(&self) -> Option<tui::ports::ProviderFamily> {
        // Read from the resolved configuration, not the process provider: a
        // subagent may target a different endpoint than the main run, and the
        // picker's tiers follow the target.
        let config = tact::config::try_settings()?;
        let subagent = config.agent.subagent?;
        Some(family_of(subagent.provider.provider))
    }
}

/// Map the model library's provider kind onto the view's family.
///
/// One `match` in one place: the view never sees `ProviderKind`, so adding a
/// provider kind fails to compile here rather than silently rendering as the
/// openai tier list.
fn family_of(kind: tact_llm::ProviderKind) -> tui::ports::ProviderFamily {
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

#[cfg(test)]
mod tests {
    use super::*;

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
