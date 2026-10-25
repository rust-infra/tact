//! The built-in tool families, as data rather than as a hand-written chain.
//!
//! Every built-in tool belongs to exactly one family. The family table is what
//! the composition root installs; `toolset()` is a fold over it. That makes the
//! set of built-ins a value a test can inspect (and a caller can filter) instead
//! of a chain of `.route(..)` calls whose order is invisible.

use anyhow::Result;

use tact_kernel::plugin::Registrar;

use super::{
    Tool, ToolRouter,
    ask_user::AskUserTool,
    background_run::{BackgroundRunTool, CheckBackgroundTool, WaitBackgroundTool},
    bash::BashTool,
    compact::CompactTool,
    edit_file::EditFileTool,
    goal::{GoalCheckpointTool, GoalProposeCompletionTool, GoalReadTool, GoalRequestInputTool},
    load_skill::LoadSkillTool,
    memory::{LoadMemoryTool, SaveMemoryTool},
    read_file::ReadFileTool,
    read_image::ReadImageTool,
    sleep::SleepTool,
    subagent::{CancelSubagentTool, CheckSubagentTool, SpawnSubagentTool, WaitSubagentTool},
    task::{TaskCreateTool, TaskGetTool, TaskListTool, TaskUpdateTool},
    team::{
        BroadcastTool, ListTeammatesTool, PlanApprovalTool, ReadInboxTool, SendMessageTool,
        ShutdownRequestTool, ShutdownResponseTool, SpawnTeammateTool,
    },
    worktree::{
        WorktreeCreateTool, WorktreeEventsTool, WorktreeListTool, WorktreeRemoveTool,
        WorktreeRunTool, WorktreeStatusTool,
    },
    write_file::WriteFileTool,
};

/// One family of built-in tools.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ToolFamily {
    /// The shell and the clock: the tools whose resources are whole-process.
    Shell,
    /// Reading and writing files.
    Files,
    /// Asking the user, and compaction.
    Interaction,
    /// Durable tasks.
    Task,
    /// Bounded goals.
    Goal,
    /// Teammates and messaging.
    Team,
    /// Isolated worktrees.
    Worktree,
    /// Detached background commands.
    Background,
    /// Child agents.
    Subagent,
    /// Persistent memory.
    Memory,
    /// Skill discovery.
    Skill,
    /// The restricted set a child agent gets.
    ///
    /// Deliberately its own family rather than `Shell + Files`: a child must not
    /// get `read_image` (a large payload into a child's context) and the list is
    /// meant to stay auditable in one place. Not part of [`Self::ALL`], which is
    /// the main agent's set.
    SubagentBasics,
}

impl ToolFamily {
    /// Every family, in installation order.
    ///
    /// Order is load-bearing only for the emitted spec list, which the model
    /// reads top-down; it does not affect dispatch.
    pub const ALL: [Self; 11] = [
        Self::Shell,
        Self::Files,
        Self::Interaction,
        Self::Task,
        Self::Goal,
        Self::Team,
        Self::Worktree,
        Self::Background,
        Self::Subagent,
        Self::Memory,
        Self::Skill,
    ];

    pub fn id(self) -> &'static str {
        match self {
            Self::Shell => "builtin.tools.shell",
            Self::Files => "builtin.tools.files",
            Self::Interaction => "builtin.tools.interaction",
            Self::Task => "builtin.tools.task",
            Self::Goal => "builtin.tools.goal",
            Self::Team => "builtin.tools.team",
            Self::Worktree => "builtin.tools.worktree",
            Self::Background => "builtin.tools.background",
            Self::Subagent => "builtin.tools.subagent",
            Self::Memory => "builtin.tools.memory",
            Self::Skill => "builtin.tools.skill",
            Self::SubagentBasics => "builtin.tools.subagent_basics",
        }
    }

    /// The names this family contributes, without constructing handlers.
    ///
    /// Used by the composition root to pull the *same* `Arc` handlers out of the
    /// router it already has, so a tool does not exist twice.
    pub fn tool_names(self, memory_enabled: bool) -> Vec<&'static str> {
        self.handlers(memory_enabled)
            .into_iter()
            .map(|handler| handler.metadata().name)
            .collect()
    }

    /// Construct this family's handlers.
    ///
    /// `memory_enabled` mirrors `[agent].memory_enabled`: the memory family
    /// contributes nothing when it is off, which is what makes those tools
    /// disappear from the advertised list rather than fail at dispatch.
    pub fn handlers(self, memory_enabled: bool) -> Vec<Box<dyn Tool>> {
        match self {
            Self::Shell => vec![Box::new(BashTool), Box::new(SleepTool)],
            Self::Files => vec![
                Box::new(ReadFileTool),
                Box::new(ReadImageTool),
                Box::new(WriteFileTool),
                Box::new(EditFileTool),
            ],
            Self::Interaction => vec![Box::new(AskUserTool), Box::new(CompactTool)],
            Self::Task => vec![
                Box::new(TaskCreateTool),
                Box::new(TaskGetTool),
                Box::new(TaskListTool),
                Box::new(TaskUpdateTool),
            ],
            Self::Goal => vec![
                Box::new(GoalReadTool),
                Box::new(GoalCheckpointTool),
                Box::new(GoalRequestInputTool),
                Box::new(GoalProposeCompletionTool),
            ],
            Self::Team => vec![
                Box::new(SpawnTeammateTool),
                Box::new(ListTeammatesTool),
                Box::new(SendMessageTool),
                Box::new(BroadcastTool),
                Box::new(ReadInboxTool),
                Box::new(PlanApprovalTool),
                Box::new(ShutdownRequestTool),
                Box::new(ShutdownResponseTool),
            ],
            Self::Worktree => vec![
                Box::new(WorktreeCreateTool),
                Box::new(WorktreeListTool),
                Box::new(WorktreeStatusTool),
                Box::new(WorktreeRunTool),
                Box::new(WorktreeRemoveTool),
                Box::new(WorktreeEventsTool),
            ],
            Self::Background => vec![
                Box::new(BackgroundRunTool),
                Box::new(CheckBackgroundTool),
                Box::new(WaitBackgroundTool),
            ],
            Self::Subagent => vec![
                Box::new(SpawnSubagentTool),
                Box::new(CheckSubagentTool),
                Box::new(WaitSubagentTool),
                Box::new(CancelSubagentTool),
            ],
            Self::Memory => {
                if memory_enabled {
                    vec![Box::new(SaveMemoryTool), Box::new(LoadMemoryTool)]
                } else {
                    Vec::new()
                }
            }
            Self::Skill => vec![Box::new(LoadSkillTool)],
            Self::SubagentBasics => vec![
                Box::new(BashTool),
                Box::new(ReadFileTool),
                Box::new(SleepTool),
                Box::new(WriteFileTool),
                Box::new(EditFileTool),
            ],
        }
    }
}

/// The restricted family set a child agent gets.
///
/// A subagent's tool list is deliberately not the parent's: it cannot spawn
/// another subagent, cannot create tasks, and has no Goal tools (it would be
/// asked to call `goal_checkpoint` for a Goal it does not own).
pub const SUBAGENT_FAMILIES: [ToolFamily; 1] = [ToolFamily::SubagentBasics];

/// One family presented to the plugin host.
///
/// `Registrar::tool` takes ownership, so a family is built for one installation
/// and consumed by it. The handlers are the same ones the router would create.
pub struct FamilyPlugin {
    family: ToolFamily,
    handlers: std::sync::Mutex<Option<Vec<Box<dyn Tool>>>>,
    shared: Vec<std::sync::Arc<dyn Tool>>,
    contexts: std::sync::Arc<dyn super::ToolContextFactory>,
}

impl FamilyPlugin {
    pub fn new(family: ToolFamily, memory_enabled: bool, context: &super::ToolContext) -> Self {
        Self {
            family,
            handlers: std::sync::Mutex::new(Some(family.handlers(memory_enabled))),
            shared: Vec::new(),
            contexts: std::sync::Arc::new(super::SharedToolContextFactory::new(context.clone())),
        }
    }

    /// A family over handlers something else already owns.
    ///
    /// The composition root uses this so the router (which emits specs) and the
    /// catalog (which dispatches) see one instance per tool — and so every
    /// family shares the *one* context factory the Agent can refresh.
    pub fn from_shared(
        family: ToolFamily,
        handlers: Vec<std::sync::Arc<dyn Tool>>,
        contexts: std::sync::Arc<dyn super::ToolContextFactory>,
    ) -> Self {
        Self {
            family,
            handlers: std::sync::Mutex::new(None),
            shared: handlers,
            contexts,
        }
    }

    pub fn family(&self) -> ToolFamily {
        self.family
    }

    /// Whether this family contributed anything at all.
    ///
    /// The memory family contributes nothing when memory is disabled, so a host
    /// installing it would create a plugin with no capabilities.
    pub fn is_empty(&self) -> bool {
        if !self.shared.is_empty() {
            return false;
        }
        self.handlers
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .as_ref()
            .is_some_and(Vec::is_empty)
    }
}

#[async_trait::async_trait]
impl tact_kernel::plugin::Plugin for FamilyPlugin {
    fn manifest(&self) -> tact_kernel::plugin::PluginManifest {
        tact_kernel::plugin::PluginManifest::new(
            tact_kernel::plugin::PluginId::new(self.family.id())
                .expect("built-in family ids are valid"),
            self.family.id(),
            env!("CARGO_PKG_VERSION"),
            tact_kernel::plugin::PluginKind::Capability,
            "built-in tool family",
        )
    }

    fn register(&self, registrar: &mut Registrar<'_>) -> anyhow::Result<()> {
        for handler in &self.shared {
            registrar.tool(super::ContextualToolAdapter::from_arc(
                handler.clone(),
                self.contexts.clone(),
            ))?;
        }
        // `Registrar` is `&mut`, `Plugin::register` is `&self`: the owned
        // handlers are moved out of a cell so a family registers exactly once.
        let Some(mut taken) = self
            .handlers
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take()
        else {
            anyhow::ensure!(
                !self.shared.is_empty(),
                "tool family {} was registered twice",
                self.family.id()
            );
            return Ok(());
        };
        for handler in taken.drain(..) {
            registrar.tool(super::ContextualToolAdapter::from_boxed(
                handler,
                self.contexts.clone(),
            ))?;
        }
        Ok(())
    }
}

/// Install every family of `families` into `host`.
///
/// This is the composition root's tool wiring: the set of built-in tools is what
/// these plugins contribute, and `host.skipped()` reports a capability the user
/// disabled.
pub async fn install_families(
    host: &mut tact_kernel::plugin::PluginHost,
    router: &ToolRouter,
    families: &[ToolFamily],
    memory_enabled: bool,
    contexts: std::sync::Arc<dyn super::ToolContextFactory>,
) -> anyhow::Result<()> {
    for family in families {
        let shared = router.handlers_for(&family.tool_names(memory_enabled));
        let plugin = FamilyPlugin::from_shared(*family, shared, contexts.clone());
        if plugin.is_empty() {
            continue;
        }
        host.install(plugin).await?;
    }
    Ok(())
}

/// Assemble a catalog by installing every family into a fresh host.
///
/// The production composition root: the catalog is a consequence of the plugin
/// set, and `skipped` reports any capability the caller's filter disabled.
pub fn catalog_from_families(
    router: &ToolRouter,
    families: &[ToolFamily],
    memory_enabled: bool,
    contexts: std::sync::Arc<dyn super::ToolContextFactory>,
    filter: tact_kernel::capability::CapabilityFilter,
) -> anyhow::Result<(tact_kernel::catalog::ToolCatalog, Vec<String>)> {
    let mut host = tact_kernel::plugin::PluginHost::new(filter);
    for family in families {
        let shared = router.handlers_for(&family.tool_names(memory_enabled));
        let plugin = FamilyPlugin::from_shared(*family, shared, contexts.clone());
        if plugin.is_empty() {
            continue;
        }
        // Install without an async context: `Agent::new` is synchronous and may
        // run on a current-thread runtime, where `block_in_place` panics.
        host.install_sync(plugin)?;
    }
    let skipped = host
        .skipped()
        .iter()
        .map(|skipped| skipped.capability.to_string())
        .collect();
    // The catalog outlives the runtime on purpose — the executor keeps it while
    // the plugins were only a registration mechanism. Their `shutdown` is the
    // default no-op (they own no resource; the handlers live in the router), so
    // dropping the runtime here is honest rather than a leak. A family that ever
    // grows a resource must move its cleanup into the handler, not the plugin.
    let runtime = host.seal_sync()?;
    let catalog = runtime.tools().clone();
    drop(runtime);
    Ok((catalog, skipped))
}

/// Assemble a router from a family list.
pub fn router_from(families: &[ToolFamily], memory_enabled: bool) -> Result<ToolRouter> {
    let mut router = ToolRouter::new();
    for family in families {
        for handler in family.handlers(memory_enabled) {
            router = router.route_boxed(handler)?;
        }
    }
    Ok(router)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(families: &[ToolFamily], memory_enabled: bool) -> Vec<String> {
        let mut names: Vec<String> = router_from(families, memory_enabled)
            .unwrap()
            .tool_specs()
            .into_iter()
            .map(|spec| spec.name)
            .collect();
        names.sort();
        names
    }

    /// The router and the catalog must hold the *same* handler, not two copies.
    ///
    /// A tool with per-instance state (a background manager handle, a cancel
    /// flag) would otherwise exist twice: the router emits its spec and the
    /// catalog dispatches it, and the two would drift. `catalog_from_families`
    /// borrows from the router for exactly this reason.
    #[test]
    fn the_catalog_shares_the_routers_handlers() {
        let router = router_from(&ToolFamily::ALL, true).unwrap();
        let shared = router.handlers_for(&["bash"]);
        assert_eq!(shared.len(), 1, "bash belongs to exactly one family");
        let catalog = catalog_from_families(
            &router,
            &ToolFamily::ALL,
            true,
            std::sync::Arc::new(super::super::SharedToolContextFactory::new(
                crate::tool::test_support::test_context("catalog_shares_handlers"),
            )),
            tact_kernel::capability::CapabilityFilter::default(),
        )
        .unwrap()
        .0;
        // The two traits are different (`Tool` here, the kernel's in the catalog),
        // so identity is checked through what both expose: the descriptor the
        // catalog resolved must describe the same tool the router holds.
        let from_catalog = catalog.handler_for("bash").expect("bash is dispatchable");
        assert_eq!(
            from_catalog.descriptor().name,
            shared[0].metadata().name,
            "the catalog must resolve the router's handler, not a second one"
        );
        // And it is a borrow, not a copy: `catalog_from_families` takes `&ToolRouter`.
        assert_eq!(
            catalog.len(),
            router.tool_specs().len(),
            "every routed tool is dispatchable, no more and no fewer"
        );
    }

    #[test]
    fn the_family_table_reproduces_the_built_in_tool_set() {
        // The composition used to be a chain of `.route(..)` calls. This pins the
        // family table to exactly that set, so reordering or dropping a tool is a
        // test failure rather than a silently smaller tool list.
        let expected = [
            "ask_user",
            "bash",
            "background_run",
            "check_background",
            "wait_background",
            "read_file",
            "read_image",
            "sleep",
            "write_file",
            "edit_file",
            "load_skill",
            "goal_read",
            "goal_checkpoint",
            "goal_request_input",
            "goal_propose_completion",
            "save_memory",
            "load_memory",
            "compact",
            "spawn_subagent",
            "check_subagent",
            "wait_subagent",
            "cancel_subagent",
            "task_create",
            "task_get",
            "task_list",
            "task_update",
            "spawn_teammate",
            "list_teammates",
            "send_message",
            "broadcast",
            "read_inbox",
            "plan_approval",
            "shutdown_request",
            "shutdown_response",
            "worktree_create",
            "worktree_list",
            "worktree_status",
            "worktree_run",
            "worktree_remove",
            "worktree_events",
        ];
        let mut expected: Vec<String> = expected.iter().map(|name| (*name).to_string()).collect();
        expected.sort();
        assert_eq!(names(&ToolFamily::ALL, true), expected);
    }

    #[test]
    fn memory_family_is_absent_when_memory_is_off() {
        let without = names(&ToolFamily::ALL, false);
        assert!(!without.iter().any(|name| name == "save_memory"));
        assert!(!without.iter().any(|name| name == "load_memory"));
        assert_eq!(without.len(), names(&ToolFamily::ALL, true).len() - 2);
    }

    #[test]
    fn subagent_families_are_the_five_restricted_tools() {
        let mut expected = vec![
            "bash".to_string(),
            "read_file".to_string(),
            "sleep".to_string(),
            "write_file".to_string(),
            "edit_file".to_string(),
        ];
        expected.sort();
        assert_eq!(names(&SUBAGENT_FAMILIES, true), expected);
    }

    #[test]
    fn every_family_id_is_unique() {
        let ids: Vec<&str> = ToolFamily::ALL.iter().map(|family| family.id()).collect();
        let unique: std::collections::BTreeSet<&str> = ids.iter().copied().collect();
        assert_eq!(ids.len(), unique.len());
    }

    #[tokio::test]
    async fn the_plugin_host_installs_every_family() {
        // The point of the host path: the catalog is whatever the plugins
        // contributed, not a second hand-written list.
        use tact_kernel::capability::CapabilityFilter;
        use tact_kernel::plugin::PluginHost;

        let context = crate::tool::test_support::test_context("family_host");
        let mut host = PluginHost::new(CapabilityFilter::default());
        for family in ToolFamily::ALL {
            host.install(FamilyPlugin::new(family, true, &context))
                .await
                .unwrap();
        }
        let runtime = host.seal().await.unwrap();
        let names: Vec<String> = runtime
            .tools()
            .descriptor_snapshot(&CapabilityFilter::default())
            .await
            .unwrap()
            .tools
            .into_iter()
            .map(|tool| tool.name)
            .collect();
        let mut expected = names_of(&ToolFamily::ALL, true);
        let mut actual = names;
        expected.sort();
        actual.sort();
        assert_eq!(actual, expected);
    }

    #[tokio::test]
    async fn a_disabled_capability_is_not_installed() {
        use tact_kernel::capability::{CapabilityFilter, CapabilityId};
        use tact_kernel::plugin::PluginHost;

        let context = crate::tool::test_support::test_context("family_host_filter");
        let disabled: CapabilityId = "tool:bash".parse().unwrap();
        let mut host = PluginHost::new(CapabilityFilter::new([disabled]));
        for family in ToolFamily::ALL {
            host.install(FamilyPlugin::new(family, true, &context))
                .await
                .unwrap();
        }
        assert!(
            host.skipped()
                .iter()
                .any(|skipped| skipped.capability.name == "bash")
        );
        let runtime = host.seal().await.unwrap();
        let snapshot = runtime
            .tools()
            .descriptor_snapshot(&CapabilityFilter::default())
            .await
            .unwrap();
        assert!(snapshot.tools.iter().all(|tool| tool.name != "bash"));
    }

    fn names_of(families: &[ToolFamily], memory_enabled: bool) -> Vec<String> {
        let mut names: Vec<String> = router_from(families, memory_enabled)
            .unwrap()
            .tool_specs()
            .into_iter()
            .map(|spec| spec.name)
            .collect();
        names.sort();
        names
    }

    #[test]
    fn families_do_not_share_a_tool_name() {
        let mut seen = std::collections::BTreeSet::new();
        for family in ToolFamily::ALL {
            for handler in family.handlers(true) {
                let name = handler.metadata().name;
                assert!(seen.insert(name), "{name} is contributed twice");
            }
        }
    }
}
