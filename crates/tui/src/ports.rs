//! The narrow ports this view needs from its host.
//!
//! `tui` used to reach into the extension crate for these: the skill registry,
//! the session store, voice settings and the marketplace worker all arrived as
//! `tact::*` types, which is why the view could not be compiled without the
//! runtime. The traits here are the seam the approved design describes — they
//! name what the *view* needs (list skills, load history, start recording) in
//! terms the view already owns, and `tact-ui` implements them over extension
//! services.
//!
//! They are deliberately small. A port that grows a method for every caller
//! becomes the god object it replaced, so each one answers exactly the questions
//! the view asks and nothing else.

use agent_tui_kit::state::SkillEntry;

/// What the view needs from the skill catalog.
///
/// Listing is the view's whole interest: `/skill list`, the palette, the slash
/// picker and the disabled marker all read a snapshot, and `/skill reload` and
/// `/skill enable|disable` ask for a fresh one. The registry's internals — file
/// discovery, frontmatter parsing, the description budget — stay with the
/// plugin that owns them.
pub trait SkillCatalog: Send + Sync {
    /// Every known skill, including the disabled ones.
    ///
    /// Disabled rows are *marked*, not hidden: a row that silently vanishes is
    /// indistinguishable from a broken install.
    fn entries(&self) -> Vec<SkillEntry>;

    /// Re-read the catalog from disk and return the refreshed entries.
    ///
    /// Blocking on purpose: the caller runs it inside `spawn_blocking`, and
    /// making the trait async would force every fake in a test to carry a
    /// runtime.
    fn reload(&self) -> anyhow::Result<Vec<SkillEntry>>;

    /// The names currently disabled, sorted.
    ///
    /// The whole set, not a per-skill query: `/skill disable` rebuilds the list
    /// it writes to config from this, and a partial list would silently
    /// re-enable everything it left out.
    fn disabled(&self) -> Vec<String>;

    /// Replace the disabled set and return the refreshed entries.
    fn set_disabled(&self, names: Vec<String>) -> anyhow::Result<Vec<SkillEntry>>;

    /// The catalog rendered for the system prompt.
    ///
    /// Empty for a catalog that has nothing to say; the view shows it in the
    /// prompt-inspection popup.
    fn description(&self) -> String;
}

/// A prompt waiting for the user, as the view renders it.
///
/// A view reconciles from a snapshot rather than trusting one event: a request
/// event can be missed (the view attaches late) or duplicated, and a waiter that
/// was dropped must not leave a ghost popup on screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingPrompt {
    pub request_id: u64,
    pub prompt: String,
    pub options: Vec<String>,
    /// Whether the answer is a set of indices rather than one.
    pub multi: bool,
    pub log_confirm: bool,
    /// The request came from a `spawn_subagent` child, so the popup titles
    /// itself accordingly. The prompt itself stays clean — the model wrote it.
    pub from_subagent: bool,
}

/// Which provider family a target uses.
///
/// The picker only needs this much: which effort tiers a model offers depends on
/// the family (`deepseek`/`kimi` reject `minimal`/`medium`, openai accepts the
/// range). Naming the family keeps the model library's `ProviderInfo` out of the
/// view, which had it in two signatures for one `match`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProviderFamily {
    #[default]
    OpenAi,
    DeepSeek,
    Kimi,
    Anthropic,
    Custom,
}

/// What the view needs from the interaction broker.
///
/// The runtime's broker is richer (it also carries the request kind and the
/// answer channel); the view only ever asks these three questions, so the port
/// stays this small on purpose. A method per caller would rebuild the object it
/// replaced.
pub trait InteractionPort: Send + Sync {
    /// The prompts waiting for an answer, oldest first.
    fn pending(&self) -> Vec<PendingPrompt>;

    /// Answer one prompt. Returns whether a waiter was actually waiting.
    ///
    /// `false` means the request had already been answered or withdrawn — the
    /// caller logs it rather than pretending the answer landed.
    fn respond(&self, response: tact_protocol::UiResponse) -> bool;

    /// Abandon one prompt, if it is still waiting.
    fn withdraw(&self, request_id: u64) -> bool;
}

/// Which target a `/model` change applies to.
///
/// The view branches on this in a dozen places (which prompt to show, which
/// persist call to make). Naming it here keeps the branch in one vocabulary
/// instead of passing `bool is_subagent` around.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelTarget {
    Main,
    Subagent,
}

/// A model choice the user made, as the writer needs it.
///
/// The two semantics are mutually exclusive: an effort-semantic model carries an
/// effort and no budget, a budget-semantic one the reverse. Modelling that as an
/// enum means neither can be silently dropped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelChoice {
    /// Effort semantics (openai / deepseek / kimi k3).
    Effort {
        model: String,
        effort: tact_protocol::ReasoningEffort,
    },
    /// Budget semantics (anthropic / kimi coding).
    Budget { model: String, budget: usize },
}

/// What the view needs in order to *write* configuration.
///
/// Separate from reading (`config_snapshot`): a read is a value the host handed
/// over, a write changes the file the next run reads. Keeping them apart is what
/// lets the view be tested with a snapshot and a recording writer, without a
/// config file on disk.
pub trait SettingsWriter: Send + Sync {
    /// Remember a model choice in memory, for this session.
    fn apply_model_choice(&self, target: ModelTarget, choice: &ModelChoice) -> anyhow::Result<()>;

    /// Write a model choice to the config file, when there is one.
    fn persist_model_choice(&self, target: ModelTarget, choice: &ModelChoice)
    -> anyhow::Result<()>;

    /// Write back one `[ui]` preference.
    ///
    /// `Setting` is an enum rather than a key string: the set is closed, and a
    /// typo in a key would silently write nothing.
    fn persist_ui(&self, setting: UiSetting) -> anyhow::Result<()>;

    /// Write back the skill-disable list.
    fn persist_disabled_skills(&self, names: &[String]) -> anyhow::Result<()>;

    /// The current settings, projected for the view.
    ///
    /// Read *after* a write: a write can recompute derived values (a budget that
    /// no longer fits raises `max_tokens`), and the view has to show the new
    /// ceiling. Projecting here rather than handing over the runtime's whole
    /// configuration keeps the view free of that tree.
    fn current_settings(&self) -> Option<ViewSettings>;
}

/// The `[ui]` preferences the view can write back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UiSetting {
    Theme(String),
    Language(String),
    /// Whether hook-injected content is drawn in the log.
    HookOutput(bool),
}

/// One persisted conversation turn, as the log renders it.
///
/// The view needs the role, the text blocks, and whether the cell is derived
/// context (a hook injection or the Goal runner's per-activation block) rather
/// than something the user typed. Naming that here keeps the runtime's `Message`
/// out of the view: the two signals that matter are decided by the store's
/// reader, which is the only code that can see the metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryTurn {
    pub role: HistoryRole,
    /// The text of each `Text` block, in order. Tool calls and results are not
    /// rendered by the log, so they are not carried.
    pub text: String,
    /// A hook's injected context, rendered as a hook cell rather than a turn.
    pub is_hook_context: bool,
    /// The Goal runner's per-activation block, not rendered at all.
    ///
    /// Derived data the Goal panel already shows, re-injected every activation;
    /// a row per activation would only push the conversation off the log.
    pub is_goal_context: bool,
}

/// Who wrote a turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryRole {
    User,
    Assistant,
}

/// What the view needs to restore a session's history.
///
/// One question: the turns, in order. The store owns how they are read.
pub trait SessionHistory: Send + Sync {
    /// The persisted turns for a session, oldest first.
    ///
    /// Blocking on purpose: the caller runs it at startup before the loop, and
    /// making the trait async would force every test fake to carry a runtime.
    fn turns(&self, session_id: &str) -> anyhow::Result<Vec<HistoryTurn>>;

    /// The most recent request body, for the prompt-inspection popup.
    ///
    /// `None` when nothing was persisted (a fresh session).
    fn latest_request_body(&self, session_id: &str) -> anyhow::Result<Option<String>>;
}

/// What the view needs from the voice worker.
///
/// The runtime's handle owns a task, a shutdown token and two channels; the view
/// only sends three commands and, when it closes, asks the worker to stop. Naming
/// those four operations keeps the task plumbing out of the view.
pub trait VoicePort: Send + Sync {
    /// Send one command to the worker.
    ///
    /// Returns whether it was delivered: a worker that already exited is not an
    /// error the user needs to see, but a caller may want to log it.
    fn send(&self, command: tact_protocol::VoiceCommand) -> bool;

    /// Whether the view should offer the voice button at all.
    ///
    /// `false` for a run with voice disabled, which is the common case: no
    /// worker is started and nothing is drawn.
    fn is_enabled(&self) -> bool;

    /// Whether the configured provider has no API key.
    ///
    /// A voice button that works but cannot transcribe is worse than one that
    /// says why, so the state carries it.
    fn missing_api_key(&self) -> bool;
}

/// The configuration values the `/model` flow needs.
///
/// A projection, not the runtime's whole resolved configuration: the view reads
/// eight fields and never the tree around them, and passing the tree is what made
/// every picker test depend on the config subsystem.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ViewSettings {
    /// The configured main model.
    pub model: String,
    /// The configured provider label, for the picker prompt.
    pub provider_label: String,
    /// Provider-reported model ids plus the configured ones.
    pub models: Vec<String>,
    /// Per-model effort tiers and thinking budgets.
    pub model_profiles: std::collections::HashMap<String, ModelProfile>,
    /// The current effort for the main target.
    pub reasoning_effort: Option<tact_protocol::ReasoningEffort>,
    /// The current thinking budget for the main target.
    pub thinking_budget: usize,
    /// The ceiling on output tokens, shown in the status bar.
    pub max_tokens: u32,
    /// Whether a config file exists to persist into.
    pub has_config_path: bool,
    /// The subagent target, when configured.
    pub subagent: Option<SubagentSettings>,
}

/// One model's selectable tiers.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ModelProfile {
    pub reasoning_efforts: Vec<tact_protocol::ReasoningEffort>,
    pub thinking_budgets: Vec<usize>,
}

/// The subagent target's settings.
#[derive(Debug, Clone, PartialEq)]
pub struct SubagentSettings {
    pub model: String,
    pub models: Vec<String>,
    pub reasoning_effort: Option<tact_protocol::ReasoningEffort>,
    pub thinking_budget: usize,
    pub provider_label: String,
    pub family: ProviderFamily,
}

/// What the view needs to offer commands: names, one-line help, and the rules
/// that decide whether a command can run now.
///
/// The catalog is where a command's own metadata lives. The view used to derive
/// the same list from its own enum, which meant a command contributed by a
/// plugin was unreachable from the palette, and a command the runtime no longer
/// had still appeared there.
pub trait CommandDirectory: Send + Sync {
    /// Every command, in the order the catalog holds them.
    fn commands(&self) -> Vec<CommandEntry>;

    /// Resolve a name the user typed, honouring aliases.
    ///
    /// `None` means "not a catalog command" — the view still has local ones, and
    /// the caller falls back to them rather than treating it as an error.
    fn resolve(&self, name: &str) -> Option<CommandEntry>;
}

/// The runtime's command directory, installed after the view is already up.
///
/// The TUI comes up before the agent is built (a deliberate choice: users should
/// not stare at a blank terminal while MCP connects), so the catalog does not
/// exist when `TuiConfig` is assembled. A shared slot lets the composition root
/// fill it once the runtime is ready, without the view polling or the protocol
/// carrying a view-only concern.
pub type SharedCommandDirectory =
    std::sync::Arc<std::sync::Mutex<Option<std::sync::Arc<dyn CommandDirectory>>>>;

/// An empty slot, ready to be filled.
#[must_use]
pub fn shared_command_directory() -> SharedCommandDirectory {
    std::sync::Arc::new(std::sync::Mutex::new(None))
}

/// One command, as a picker shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandEntry {
    /// What the user types, without the leading `/`.
    pub name: String,
    /// One line, shown beside the name.
    pub summary: String,
    /// Whether the command needs arguments before it can run.
    pub needs_args: bool,
}

/// What the view needs to populate the `/model` pickers.
///
/// Everything here is a *query* about the current provider and the model ids the
/// endpoint reports. The view does not care how those are discovered — it needs
/// the merged candidate list and the current ids — so the port stays a question
/// rather than exposing the model library's client.
pub trait ModelCatalog: Send + Sync {
    /// The configured models, plus any the endpoint reported.
    ///
    /// Blocking on purpose: the caller already runs it inside `block_in_place`
    /// when a runtime exists, and making the trait async would force every test
    /// fake to carry one.
    fn candidates(&self, configured: &[String]) -> Vec<String>;

    /// The configured models, plus the ids `provider` reports.
    ///
    /// A subagent can target a different provider than the main run, so the
    /// endpoint to ask is a parameter rather than the process global.
    fn candidates_for(&self, configured: &[String], provider_label: &str) -> Vec<String>;

    /// Whether `model` selects a reasoning effort rather than a thinking budget.
    ///
    /// The two are mutually exclusive on the wire, so the picker's second step
    /// branches on this.
    fn uses_effort(&self, model: &str) -> bool;

    /// The configured provider's label, for the picker prompt.
    fn provider_label(&self) -> String;

    /// The family the main run's provider belongs to.
    fn family(&self) -> ProviderFamily;

    /// The family a subagent's provider belongs to.
    fn subagent_family(&self) -> Option<ProviderFamily>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, disabled: bool) -> SkillEntry {
        SkillEntry {
            name: name.to_string(),
            description: format!("{name} description"),
            body: String::new(),
            dir: format!("/skills/{name}"),
            disabled,
        }
    }

    /// The view's production code reaches skills only through this port.
    ///
    /// This is the property that lets `tui` be compiled without the extension
    /// crate: the registry, its discovery rules and its frontmatter parsing stay
    /// with the plugin that owns them. A `tact::skill` in the production region
    /// would quietly restore the edge the port exists to remove.
    #[test]
    fn production_code_reaches_skills_only_through_the_port() {
        let mut offenders = Vec::new();
        for entry in std::fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/src"))
            .expect("the source directory is readable")
        {
            let path = entry.expect("a directory entry").path();
            if path.is_dir() {
                continue;
            }
            if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
                continue;
            }
            let text = std::fs::read_to_string(&path).expect("a readable source file");
            // The test modules may still use the real registry to build fixtures;
            // what matters is that production code does not.
            let production = text.split("#[cfg(test)]").next().unwrap_or_default();
            if production.contains("tact::skill") {
                offenders.push(
                    path.file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or_default()
                        .to_string(),
                );
            }
        }
        assert!(
            offenders.is_empty(),
            "these files reach into tact::skill from production code: {offenders:?}"
        );
    }

    /// The view's production code reaches interaction only through this port.
    ///
    /// The runtime's responder is one implementation; a view that named it
    /// directly could not be driven by anything else, and the reverse
    /// `DriverMessage::UiResponse` path would have to stay alive beside it.
    #[test]
    fn production_code_reaches_interaction_only_through_the_port() {
        let mut offenders = Vec::new();
        for entry in std::fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/src"))
            .expect("the source directory is readable")
        {
            let path = entry.expect("a directory entry").path();
            if path.is_dir() {
                continue;
            }
            if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
                continue;
            }
            let text = std::fs::read_to_string(&path).expect("a readable source file");
            let production = text.split("#[cfg(test)]").next().unwrap_or_default();
            if production.contains("tact::ui_responder") {
                offenders.push(
                    path.file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or_default()
                        .to_string(),
                );
            }
        }
        assert!(
            offenders.is_empty(),
            "these files reach into tact::ui_responder from production code: {offenders:?}"
        );
    }

    /// A catalog keeps the disabled rows and marks them.
    ///
    /// This is the contract `/skill list` depends on: hiding a disabled skill
    /// would make a deliberate user choice look like a missing install.
    #[test]
    fn disabled_skills_are_listed_not_hidden() {
        struct Catalog;
        impl SkillCatalog for Catalog {
            fn entries(&self) -> Vec<SkillEntry> {
                vec![entry("enabled", false), entry("off", true)]
            }
            fn reload(&self) -> anyhow::Result<Vec<SkillEntry>> {
                Ok(self.entries())
            }
            fn disabled(&self) -> Vec<String> {
                vec!["off".to_string()]
            }
            fn set_disabled(&self, _names: Vec<String>) -> anyhow::Result<Vec<SkillEntry>> {
                Ok(self.entries())
            }
            fn description(&self) -> String {
                String::new()
            }
        }

        let catalog = Catalog;
        let entries = catalog.entries();
        assert_eq!(entries.len(), 2, "both skills are listed");
        assert!(
            entries
                .iter()
                .any(|entry| entry.name == "off" && entry.disabled),
            "the disabled one is present and marked"
        );
    }
}
