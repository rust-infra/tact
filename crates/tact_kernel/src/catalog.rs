//! Registration catalogs.
//!
//! Catalogs are keyed by stable names and reject duplicates at commit time.
//! They expose read-only lookups; handler objects never leave the catalog.
//! A commit applies to a clone and replaces the catalogs only after every
//! contribution validates, so a failed plugin leaves no partial entries.

use std::collections::BTreeMap;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use crate::capability::command::{
    Command, CommandCandidate, CommandInvocation, CommandMeta, CommandOutcome,
};
use crate::capability::prompt::PromptSection;
use crate::capability::tool::Tool;
use crate::capability::view::View;
use crate::capability::{
    CapabilityFilter, CapabilityId, CapabilityKind, SkippedCapability, validate_name,
};
use crate::tool::descriptor::ToolDescriptor;
use crate::tool::source::{ToolDescriptorSnapshot, ToolSource};

pub struct ToolCatalog {
    tools: BTreeMap<String, Arc<dyn Tool>>,
    /// Shared by every clone: a source registered through one handle is
    /// visible to the catalog that dispatches.
    sources: Arc<std::sync::Mutex<Vec<Arc<dyn ToolSource>>>>,
    /// Shared by every clone, so a source added after a clone is visible to it
    /// and the generation both of them report cannot drift.
    generation: Arc<AtomicU64>,
}

impl Clone for ToolCatalog {
    fn clone(&self) -> Self {
        Self {
            tools: self.tools.clone(),
            sources: self.sources.clone(),
            generation: self.generation.clone(),
        }
    }
}

impl Default for ToolCatalog {
    fn default() -> Self {
        Self {
            tools: BTreeMap::new(),
            sources: Arc::new(std::sync::Mutex::new(Vec::new())),
            generation: Arc::new(AtomicU64::new(0)),
        }
    }
}

impl ToolCatalog {
    pub fn insert(&mut self, tool: Arc<dyn Tool>) -> anyhow::Result<()> {
        let descriptor = tool.descriptor();
        validate_name(&descriptor.name)?;
        if self.tools.contains_key(&descriptor.name) {
            anyhow::bail!("duplicate tool: {}", descriptor.name);
        }
        self.tools.insert(descriptor.name, tool);
        self.generation.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    /// Register a dynamic source.
    ///
    /// Takes `&self` so a host can add one through a shared `Arc<ToolCatalog>`
    /// (the executor holds it that way) without rebuilding the catalog and
    /// invalidating every ticket.
    pub fn add_source(&self, source: Arc<dyn ToolSource>) {
        let mut sources = self
            .sources
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        sources.push(source);
        self.generation.fetch_add(1, Ordering::SeqCst);
    }

    /// Drop every dynamic source. Used when the host replaces its MCP router.
    pub fn clear_sources(&self) {
        self.sources
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clear();
        self.generation.fetch_add(1, Ordering::SeqCst);
    }

    pub async fn refresh_sources(&self) -> anyhow::Result<()> {
        // Cloned out of the lock: `refresh` is async and may re-enter the
        // catalog through a tool.
        let sources: Vec<Arc<dyn ToolSource>> = self
            .sources
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone();
        let mut changed = false;
        for source in &sources {
            changed |= source.refresh().await?;
        }
        if changed {
            self.generation.fetch_add(1, Ordering::SeqCst);
        }
        Ok(())
    }

    /// The current generation, without rebuilding anything.
    ///
    /// What a host compares against the generation it advertised, so a check
    /// cannot pass merely because it refreshed and then read the refreshed value.
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    /// The source-contributed descriptors, synchronously.
    ///
    /// For a host that must rebuild its advertised list without awaiting: a
    /// source's descriptors are a pure function of its own state, so this cannot
    /// disagree with [`Self::descriptor_snapshot`].
    pub fn source_descriptors(&self) -> Vec<ToolDescriptor> {
        let sources: Vec<Arc<dyn ToolSource>> = self
            .sources
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone();
        sources
            .iter()
            .flat_map(|source| source.descriptors())
            .collect()
    }

    /// Whether `name` resolves to a handler for `filter`-allowed tools.
    ///
    /// The advertised list is filtered by the same rule, so a spec the host emits
    /// is always resolvable and vice versa.
    pub fn can_dispatch(&self, name: &str, filter: &CapabilityFilter) -> bool {
        let id = CapabilityId::new(CapabilityKind::Tool, name.to_string());
        filter.allows(&id) && self.handler_for(name).is_some()
    }

    pub async fn descriptor_snapshot(
        &self,
        filter: &CapabilityFilter,
    ) -> anyhow::Result<ToolDescriptorSnapshot> {
        let mut tools: Vec<ToolDescriptor> =
            self.tools.values().map(|tool| tool.descriptor()).collect();
        let sources: Vec<Arc<dyn ToolSource>> = self
            .sources
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone();
        for source in &sources {
            for tool in source.tools() {
                tools.push(tool.descriptor());
            }
        }
        let mut seen = std::collections::BTreeSet::new();
        tools.retain(|tool| {
            let id = CapabilityId::new(CapabilityKind::Tool, tool.name.clone());
            filter.allows(&id) && seen.insert(tool.name.clone())
        });
        tools.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(ToolDescriptorSnapshot {
            generation: self.generation.load(Ordering::SeqCst),
            tools,
        })
    }

    pub fn get(&self, name: &str) -> Option<&Arc<dyn Tool>> {
        self.tools.get(name)
    }

    /// The handler for `name`, whether a native insert or a dynamic source
    /// provided it.
    ///
    /// A native tool wins over a source tool of the same name, matching
    /// [`Self::descriptor_snapshot`], so a server cannot shadow a built-in.
    pub fn handler_for(&self, name: &str) -> Option<Arc<dyn Tool>> {
        if let Some(tool) = self.tools.get(name) {
            return Some(tool.clone());
        }
        let sources: Vec<Arc<dyn ToolSource>> = self
            .sources
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone();
        for source in &sources {
            for tool in source.tools() {
                if tool.descriptor().name == name {
                    return Some(tool);
                }
            }
        }
        None
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

    /// Run a command by name, resolving an alias.
    ///
    /// An unknown name is `Usage`, not an error: the host shows the same line
    /// whether the user mistyped a built-in or typed a command no plugin
    /// contributes, and neither should abort a turn.
    pub fn run(&self, invocation: &CommandInvocation) -> CommandOutcome {
        match self.get(&invocation.name) {
            Some(command) => command.run(invocation),
            None => CommandOutcome::Usage(format!("unknown command: /{}", invocation.name)),
        }
    }

    /// Route an invocation that may have to await.
    ///
    /// One lookup, same as [`Self::run`]: a pure command answers through its
    /// defaulted `run_async`, so the caller does not have to know which kind it
    /// is calling.
    pub async fn run_async(&self, invocation: &CommandInvocation) -> CommandOutcome {
        match self.get(&invocation.name) {
            Some(command) => command.run_async(invocation).await,
            None => CommandOutcome::Usage(format!("unknown command: /{}", invocation.name)),
        }
    }

    /// Every command's categories plus their own completions for `args`.
    ///
    /// The first token decides the command; everything after it is the command's
    /// grammar. A name that resolves to nothing contributes only itself, which is
    /// what keeps a typo completable back to a real command.
    pub fn complete(&self, query: &str) -> Vec<CommandCandidate> {
        let (name, args) = match query.split_once(char::is_whitespace) {
            Some((name, args)) => (name, args.trim_start()),
            None => (query, ""),
        };
        match self.get(name) {
            Some(command) if !args.is_empty() || command.meta().needs_args => command
                .complete(args)
                .into_iter()
                .map(|candidate| CommandCandidate {
                    path: format!("{name} {}", candidate.path.trim_start()),
                    detail: candidate.detail,
                    incomplete: candidate.incomplete,
                })
                .collect(),
            _ => self
                .commands
                .values()
                .filter(|command| command.meta().name.starts_with(name))
                .map(|command| CommandCandidate {
                    path: command.meta().name.clone(),
                    detail: command.meta().summary.clone(),
                    incomplete: command.meta().needs_args,
                })
                .collect(),
        }
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
    pub services: crate::service::ServiceRegistry,
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
        for service in batch.services.drain(..) {
            (service.0)(&mut next.services)?;
        }
        *self = next;
        Ok(())
    }
}

/// What one plugin's `register` produced, staged until the commit succeeds.
#[derive(Default)]
pub struct ContributionBatch {
    pub tools: Vec<Arc<dyn Tool>>,
    pub commands: Vec<Arc<dyn Command>>,
    pub prompts: Vec<Arc<dyn PromptSection>>,
    pub views: Vec<Arc<dyn View>>,
    /// Services this plugin provides, recorded at registration.
    ///
    /// Staged rather than applied directly so a plugin whose `register` fails
    /// leaves no half-provided service behind — the same transaction the four
    /// catalogs get.
    pub services: Vec<StagedService>,
    pub skipped: Vec<SkippedCapability>,
}

/// Applies one staged service to the registry.
///
/// A closure because the registry is keyed by `T` and the batch cannot name it:
/// `insert::<T>` is monomorphised at the call site, and this is what carries that
/// monomorphisation to commit time.
pub type ApplyService =
    Box<dyn FnOnce(&mut crate::service::ServiceRegistry) -> anyhow::Result<()> + Send>;

/// A service handle waiting to be committed.
pub struct StagedService(pub ApplyService);

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

#[cfg(test)]
mod snapshot_tests {
    use super::*;
    use crate::capability::CapabilityFilter;
    use crate::capability::tool::Tool;
    use crate::tool::context::ToolCallContext;
    use crate::tool::descriptor::ToolDescriptor;
    use crate::tool::source::ToolSource;
    use crate::tool::{ToolCallResult, ToolMetadata};
    use serde_json::json;

    struct FixedTool(&'static str);

    #[async_trait::async_trait]
    impl Tool for FixedTool {
        fn descriptor(&self) -> ToolDescriptor {
            ToolDescriptor::from_native(
                self.0,
                self.0,
                json!({"type": "object"}),
                &ToolMetadata::read_json(self.0, self.0, self.0),
            )
        }

        async fn call(
            &self,
            _ctx: ToolCallContext,
            _input: serde_json::Value,
        ) -> anyhow::Result<ToolCallResult> {
            Ok(ToolCallResult::text(self.0))
        }
    }

    /// A source whose `refresh` reports a real change.
    struct ChangingSource;

    #[async_trait::async_trait]
    impl ToolSource for ChangingSource {
        fn id(&self) -> &str {
            "changing"
        }

        async fn refresh(&self) -> anyhow::Result<bool> {
            Ok(true)
        }

        fn tools(&self) -> Vec<Arc<dyn Tool>> {
            Vec::new()
        }
    }

    struct FixedSource {
        id: &'static str,
        tools: Vec<Arc<dyn Tool>>,
    }

    #[async_trait::async_trait]
    impl ToolSource for FixedSource {
        fn id(&self) -> &str {
            self.id
        }

        async fn refresh(&self) -> anyhow::Result<bool> {
            Ok(false)
        }

        fn tools(&self) -> Vec<Arc<dyn Tool>> {
            self.tools.clone()
        }
    }

    fn source(id: &'static str, name: &'static str) -> Arc<dyn ToolSource> {
        Arc::new(FixedSource {
            id,
            tools: vec![Arc::new(FixedTool(name))],
        })
    }

    #[tokio::test]
    async fn snapshot_merges_sources_and_registration_bumps_generation() {
        let mut catalog = ToolCatalog::default();
        catalog.insert(Arc::new(FixedTool("native"))).unwrap();
        let before = catalog.generation();
        catalog.add_source(source("one", "remote"));
        assert!(catalog.generation() > before, "a new source is a change");

        let snapshot = catalog
            .descriptor_snapshot(&CapabilityFilter::default())
            .await
            .unwrap();
        let names: Vec<_> = snapshot
            .tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect();
        assert_eq!(names, vec!["native", "remote"]);
        assert_eq!(snapshot.generation, catalog.generation());
    }

    #[tokio::test]
    async fn a_quiet_refresh_does_not_bump_the_generation() {
        // The generation is what a host compares against the tool list the model
        // holds. Bumping it unconditionally would make that check vacuous: every
        // call would look stale, or (worse) every fresh snapshot would look
        // current because it was rebuilt on the spot.
        let catalog = ToolCatalog::default();
        catalog.add_source(source("one", "remote"));
        let before = catalog.generation();
        catalog.refresh_sources().await.unwrap();
        assert_eq!(catalog.generation(), before);

        // A source that does report a change still bumps it.
        catalog.add_source(Arc::new(ChangingSource));
        let before = catalog.generation();
        catalog.refresh_sources().await.unwrap();
        assert!(catalog.generation() > before);
    }

    #[tokio::test]
    async fn disabled_source_tool_is_absent_from_snapshot() {
        let catalog = ToolCatalog::default();
        catalog.add_source(source("one", "remote"));
        let disabled: crate::capability::CapabilityId = "tool:remote".parse().unwrap();
        let snapshot = catalog
            .descriptor_snapshot(&CapabilityFilter::new([disabled]))
            .await
            .unwrap();
        assert!(snapshot.tools.iter().all(|tool| tool.name != "remote"));
    }

    #[tokio::test]
    async fn handler_for_finds_a_tool_only_a_source_provides() {
        // `get` answers from the native map only; the executor resolves handlers
        // through `handler_for`, so a source tool must be reachable there or a
        // descriptor would exist with no way to run it.
        let catalog = ToolCatalog::default();
        catalog.add_source(source("one", "remote"));
        assert!(catalog.get("remote").is_none());
        assert_eq!(
            catalog
                .handler_for("remote")
                .map(|tool| tool.descriptor().name),
            Some("remote".to_string())
        );
    }

    #[tokio::test]
    async fn handler_for_prefers_the_native_handler() {
        let mut catalog = ToolCatalog::default();
        catalog.insert(Arc::new(FixedTool("same"))).unwrap();
        catalog.add_source(source("one", "same"));
        let handler = catalog.handler_for("same").unwrap();
        assert_eq!(handler.descriptor().name, "same");
    }

    #[tokio::test]
    async fn a_source_added_after_clone_is_visible_to_the_clone() {
        // The executor keeps one clone and the host keeps another; a source
        // registered later must not be invisible to the catalog that dispatches.
        let catalog = ToolCatalog::default();
        let shared = catalog.clone();
        catalog.add_source(source("late", "late_tool"));
        let snapshot = shared
            .descriptor_snapshot(&CapabilityFilter::default())
            .await
            .unwrap();
        assert!(snapshot.tools.iter().any(|tool| tool.name == "late_tool"));
    }

    #[tokio::test]
    async fn clearing_sources_removes_only_the_dynamic_ones() {
        let catalog = ToolCatalog::default();
        let mut with_native = catalog.clone();
        with_native.insert(Arc::new(FixedTool("native"))).unwrap();
        catalog.add_source(source("one", "remote"));
        catalog.clear_sources();
        let snapshot = catalog
            .descriptor_snapshot(&CapabilityFilter::default())
            .await
            .unwrap();
        assert!(snapshot.tools.iter().all(|tool| tool.name != "remote"));
    }

    #[tokio::test]
    async fn duplicate_source_tool_keeps_the_first_source() {
        let catalog = ToolCatalog::default();
        catalog.add_source(source("one", "same"));
        catalog.add_source(source("two", "same"));
        let snapshot = catalog
            .descriptor_snapshot(&CapabilityFilter::default())
            .await
            .unwrap();
        assert_eq!(snapshot.tools.len(), 1);
    }
}

#[cfg(test)]
mod command_tests {
    use super::*;
    use crate::capability::command::{
        Command, CommandCandidate, CommandInvocation, CommandMeta, CommandOutcome, CommandScope,
    };

    struct FixedCommand {
        meta: CommandMeta,
    }

    impl FixedCommand {
        fn new(name: &str, needs_args: bool, aliases: &[&str]) -> Self {
            Self {
                meta: CommandMeta {
                    name: name.to_string(),
                    aliases: aliases.iter().map(|alias| (*alias).to_string()).collect(),
                    summary: format!("{name} summary"),
                    usage: format!("/{name}"),
                    scope: CommandScope::Runtime,
                    needs_args,
                },
            }
        }
    }

    impl Command for FixedCommand {
        fn meta(&self) -> &CommandMeta {
            &self.meta
        }

        fn run(&self, invocation: &CommandInvocation) -> CommandOutcome {
            CommandOutcome::Message(format!("{}:{}", self.meta.name, invocation.args))
        }

        fn complete(&self, args: &str) -> Vec<CommandCandidate> {
            vec![
                CommandCandidate {
                    path: "list".to_string(),
                    detail: "list".to_string(),
                    incomplete: false,
                },
                CommandCandidate {
                    path: format!("get {args}"),
                    detail: "get".to_string(),
                    incomplete: false,
                },
            ]
        }
    }

    fn catalog() -> CommandCatalog {
        let mut catalog = CommandCatalog::default();
        catalog
            .insert(Arc::new(FixedCommand::new("plugin", true, &["p"])))
            .unwrap();
        catalog
            .insert(Arc::new(FixedCommand::new("theme", false, &[])))
            .unwrap();
        catalog
    }

    #[test]
    fn run_dispatches_to_the_owning_command() {
        let outcome = catalog().run(&CommandInvocation {
            name: "plugin".into(),
            args: "list".into(),
        });
        assert_eq!(outcome, CommandOutcome::Message("plugin:list".into()));
    }

    #[test]
    fn run_resolves_an_alias_to_the_same_command() {
        let outcome = catalog().run(&CommandInvocation {
            name: "p".into(),
            args: "list".into(),
        });
        assert_eq!(outcome, CommandOutcome::Message("plugin:list".into()));
    }

    #[test]
    fn an_unknown_command_is_a_usage_line_not_a_panic() {
        let outcome = catalog().run(&CommandInvocation {
            name: "nope".into(),
            args: String::new(),
        });
        assert_eq!(
            outcome,
            CommandOutcome::Usage("unknown command: /nope".into())
        );
    }

    #[test]
    fn an_unwired_command_says_so_instead_of_succeeding() {
        struct DeclaredOnly;
        impl Command for DeclaredOnly {
            fn meta(&self) -> &CommandMeta {
                static META: std::sync::OnceLock<CommandMeta> = std::sync::OnceLock::new();
                META.get_or_init(|| CommandMeta {
                    name: "later".into(),
                    aliases: Vec::new(),
                    summary: "not wired".into(),
                    usage: "/later".into(),
                    scope: CommandScope::Runtime,
                    needs_args: false,
                })
            }
        }
        let mut catalog = CommandCatalog::default();
        catalog.insert(Arc::new(DeclaredOnly)).unwrap();
        let outcome = catalog.run(&CommandInvocation {
            name: "later".into(),
            args: "x".into(),
        });
        assert!(matches!(outcome, CommandOutcome::Usage(_)));
        assert!(format!("{outcome:?}").contains("not runnable yet"));
    }

    #[test]
    fn completion_without_args_lists_matching_names() {
        let candidates = catalog().complete("pl");
        assert_eq!(
            candidates,
            vec![CommandCandidate {
                path: "plugin".into(),
                detail: "plugin summary".into(),
                // `plugin` takes a subcommand, so Enter completes rather than runs.
                incomplete: true,
            }]
        );
    }

    #[test]
    fn completion_with_args_delegates_to_the_command() {
        // A command that takes arguments owns its own grammar; the catalog only
        // prefixes the name back on.
        let candidates = catalog().complete("plugin ");
        let paths: Vec<&str> = candidates.iter().map(|c| c.path.as_str()).collect();
        assert_eq!(paths, vec!["plugin list", "plugin get "]);
    }

    #[test]
    fn completion_of_a_command_without_args_still_lists_names() {
        // `theme` needs no arguments, so trailing space must not switch modes.
        let candidates = catalog().complete("the");
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].path, "theme");
        assert!(!candidates[0].incomplete);
    }

    #[test]
    fn a_typed_alias_completes_to_its_own_subcommands() {
        let candidates = catalog().complete("p ");
        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates[0].path, "p list");
    }
}

#[cfg(test)]
mod async_command_tests {
    use super::*;
    use crate::capability::command::{
        Command, CommandInvocation, CommandMeta, CommandOutcome, CommandScope,
    };
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    /// A command whose answer can only be produced by awaiting.
    struct Async {
        meta: CommandMeta,
        ran: Arc<AtomicBool>,
    }

    impl Async {
        fn new(ran: Arc<AtomicBool>) -> Self {
            Self {
                meta: CommandMeta {
                    name: "gated".into(),
                    aliases: Vec::new(),
                    summary: "needs to await".into(),
                    usage: "/gated".into(),
                    scope: CommandScope::Runtime,
                    needs_args: false,
                },
                ran,
            }
        }
    }

    #[async_trait::async_trait]
    impl Command for Async {
        fn meta(&self) -> &CommandMeta {
            &self.meta
        }

        async fn run_async(&self, _invocation: &CommandInvocation) -> CommandOutcome {
            self.ran.store(true, Ordering::SeqCst);
            CommandOutcome::Message("awaited".into())
        }
    }

    #[tokio::test]
    async fn a_command_can_answer_without_blocking_the_host() {
        let ran = Arc::new(AtomicBool::new(false));
        let mut catalog = CommandCatalog::default();
        catalog.insert(Arc::new(Async::new(ran.clone()))).unwrap();
        let outcome = catalog
            .run_async(&CommandInvocation {
                name: "gated".into(),
                args: String::new(),
            })
            .await;
        assert_eq!(outcome, CommandOutcome::Message("awaited".into()));
        assert!(ran.load(Ordering::SeqCst), "the async body ran");
    }

    /// A command that implements only the sync half.
    struct SyncOnly {
        meta: CommandMeta,
    }

    impl Command for SyncOnly {
        fn meta(&self) -> &CommandMeta {
            &self.meta
        }

        fn run(&self, _invocation: &CommandInvocation) -> CommandOutcome {
            CommandOutcome::Message("sync".into())
        }
    }

    #[tokio::test]
    async fn run_async_still_reaches_a_sync_command() {
        // A pure command implements only `run`; the async route must not make
        // the caller special-case it.
        let mut catalog = CommandCatalog::default();
        catalog
            .insert(Arc::new(SyncOnly {
                meta: CommandMeta {
                    name: "plain".into(),
                    aliases: Vec::new(),
                    summary: String::new(),
                    usage: "/plain".into(),
                    scope: CommandScope::Runtime,
                    needs_args: false,
                },
            }))
            .unwrap();
        let outcome = catalog
            .run_async(&CommandInvocation {
                name: "plain".into(),
                args: String::new(),
            })
            .await;
        assert!(
            !matches!(outcome, CommandOutcome::Usage(_)),
            "a sync command must answer through the async route too: {outcome:?}"
        );
    }

    #[tokio::test]
    async fn an_unknown_command_is_a_usage_error_on_the_async_route() {
        let catalog = CommandCatalog::default();
        let outcome = catalog
            .run_async(&CommandInvocation {
                name: "nope".into(),
                args: String::new(),
            })
            .await;
        assert!(matches!(outcome, CommandOutcome::Usage(_)), "{outcome:?}");
    }
}
