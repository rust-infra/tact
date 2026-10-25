//! The system prompt's parts as kernel prompt sections.
//!
//! The Tera template decides *where* each part lands and how it is framed, so a
//! section cannot render its own prose: `prompt:role` supplies the `role`
//! variable, `prompt:guidelines` supplies the `guidelines` list, and so on. That
//! keeps the assembled prompt byte-identical to the old builder chain while
//! making each part a contribution a plugin set decides.
//!
//! [`PromptFragment`] is the typed value a section carries; [`assemble`] applies
//! the fragments in catalog order, which is why two sections cannot silently
//! overwrite each other's variable.

use anyhow::Result;
use tact_kernel::capability::CapabilityFilter;
use tact_kernel::capability::prompt::{PromptContext, PromptSection};
use tact_kernel::plugin::{Plugin, PluginHost, PluginId, PluginKind, PluginManifest, Registrar};

use crate::prompt::SystemPrompt;

/// One part of the system prompt, typed so the template keeps its framing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptFragment {
    /// The agent's role (`role`).
    Role(String),
    /// Soft rules, rendered as a bullet list (`guidelines`).
    Guidelines(Vec<String>),
    /// Hard rules, rendered as a bullet list (`constraints`).
    Constraints(Vec<String>),
    /// The skills catalog (`skills_available`).
    Skills(String),
    /// Retrieved memory (`memory`).
    Memory(String),
    /// When to save memory (`memory_guidance`).
    MemoryGuidance(String),
    /// Project instructions such as `AGENTS.md` (`additional`).
    Additional(String),
    /// Third-party prose from MCP servers (`mcp_instructions`).
    McpInstructions(String),
    /// The directory snapshot and other per-request context (`dynamic_context`).
    DynamicContext(String),
}

impl PromptFragment {
    fn id(&self) -> &'static str {
        match self {
            Self::Role(_) => "prompt:role",
            Self::Guidelines(_) => "prompt:guidelines",
            Self::Constraints(_) => "prompt:constraints",
            Self::Skills(_) => "prompt:skills",
            Self::Memory(_) => "prompt:memory",
            Self::MemoryGuidance(_) => "prompt:memory_guidance",
            Self::Additional(_) => "prompt:additional",
            Self::McpInstructions(_) => "prompt:mcp_instructions",
            Self::DynamicContext(_) => "prompt:dynamic_context",
        }
    }

    /// The template variable this fragment fills.
    ///
    /// The catalog orders sections by priority, and the template decides the
    /// final layout; this mapping is the only place the two names meet.
    fn variable(&self) -> &'static str {
        match self {
            Self::Role(_) => "role",
            Self::Guidelines(_) => "guidelines",
            Self::Constraints(_) => "constraints",
            Self::Skills(_) => "skills_available",
            Self::Memory(_) => "memory",
            Self::MemoryGuidance(_) => "memory_guidance",
            Self::Additional(_) => "additional",
            Self::McpInstructions(_) => "mcp_instructions",
            Self::DynamicContext(_) => "dynamic_context",
        }
    }

    /// Template order: role, skills, guidelines, constraints, memory guidance,
    /// additional, MCP, then the dynamic suffix after `DYNAMIC_BOUNDARY`.
    fn priority(&self) -> i32 {
        match self {
            Self::Role(_) => 0,
            Self::Skills(_) => 10,
            Self::Guidelines(_) => 20,
            Self::Constraints(_) => 30,
            Self::MemoryGuidance(_) => 40,
            Self::Additional(_) => 50,
            Self::McpInstructions(_) => 60,
            Self::Memory(_) => 70,
            Self::DynamicContext(_) => 80,
        }
    }

    fn is_empty(&self) -> bool {
        match self {
            Self::Role(body)
            | Self::Skills(body)
            | Self::Memory(body)
            | Self::MemoryGuidance(body)
            | Self::Additional(body)
            | Self::McpInstructions(body)
            | Self::DynamicContext(body) => body.trim().is_empty(),
            Self::Guidelines(items) | Self::Constraints(items) => {
                items.iter().all(|item| item.trim().is_empty())
            }
        }
    }
}

/// A prompt section that carries one [`PromptFragment`].
pub struct FragmentSection(PromptFragment);

impl PromptSection for FragmentSection {
    fn id(&self) -> &'static str {
        self.0.id()
    }

    fn priority(&self) -> i32 {
        self.0.priority()
    }

    fn render(&self, _ctx: &PromptContext) -> Option<String> {
        // An empty fragment contributes nothing, matching the template's
        // `{% if … %}` guards: the value is only material if it is non-empty.
        (!self.0.is_empty()).then(|| self.0.variable().to_string())
    }
}

/// The plugin that contributes one section.
struct SectionPlugin(PromptFragment);

#[async_trait::async_trait]
impl Plugin for SectionPlugin {
    fn manifest(&self) -> PluginManifest {
        let id = self
            .0
            .id()
            .replace(':', ".")
            .replacen("prompt.", "builtin.prompt.", 1);
        PluginManifest::new(
            PluginId::new(id).expect("built-in section ids are valid"),
            "system prompt section",
            env!("CARGO_PKG_VERSION"),
            PluginKind::Capability,
            "built-in system prompt section",
        )
    }

    fn register(&self, registrar: &mut Registrar<'_>) -> Result<()> {
        registrar.prompt_section(FragmentSection(self.0.clone()))
    }
}

/// Install `fragments` as plugins and return the sections that survived the
/// filter, in template order.
pub fn install_fragments(
    fragments: Vec<PromptFragment>,
    filter: CapabilityFilter,
) -> Result<Vec<PromptFragment>> {
    let mut host = PluginHost::new(filter);
    for fragment in &fragments {
        host.install_sync(SectionPlugin(fragment.clone()))?;
    }
    let catalog = host.seal_sync()?.prompts().clone();
    // The catalogue proves which ids were installed; the fragments themselves
    // carry the values, so map back through the id.
    let installed: Vec<&str> = catalog
        .sections()
        .iter()
        .map(|section| section.id())
        .collect();
    Ok(installed
        .iter()
        .filter_map(|id| {
            // `sections()` is already priority-ordered, and so is the fragment
            // list, so a linear scan per id is enough and keeps the values.
            fragments_for(id, &fragments)
        })
        .collect())
}

fn fragments_for(id: &str, fragments: &[PromptFragment]) -> Option<PromptFragment> {
    fragments
        .iter()
        .find(|fragment| fragment.id() == id)
        .cloned()
}

/// Apply the fragments to a builder, in template order.
///
/// A fragment whose value is empty is skipped, exactly as the template's guards
/// would skip it — so `Some("")` cannot produce an empty `# Your role` header.
pub fn apply(
    mut builder: crate::prompt::SystemPromptBuilder,
    fragments: &[PromptFragment],
) -> Result<SystemPrompt> {
    for fragment in fragments {
        if fragment.is_empty() {
            continue;
        }
        match fragment {
            PromptFragment::Role(body) => {
                builder.role(body.clone());
            }
            PromptFragment::Skills(body) => {
                builder.skills_available(body.clone());
            }
            PromptFragment::Guidelines(items) => {
                for item in items {
                    builder.add_guideline(item);
                }
            }
            PromptFragment::Constraints(items) => {
                for item in items {
                    builder.add_constraint(item);
                }
            }
            PromptFragment::MemoryGuidance(body) => {
                builder.memory_guidance(body.clone());
            }
            PromptFragment::Additional(body) => {
                builder.additional(body.clone());
            }
            PromptFragment::McpInstructions(body) => {
                builder.mcp_instructions(body.clone());
            }
            PromptFragment::Memory(body) => {
                builder.memory(body.clone());
            }
            PromptFragment::DynamicContext(body) => {
                builder.dynamic_context(body.clone());
            }
        }
    }
    builder.build().map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn context() -> PromptContext {
        PromptContext {
            workspace: PathBuf::from("/repo"),
            session_id: None,
        }
    }

    fn full() -> Vec<PromptFragment> {
        vec![
            PromptFragment::Role("Senior Rust engineer".into()),
            PromptFragment::Skills("- bash".into()),
            PromptFragment::Guidelines(vec!["Keep functions small".into()]),
            PromptFragment::Constraints(vec!["Never expose secrets".into()]),
            PromptFragment::MemoryGuidance("Save preferences".into()),
            PromptFragment::Additional("# Project rules".into()),
            PromptFragment::McpInstructions("use the docs server".into()),
            PromptFragment::Memory("you discussed async".into()),
            PromptFragment::DynamicContext("src/prompt/mod.rs".into()),
        ]
    }

    #[test]
    fn fragments_reproduce_the_builder_output() {
        // The point of the typed fragment: the Tera template keeps deciding the
        // framing, so moving assembly into plugins cannot change one byte.
        let expected = SystemPrompt::builder()
            .role("Senior Rust engineer")
            .skills_available("- bash")
            .add_guideline("Keep functions small")
            .add_constraint("Never expose secrets")
            .memory_guidance("Save preferences")
            .additional("# Project rules")
            .mcp_instructions("use the docs server")
            .memory("you discussed async")
            .dynamic_context("src/prompt/mod.rs")
            .build()
            .unwrap()
            .to_prompt()
            .render()
            .unwrap();

        let actual = apply(SystemPrompt::builder(), &full())
            .unwrap()
            .to_prompt()
            .render()
            .unwrap();
        assert_eq!(actual, expected);
    }

    #[test]
    fn an_empty_fragment_is_skipped_like_the_template_guard() {
        let with_empty = vec![
            PromptFragment::Role(String::new()),
            PromptFragment::Guidelines(Vec::new()),
            PromptFragment::Memory("body".into()),
        ];
        let actual = apply(SystemPrompt::builder(), &with_empty)
            .unwrap()
            .to_prompt()
            .render()
            .unwrap();
        assert!(!actual.contains("# Your role"), "{actual}");
        assert!(!actual.contains("# Guidelines"), "{actual}");
        assert!(actual.contains("body"));
    }

    #[test]
    fn sections_are_installed_in_template_order() {
        let installed = install_fragments(full(), CapabilityFilter::default()).unwrap();
        let ids: Vec<&str> = installed.iter().map(PromptFragment::id).collect();
        assert_eq!(
            ids,
            vec![
                "prompt:role",
                "prompt:skills",
                "prompt:guidelines",
                "prompt:constraints",
                "prompt:memory_guidance",
                "prompt:additional",
                "prompt:mcp_instructions",
                "prompt:memory",
                "prompt:dynamic_context",
            ]
        );
    }

    #[test]
    fn a_disabled_fragment_is_absent() {
        let disabled: tact_kernel::capability::CapabilityId =
            "prompt:prompt:skills".parse().unwrap();
        let installed = install_fragments(full(), CapabilityFilter::new([disabled])).unwrap();
        assert!(
            installed
                .iter()
                .all(|fragment| fragment.id() != "prompt:skills")
        );
        assert_eq!(installed.len(), 8);
    }

    #[test]
    fn every_fragment_has_a_distinct_variable_and_id() {
        let fragments = full();
        let ids: std::collections::BTreeSet<&str> =
            fragments.iter().map(PromptFragment::id).collect();
        let variables: std::collections::BTreeSet<&str> =
            fragments.iter().map(PromptFragment::variable).collect();
        assert_eq!(ids.len(), fragments.len());
        assert_eq!(variables.len(), fragments.len());
    }

    #[test]
    fn render_reports_the_variable_not_the_body() {
        // A section that rendered its own prose would be framed by nothing; the
        // template owns the headers, so the section only names its variable.
        let section = FragmentSection(PromptFragment::Memory("body".into()));
        assert_eq!(section.render(&context()).as_deref(), Some("memory"));
    }
}
