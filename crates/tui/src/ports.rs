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
    /// `UserCommand::UiResponse` path would have to stay alive beside it.
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
