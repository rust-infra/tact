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
pub mod plugin;
pub mod stats;

pub use background::BackgroundCommand;
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
pub fn builtin_catalog(
    stats: Arc<std::sync::RwLock<crate::stats::SessionStats>>,
    background: Option<BackgroundWiring>,
) -> tact_kernel::catalog::CommandCatalog {
    let mut catalog = tact_kernel::catalog::CommandCatalog::default();
    for command in builtin_commands(stats) {
        catalog
            .insert(command)
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
            None,
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
        assert!(matches!(outcome, CommandOutcome::Message(_)), "{outcome:?}");
    }
}
