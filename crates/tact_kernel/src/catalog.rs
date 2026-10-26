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
    Command, CommandCandidate, CommandInvocation, CommandMeta, CommandOutcome, CommandSource,
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
    /// Commands discovered at runtime (MCP prompts). Behind a lock and an `Arc`
    /// so a host can add one through a shared catalog without rebuilding it and
    /// invalidating every name a caller already resolved.
    sources: Arc<std::sync::Mutex<Vec<Arc<dyn CommandSource>>>>,
    generation: Arc<std::sync::atomic::AtomicU64>,
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
        self.bump();
        Ok(())
    }

    /// Merge another catalog's native commands into this one.
    ///
    /// Used by a host that composes a catalog from two sources: the plugins'
    /// contributions (already sealed into a `CommandCatalog`) and the commands
    /// assembled from live runtime state. Duplicate names are an error rather
    /// than a silent shadow, because a shadowed `/permission` is a bug nobody
    /// would see.
    pub fn extend_from(&mut self, other: &Self) -> anyhow::Result<()> {
        for command in other.commands.values() {
            self.insert(command.clone())?;
        }
        Ok(())
    }

    /// Resolve a command, native first.
    ///
    /// A native command shadows a source command of the same name, matching
    /// [`Self::descriptors`] so the advertised row and the dispatched handler can
    /// never disagree — and so an MCP prompt cannot shadow `/stats`.
    ///
    /// Returns an owned handle rather than a reference because a source command
    /// does not live in a map this catalog can borrow from.
    pub fn get(&self, name: &str) -> Option<Arc<dyn Command>> {
        let name = self.aliases.get(name).map_or(name, String::as_str);
        if let Some(command) = self.commands.get(name) {
            return Some(command.clone());
        }
        self.source_commands()
            .into_iter()
            .find(|command| command.meta().name == name)
    }

    /// Register a dynamic source.
    ///
    /// Takes `&self` so a host can add one after assembly, through the shared
    /// catalog the driver already holds: MCP prompts are only known once a server
    /// has listed them, which is long after the host sealed.
    pub fn add_source(&self, source: Arc<dyn CommandSource>) {
        let mut sources = self
            .sources
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        sources.push(source);
        self.bump();
    }

    /// Drop every dynamic source. Used when a host replaces its MCP router.
    pub fn clear_sources(&self) {
        self.sources
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clear();
        self.bump();
    }

    /// Re-read every source, reporting whether any of them changed.
    ///
    /// The boolean is what keeps the generation honest: a source that re-lists
    /// nothing new must not bump it, or every caller would look "current" because
    /// the snapshot it compares against was rebuilt on the spot.
    pub async fn refresh_sources(&self) -> anyhow::Result<bool> {
        let sources = self.snapshot_sources();
        let mut changed = false;
        for source in &sources {
            changed |= source.refresh().await?;
        }
        if changed {
            self.bump();
        }
        Ok(changed)
    }

    /// The current generation, without rebuilding anything.
    pub fn generation(&self) -> u64 {
        self.generation.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn bump(&self) {
        self.generation
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }

    /// Cloned out of the lock, because `refresh` is async and may re-enter.
    fn snapshot_sources(&self) -> Vec<Arc<dyn CommandSource>> {
        self.sources
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }

    /// Every source-contributed command, without deduplicating against natives.
    fn source_commands(&self) -> Vec<Arc<dyn Command>> {
        self.snapshot_sources()
            .iter()
            .flat_map(|source| source.commands())
            .collect()
    }

    /// Every command's metadata, sorted by name.
    ///
    /// Sorted rather than insertion-ordered so a picker's list is stable across
    /// runs: a palette that reshuffles when a plugin loads makes the same
    /// keystrokes land on a different command.
    pub fn descriptors(&self) -> Vec<CommandMeta> {
        self.descriptors_filtered(&CapabilityFilter::default())
    }

    /// The same listing, minus the commands the filter denies.
    ///
    /// The filter is applied here rather than at registration, because a source's
    /// set changes at runtime: a prompt that appears after the host sealed must
    /// still be filtered on every listing.
    pub fn descriptors_filtered(&self, filter: &CapabilityFilter) -> Vec<CommandMeta> {
        let mut by_name: BTreeMap<String, CommandMeta> = BTreeMap::new();
        for command in self.commands.values() {
            by_name.insert(command.meta().name.clone(), command.meta().clone());
        }
        // Sources never shadow a native: `get` prefers the map, so the listing
        // must too, or a palette would offer a row that dispatches elsewhere.
        for command in self.source_commands() {
            by_name
                .entry(command.meta().name.clone())
                .or_insert_with(|| command.meta().clone());
        }
        by_name
            .into_values()
            .filter(|meta| {
                filter.allows(&CapabilityId::new(
                    CapabilityKind::Command,
                    meta.name.clone(),
                ))
            })
            .collect()
    }

    /// Whether `name` resolves to a command the filter allows.
    ///
    /// The listing is filtered by exactly this rule, so an advertised row is
    /// always dispatchable and vice versa.
    pub fn can_dispatch(&self, name: &str, filter: &CapabilityFilter) -> bool {
        filter.allows(&CapabilityId::new(
            CapabilityKind::Command,
            name.to_string(),
        )) && self.get(name).is_some()
    }

    pub fn len(&self) -> usize {
        self.descriptors().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
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

    /// Every installed view id, in name order.
    ///
    /// Exposed so a composition root can assert exactly one view is attached: two
    /// views in one process is a wiring mistake, not a feature — the same `App`
    /// cannot be drawn twice.
    pub fn ids(&self) -> Vec<&'static str> {
        self.views.keys().copied().collect()
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

    /// A source that contributes one command and reports no change on refresh.
    struct OneCommandSource {
        id: &'static str,
        command: Arc<dyn Command>,
    }

    #[async_trait::async_trait]
    impl CommandSource for OneCommandSource {
        fn id(&self) -> &str {
            self.id
        }

        async fn refresh(&self) -> anyhow::Result<bool> {
            Ok(false)
        }

        fn commands(&self) -> Vec<Arc<dyn Command>> {
            vec![self.command.clone()]
        }
    }

    #[test]
    fn a_source_contributed_command_is_listed_and_resolves() {
        let mut catalog = CommandCatalog::default();
        catalog
            .insert(Arc::new(FixedCommand::new("native", &[])))
            .unwrap();
        catalog.add_source(Arc::new(OneCommandSource {
            id: "mcp",
            command: Arc::new(FixedCommand::new("remote", &[])),
        }));

        // Named resolution reaches through the source, not just the native map.
        assert!(
            catalog.get("remote").is_some(),
            "a source command must resolve by name"
        );
        // The listing carries both, so a palette shows what can actually run.
        let names: Vec<String> = catalog.descriptors().into_iter().map(|m| m.name).collect();
        assert_eq!(names, vec!["native".to_string(), "remote".to_string()]);
    }

    #[test]
    fn a_native_command_wins_over_a_source_of_the_same_name() {
        // Same rule as `ToolCatalog::handler_for`: a server must not shadow a
        // built-in, and the listing must agree with what dispatch would pick.
        let mut catalog = CommandCatalog::default();
        catalog
            .insert(Arc::new(FixedCommand::new("stats", &[])))
            .unwrap();
        catalog.add_source(Arc::new(OneCommandSource {
            id: "mcp",
            command: Arc::new(FixedCommand::new("stats", &[])),
        }));

        let listed: Vec<String> = catalog.descriptors().into_iter().map(|m| m.name).collect();
        assert_eq!(listed, vec!["stats".to_string()], "no duplicate row");
        assert_eq!(
            catalog.get("stats").expect("native wins").meta().scope,
            CommandScope::Runtime
        );
    }

    #[test]
    fn a_filtered_listing_excludes_a_source_command() {
        // No `mut`: `add_source` takes `&self`, which is what lets a host add a
        // source after it sealed.
        let catalog = CommandCatalog::default();
        catalog.add_source(Arc::new(OneCommandSource {
            id: "mcp",
            command: Arc::new(FixedCommand::new("remote", &[])),
        }));

        let filter = CapabilityFilter::new([CapabilityId::new(
            CapabilityKind::Command,
            "remote".to_string(),
        )]);
        assert!(
            catalog.descriptors_filtered(&filter).is_empty(),
            "a disabled command must not be advertised"
        );
        assert!(
            !catalog.can_dispatch("remote", &filter),
            "and it must not dispatch either"
        );
    }

    #[tokio::test]
    async fn refreshing_sources_reports_whether_anything_changed() {
        let catalog = CommandCatalog::default();
        catalog.add_source(Arc::new(OneCommandSource {
            id: "mcp",
            command: Arc::new(FixedCommand::new("remote", &[])),
        }));
        let before = catalog.generation();
        // This source reports `false`, so the generation must not move: a host
        // comparing generations would otherwise think a stale list was current.
        assert!(
            !catalog
                .refresh_sources()
                .await
                .expect("refresh is infallible here")
        );
        assert_eq!(catalog.generation(), before);
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

    /// Every command is listed, with the metadata it owns.
    ///
    /// This is what a picker renders; a command the catalog can route but not
    /// list is unreachable for a user who does not already know its name.
    #[test]
    fn descriptors_list_every_command_with_its_own_metadata() {
        let mut catalog = CommandCatalog::default();
        catalog
            .insert(Arc::new(FixedCommand::new("beta", &[])))
            .unwrap();
        catalog
            .insert(Arc::new(FixedCommand::new("alpha", &["a"])))
            .unwrap();

        let listed = catalog.descriptors();
        assert_eq!(
            listed
                .iter()
                .map(|meta| meta.name.as_str())
                .collect::<Vec<_>>(),
            vec!["alpha", "beta"],
            "sorted by name, so the list is stable across runs"
        );
        // Each entry is the owning command's own metadata, aliases included:
        // a picker shows what the command declares, not what the catalog
        // reconstructs from the name.
        assert_eq!(listed[0].aliases, vec!["a".to_string()]);
        assert!(listed[1].aliases.is_empty());
    }

    /// A command whose registration was rejected is not listed.
    ///
    /// The catalog is transactional: a failed insert must leave no trace, or a
    /// picker would offer a command that cannot run.
    #[test]
    fn a_rejected_command_is_not_listed() {
        let mut catalog = CommandCatalog::default();
        catalog
            .insert(Arc::new(FixedCommand::new("real", &[])))
            .unwrap();
        let _ = catalog.insert(Arc::new(FixedCommand::new("real", &[])));
        assert_eq!(catalog.descriptors().len(), 1);
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
        assert!(
            matches!(&outcome, CommandOutcome::Message(text) if text == "plugin:list"),
            "{outcome:?}"
        );
    }

    #[test]
    fn run_resolves_an_alias_to_the_same_command() {
        let outcome = catalog().run(&CommandInvocation {
            name: "p".into(),
            args: "list".into(),
        });
        assert!(
            matches!(&outcome, CommandOutcome::Message(text) if text == "plugin:list"),
            "{outcome:?}"
        );
    }

    #[test]
    fn an_unknown_command_is_a_usage_line_not_a_panic() {
        let outcome = catalog().run(&CommandInvocation {
            name: "nope".into(),
            args: String::new(),
        });
        assert!(
            matches!(&outcome, CommandOutcome::Usage(text)
                if text == "unknown command: /nope"),
            "{outcome:?}"
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
        assert!(
            matches!(&outcome, CommandOutcome::Message(text) if text == "awaited"),
            "{outcome:?}"
        );
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
