//! Registration catalogs.
//!
//! Catalogs are keyed by stable names and reject duplicates at commit time.
//! They expose read-only lookups; handler objects never leave the catalog.
//! A commit applies to a clone and replaces the catalogs only after every
//! contribution validates, so a failed plugin leaves no partial entries.

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::capability::command::{Command, CommandMeta};
use crate::capability::prompt::PromptSection;
use crate::capability::tool::Tool;
use crate::capability::view::View;
use crate::capability::{SkippedCapability, validate_name};

#[derive(Clone, Default)]
pub struct ToolCatalog {
    tools: BTreeMap<String, Arc<dyn Tool>>,
}

impl ToolCatalog {
    pub fn insert(&mut self, tool: Arc<dyn Tool>) -> anyhow::Result<()> {
        let descriptor = tool.descriptor();
        validate_name(&descriptor.name)?;
        if self.tools.contains_key(&descriptor.name) {
            anyhow::bail!("duplicate tool: {}", descriptor.name);
        }
        self.tools.insert(descriptor.name, tool);
        Ok(())
    }

    pub fn get(&self, name: &str) -> Option<&Arc<dyn Tool>> {
        self.tools.get(name)
    }

    pub fn len(&self) -> usize {
        self.tools.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }
}

#[derive(Clone, Default)]
pub struct CommandCatalog {
    commands: BTreeMap<String, Arc<dyn Command>>,
    aliases: BTreeMap<String, String>,
}

impl CommandCatalog {
    pub fn insert(&mut self, command: Arc<dyn Command>) -> anyhow::Result<()> {
        let meta: &CommandMeta = command.meta();
        validate_name(&meta.name)?;
        if self.commands.contains_key(&meta.name) {
            anyhow::bail!("duplicate command: {}", meta.name);
        }
        if self.aliases.contains_key(&meta.name) {
            anyhow::bail!("command name shadows an existing alias: {}", meta.name);
        }
        for (index, alias) in meta.aliases.iter().enumerate() {
            validate_name(alias)?;
            if self.aliases.contains_key(alias)
                || self.commands.contains_key(alias)
                || meta.aliases[..index].contains(alias)
            {
                anyhow::bail!("duplicate command alias: {alias}");
            }
        }
        for alias in &meta.aliases {
            self.aliases.insert(alias.clone(), meta.name.clone());
        }
        self.commands.insert(meta.name.clone(), command);
        Ok(())
    }

    pub fn get(&self, name: &str) -> Option<&Arc<dyn Command>> {
        let name = self.aliases.get(name).map_or(name, String::as_str);
        self.commands.get(name)
    }

    pub fn len(&self) -> usize {
        self.commands.len()
    }

    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }
}

#[derive(Clone, Default)]
pub struct PromptCatalog {
    sections: BTreeMap<&'static str, Arc<dyn PromptSection>>,
}

impl PromptCatalog {
    pub fn insert(&mut self, section: Arc<dyn PromptSection>) -> anyhow::Result<()> {
        let id = section.id();
        validate_name(id)?;
        if self.sections.contains_key(id) {
            anyhow::bail!("duplicate prompt section id: {id}");
        }
        self.sections.insert(id, section);
        Ok(())
    }

    pub fn sections(&self) -> Vec<Arc<dyn PromptSection>> {
        let mut sections: Vec<_> = self.sections.values().cloned().collect();
        sections.sort_by(|a, b| {
            a.priority()
                .cmp(&b.priority())
                .then_with(|| a.id().cmp(b.id()))
        });
        sections
    }
}

#[derive(Clone, Default)]
pub struct ViewCatalog {
    views: BTreeMap<&'static str, Arc<dyn View>>,
}

impl ViewCatalog {
    pub fn insert(&mut self, view: Arc<dyn View>) -> anyhow::Result<()> {
        let id = view.id();
        validate_name(id)?;
        if self.views.contains_key(id) {
            anyhow::bail!("duplicate view id: {id}");
        }
        self.views.insert(id, view);
        Ok(())
    }

    pub fn get(&self, id: &str) -> Option<&Arc<dyn View>> {
        self.views.get(id)
    }
}

#[derive(Clone, Default)]
pub struct Catalogs {
    pub tools: ToolCatalog,
    pub commands: CommandCatalog,
    pub prompts: PromptCatalog,
    pub views: ViewCatalog,
}

impl Catalogs {
    /// Apply every staged contribution atomically.
    ///
    /// The batch is drained into a clone of the catalogs; the clone replaces
    /// the live catalogs only when every insertion succeeded. A duplicate or
    /// bad contribution therefore leaves the previous catalog untouched.
    pub fn commit(&mut self, batch: &mut ContributionBatch) -> anyhow::Result<()> {
        let mut next = self.clone();
        for tool in batch.tools.drain(..) {
            next.tools.insert(tool)?;
        }
        for command in batch.commands.drain(..) {
            next.commands.insert(command)?;
        }
        for section in batch.prompts.drain(..) {
            next.prompts.insert(section)?;
        }
        for view in batch.views.drain(..) {
            next.views.insert(view)?;
        }
        *self = next;
        Ok(())
    }
}

#[derive(Default)]
pub struct ContributionBatch {
    pub tools: Vec<Arc<dyn Tool>>,
    pub commands: Vec<Arc<dyn Command>>,
    pub prompts: Vec<Arc<dyn PromptSection>>,
    pub views: Vec<Arc<dyn View>>,
    pub skipped: Vec<SkippedCapability>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::command::{Command, CommandMeta, CommandScope};

    struct FixedCommand {
        meta: CommandMeta,
    }

    impl FixedCommand {
        fn new(name: &str, aliases: &[&str]) -> Self {
            Self {
                meta: CommandMeta {
                    name: name.to_string(),
                    aliases: aliases.iter().map(|alias| (*alias).to_string()).collect(),
                    summary: String::new(),
                    usage: String::new(),
                    scope: CommandScope::Runtime,
                    needs_args: false,
                },
            }
        }
    }

    impl Command for FixedCommand {
        fn meta(&self) -> &CommandMeta {
            &self.meta
        }
    }

    #[test]
    fn command_name_cannot_shadow_an_existing_alias() {
        let mut catalog = CommandCatalog::default();
        catalog
            .insert(Arc::new(FixedCommand::new("first", &["x"])))
            .unwrap();
        assert!(
            catalog
                .insert(Arc::new(FixedCommand::new("x", &[])))
                .is_err()
        );
        assert_eq!(catalog.get("x").unwrap().meta().name, "first");
    }

    #[test]
    fn duplicate_alias_within_one_command_is_rejected() {
        let mut catalog = CommandCatalog::default();
        assert!(
            catalog
                .insert(Arc::new(FixedCommand::new("first", &["x", "x"])))
                .is_err()
        );
    }
}

#[cfg(test)]
mod prompt_tests {
    use super::*;
    use crate::capability::prompt::{PromptContext, PromptSection};
    use std::path::PathBuf;

    struct FixedPrompt(&'static str, i32);

    impl PromptSection for FixedPrompt {
        fn id(&self) -> &'static str {
            self.0
        }

        fn priority(&self) -> i32 {
            self.1
        }

        fn render(&self, _ctx: &PromptContext) -> Option<String> {
            Some(self.0.to_string())
        }
    }

    #[test]
    fn prompt_sections_order_by_priority_then_id() {
        let mut catalog = PromptCatalog::default();
        catalog.insert(Arc::new(FixedPrompt("b", 10))).unwrap();
        catalog.insert(Arc::new(FixedPrompt("a", 10))).unwrap();
        catalog.insert(Arc::new(FixedPrompt("z", 1))).unwrap();

        let ids: Vec<_> = catalog
            .sections()
            .iter()
            .map(|section| section.id())
            .collect();
        assert_eq!(ids, vec!["z", "a", "b"]);
    }

    #[test]
    fn duplicate_prompt_section_id_is_rejected() {
        let mut catalog = PromptCatalog::default();
        catalog.insert(Arc::new(FixedPrompt("same", 1))).unwrap();
        assert!(catalog.insert(Arc::new(FixedPrompt("same", 2))).is_err());
        assert_eq!(catalog.sections().len(), 1);
    }

    #[test]
    fn prompt_context_is_constructible_without_tact_types() {
        let context = PromptContext {
            workspace: PathBuf::from("/repo"),
            session_id: None,
        };
        assert_eq!(context.workspace, PathBuf::from("/repo"));
    }
}
