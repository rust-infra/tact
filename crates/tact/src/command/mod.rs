//! Built-in slash commands as catalog contributions.
//!
//! A command used to live wherever its handler happened to be: `/stats` in the
//! driver, `/model` in the TUI, each with its own match arm and its own idea of
//! what the arguments mean. Here a command is one type that owns its metadata,
//! its argument grammar and its answer, and the kernel's `CommandCatalog` routes
//! it. The host still renders the answer — a `CommandOutcome::Message` is text,
//! not a popup — so the command does not need to know which frontend called it.
//!
//! Only *host-side* commands live here (a snapshot of local state, a settings
//! query). A command that needs to drive the agent loop stays on the driver's
//! side until the loop itself is a plugin, because a command cannot own a
//! `&mut Agent` through the catalog.

use std::sync::Arc;

use tact_kernel::capability::command::CommandMeta;

pub use tact_kernel::capability::command::Command;

pub mod background;
pub mod compact;
pub mod goal;
pub mod hooks;
pub mod mcp;
pub mod permission;
pub mod plugin;
pub mod settings;
pub mod stats;

pub use background::BackgroundCommand;
pub use goal::GoalCommand;
pub use plugin::CommandPlugin;
pub use stats::StatsCommand;

pub use tact_kernel::capability::command::{CommandInvocation, CommandOutcome, Emitted};
pub use tact_kernel::catalog::CommandCatalog;

/// A command whose metadata is fixed and whose body is a closure.
///
/// The built-ins are all of this shape: a name, an alias list, and one function
/// over the arguments. Named `FnCommand` rather than `SimpleCommand` because
/// what it is *is* a function.
pub struct FnCommand {
    meta: CommandMeta,
    run: Arc<dyn Fn(&str) -> CommandOutcome + Send + Sync>,
}

impl FnCommand {
    pub fn new(
        meta: CommandMeta,
        run: impl Fn(&str) -> CommandOutcome + Send + Sync + 'static,
    ) -> Self {
        Self {
            meta,
            run: Arc::new(run),
        }
    }
}

impl Command for FnCommand {
    fn meta(&self) -> &CommandMeta {
        &self.meta
    }

    fn run(&self, invocation: &CommandInvocation) -> CommandOutcome {
        (self.run)(&invocation.args)
    }
}

/// Every host-side command this crate contributes.
///
/// A function, not a static table: the commands take state (the stats handle),
/// so they are built where that state is.
pub fn builtin_commands(
    stats: Arc<std::sync::RwLock<crate::stats::SessionStats>>,
) -> Vec<Arc<dyn Command>> {
    vec![Arc::new(StatsCommand::new(stats))]
}

/// The catalog a host serves, with every built-in command inserted.
///
/// Returned as a kernel type so a frontend does not have to depend on the kernel
/// to hold one. Duplicate names are a programming error and panic: two built-ins
/// sharing a name would be silently shadowed, which is worse than a startup
/// crash in a test.
/// The turn lifecycle as a catalog command sees it.
///
/// Re-exported so a frontend can pass one in without depending on the kernel
/// directly — the same reason `builtin_catalog` returns a kernel type.
pub use tact_kernel::runtime::ChatService;

/// What the MCP commands need from the host.
///
/// A narrow read-only view rather than the router itself: `/mcp list` and
/// `/mcp prompts` only ever read, and handing a command the whole router would
/// let it reconnect or dispatch tools, which is not what a listing does.
pub trait McpCatalog: Send + Sync {
    /// The connected servers and their tool counts.
    fn server_summaries(&self) -> Vec<(String, usize)>;

    /// Describe every configured server.
    ///
    /// On the port rather than resolved inside the command, because resolution
    /// needs a working directory and the process's is a global: a caller that
    /// captured its own must not have a later `cd` change the answer.
    fn describe(
        &self,
        connected: &[(String, usize)],
    ) -> anyhow::Result<Vec<crate::mcp::McpServerView>>;

    /// A rendered `prompts/list`, optionally scoped to one server.
    ///
    /// A boxed future rather than `async fn` so an implementor does not need the
    /// `async_trait` macro — the only implementation lives in a frontend crate
    /// that has no reason to depend on it.
    fn list_prompts<'a>(
        &'a self,
        server: Option<&'a str>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<String>> + Send + 'a>>;
}

/// What `/mcp` needs from the host.
pub struct McpWiring {
    /// The read-only view of the connected servers.
    pub catalog: Arc<dyn McpCatalog>,
    /// How a server listing becomes text.
    pub render: mcp::McpRenderer,
}

/// What `/hooks` needs from the host.
pub struct HooksWiring {
    /// The project directory whose hooks are inspected.
    pub work_dir: std::path::PathBuf,
    /// How a listing becomes text; the renderer lives with the other CLI
    /// surfaces, so the host supplies it rather than the command reaching for it.
    pub render: hooks::HooksRenderer,
}

/// The optional capabilities a host may supply to the built-in catalog.
///
/// One struct rather than eight positional parameters: every field is an `Option`
/// of a different type, so a caller that swapped two of them would still compile
/// and silently offer the wrong commands. Defaulting the whole set is also what
/// makes "which commands does a bare host get" answerable at a glance.
#[derive(Default)]
pub struct CatalogWirings {
    /// `/background`
    pub background: Option<BackgroundWiring>,
    /// `/goal`
    pub goals: Option<GoalWiring>,
    /// `/compact` needs the turn lifecycle, so a host that has one gets the
    /// command and a host without one does not offer a command it cannot run.
    pub chat: Option<Arc<dyn tact_kernel::runtime::ChatService>>,
    /// `/hooks` needs a directory and a renderer, both of which the host owns.
    pub hooks: Option<HooksWiring>,
    /// `/mcp list` and `/mcp prompts` read the router; the host supplies the view.
    pub mcp: Option<McpWiring>,
    /// `/permission` writes through the manager the whole runtime shares.
    pub permission: Option<crate::permission::SharedPermissionManager>,
    /// `/model`, `/thinking` and `/effort` record a change for the host to apply.
    pub settings: Option<Arc<dyn crate::agent::settings_service::SettingsService>>,
}

pub fn builtin_catalog(
    stats: Arc<std::sync::RwLock<crate::stats::SessionStats>>,
    wirings: CatalogWirings,
) -> tact_kernel::catalog::CommandCatalog {
    let CatalogWirings {
        background,
        goals,
        chat,
        hooks,
        mcp,
        permission,
        settings,
    } = wirings;
    let mut catalog = tact_kernel::catalog::CommandCatalog::default();
    for command in builtin_commands(stats) {
        catalog
            .insert(command)
            .expect("built-in command names are unique");
    }
    if let Some(chat) = chat {
        catalog
            .insert(Arc::new(compact::CompactCommand::new(chat)))
            .expect("built-in command names are unique");
    }
    if let Some(settings) = settings {
        for verb in [
            settings::SettingsVerb::Model,
            settings::SettingsVerb::Thinking,
            settings::SettingsVerb::Effort,
        ] {
            catalog
                .insert(Arc::new(settings::SettingsCommand::new(
                    verb,
                    settings.clone(),
                )))
                .expect("built-in command names are unique");
        }
    }
    if let Some(permission) = permission {
        catalog
            .insert(permission::wiring(permission))
            .expect("built-in command names are unique");
    }
    if let Some(mcp) = mcp {
        catalog
            .insert(Arc::new(mcp::McpCommand::new(mcp.catalog, mcp.render)))
            .expect("built-in command names are unique");
    }
    if let Some(hooks) = hooks {
        catalog
            .insert(Arc::new(hooks::HooksCommand::new(
                hooks.work_dir,
                hooks.render,
            )))
            .expect("built-in command names are unique");
    }
    if let Some(wiring) = goals {
        catalog
            .insert(Arc::new(GoalCommand::new(
                wiring.goal_manager,
                wiring.task_manager,
                wiring.limits,
                wiring.events,
                wiring.service,
            )))
            .expect("built-in command names are unique");
    }
    if let Some(wiring) = background {
        catalog
            .insert(Arc::new(BackgroundCommand::new(
                wiring.manager,
                wiring.session,
            )))
            .expect("built-in command names are unique");
    }
    catalog
}

/// What `/background` needs: the manager, and a way to learn the session id.
///
/// A struct rather than a tuple because both fields are `Arc<dyn …>`-shaped and
/// a two-tuple of those reads badly at the call site.
pub struct BackgroundWiring {
    pub manager: Arc<crate::background::SharedBackgroundManager>,
    /// Resolved per call: the driver learns the session id only after the
    /// session row exists, which is after the catalog is built.
    pub session: Arc<dyn Fn() -> Option<String> + Send + Sync>,
}

/// What `/goal` needs: the two managers and the run budget it reports.
pub struct GoalWiring {
    pub goal_manager: crate::goal::SharedGoalManager,
    pub task_manager: crate::task::SharedTaskManager,
    pub limits: crate::goal::GoalLimits,
    /// Where a pause or cancel reports itself.
    ///
    /// The lifecycle helpers answer by emitting, so the command needs the same sink
    /// the driver uses — otherwise `/goal pause` would be the one Goal operation
    /// that says nothing.
    pub events: tact_protocol::events::UpdateSink,
    /// The Goal domain service: creating and accepting a Goal go through it.
    pub service: std::sync::Arc<dyn crate::goal::service::GoalService>,
}

/// A metadata literal with `Runtime` scope, for the built-ins.
pub(crate) fn runtime_meta(name: &str, aliases: &[&str], summary: &str) -> CommandMeta {
    CommandMeta {
        name: name.to_string(),
        aliases: aliases.iter().map(|alias| (*alias).to_string()).collect(),
        summary: summary.to_string(),
        usage: format!("/{name}"),
        scope: tact_kernel::capability::command::CommandScope::Runtime,
        needs_args: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tact_kernel::catalog::CommandCatalog;

    fn catalog() -> CommandCatalog {
        let mut catalog = CommandCatalog::default();
        for command in builtin_commands(Arc::new(std::sync::RwLock::new(
            crate::stats::SessionStats::default(),
        ))) {
            catalog.insert(command).unwrap();
        }
        catalog
    }

    #[test]
    fn every_builtin_command_is_routable_by_name() {
        let catalog = catalog();
        assert!(catalog.get("stats").is_some());
    }

    #[test]
    fn an_unknown_command_says_so() {
        let outcome = catalog().run(&CommandInvocation {
            name: "no-such-command".into(),
            args: String::new(),
        });
        assert!(matches!(outcome, CommandOutcome::Usage(_)), "{outcome:?}");
    }

    #[test]
    fn the_host_catalog_carries_every_builtin() {
        // The driver builds one of these per loop; a command missing from it is
        // unreachable even though it exists.
        let catalog = builtin_catalog(
            Arc::new(std::sync::RwLock::new(crate::stats::SessionStats::default())),
            crate::command::CatalogWirings::default(),
        );
        assert!(catalog.get("stats").is_some());
        assert!(
            catalog.get("background").is_none(),
            "background needs its manager, so a caller without one does not get it"
        );
    }

    #[test]
    fn a_routed_command_answers_not_routes_again() {
        // The catalog's job is to hand the invocation to the owner and return
        // its answer unchanged; a second dispatch layer is what this replaces.
        let outcome = catalog().run(&CommandInvocation {
            name: "stats".into(),
            args: String::new(),
        });
        // The outcome is whatever the owner produced — a titled popup for
        // `/stats` — and the catalog must not reshape it.
        assert!(matches!(outcome, CommandOutcome::Emit(_)), "{outcome:?}");
    }
}
