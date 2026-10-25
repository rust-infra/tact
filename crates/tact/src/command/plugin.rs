//! The command surface as a plugin.
//!
//! The catalog existed before this and had no contributor: a command was built
//! and called directly, which meant the kernel's `CommandCatalog` was a routing
//! table nobody routed through. This plugin is what makes the commands *entries*
//! — `Registrar::command` validates the name, applies the capability filter, and
//! stages the contribution so a failed install leaves nothing behind.
//!
//! One plugin per concern, following the family pattern:
//! [`StatsCommandPlugin`] owns nothing, [`BackgroundCommandPlugin`] owns the
//! manager it reads. A host installs the ones whose state it has.

use std::sync::Arc;

use anyhow::Result;
use tact_kernel::capability::command::Command;
use tact_kernel::plugin::{Plugin, PluginId, PluginKind, PluginManifest, Registrar};

/// Everything a host needs to contribute one command.
///
/// Takes `Arc<dyn Command>` rather than a generic parameter so a plugin can hold
/// a command built elsewhere (which is what the driver does: it builds the stats
/// command over the agent's own handle).
pub struct CommandPlugin {
    id: &'static str,
    summary: &'static str,
    command: Arc<dyn Command>,
}

impl CommandPlugin {
    /// A plugin contributing exactly one command.
    ///
    /// The id is the plugin's, not the command's: a plugin that later grows a
    /// second command keeps its identity, and a capability filter that targets
    /// `command:stats` still works because the *command* keeps its own name.
    pub fn new(id: &'static str, summary: &'static str, command: Arc<dyn Command>) -> Self {
        Self {
            id,
            summary,
            command,
        }
    }

    /// The `/stats` plugin over the given snapshot.
    pub fn stats(stats: Arc<std::sync::RwLock<crate::stats::SessionStats>>) -> Self {
        Self::new(
            "builtin.command.stats",
            "The /stats command",
            Arc::new(crate::command::StatsCommand::new(stats)),
        )
    }

    /// The `/background` plugin over the given manager.
    pub fn background(
        manager: Arc<crate::background::SharedBackgroundManager>,
        session: Arc<dyn Fn() -> Option<String> + Send + Sync>,
    ) -> Self {
        Self::new(
            "builtin.command.background",
            "The /background command",
            Arc::new(crate::command::BackgroundCommand::new(manager, session)),
        )
    }
}

#[async_trait::async_trait]
impl Plugin for CommandPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest::new(
            PluginId::new(self.id).expect("built-in command plugin ids are valid"),
            self.id,
            env!("CARGO_PKG_VERSION"),
            PluginKind::Capability,
            self.summary,
        )
    }

    fn register(&self, registrar: &mut Registrar<'_>) -> Result<()> {
        // `Command` is not `Clone` and `register` takes `&self`, so the shared
        // handle is what lets one instance be contributed: the catalog stores the
        // same `Arc` the caller kept, which is what makes the driver's existing
        // handle and the catalog agree.
        registrar.command(SharedCommand(self.command.clone()))
    }
}

/// One command behind an `Arc`, so a caller can keep using it after installing.
///
/// A newtype rather than implementing `Command for Arc<dyn Command>`: that impl
/// would apply to every command in the crate and make the delegation invisible.
struct SharedCommand(Arc<dyn Command>);

#[async_trait::async_trait]
impl Command for SharedCommand {
    fn meta(&self) -> &tact_kernel::capability::command::CommandMeta {
        self.0.meta()
    }

    fn run(
        &self,
        invocation: &tact_kernel::capability::command::CommandInvocation,
    ) -> tact_kernel::capability::command::CommandOutcome {
        self.0.run(invocation)
    }

    async fn run_async(
        &self,
        invocation: &tact_kernel::capability::command::CommandInvocation,
    ) -> tact_kernel::capability::command::CommandOutcome {
        self.0.run_async(invocation).await
    }

    fn complete(&self, args: &str) -> Vec<tact_kernel::capability::command::CommandCandidate> {
        self.0.complete(args)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tact_kernel::capability::CapabilityFilter;
    use tact_kernel::capability::command::{CommandInvocation, CommandOutcome};
    use tact_kernel::plugin::PluginHost;

    fn stats() -> Arc<std::sync::RwLock<crate::stats::SessionStats>> {
        Arc::new(std::sync::RwLock::new(crate::stats::SessionStats::default()))
    }

    #[tokio::test]
    async fn the_stats_plugin_contributes_the_command() {
        let mut host = PluginHost::new(CapabilityFilter::default());
        host.install(CommandPlugin::stats(stats())).await.unwrap();
        let runtime = host.seal().await.unwrap();
        assert!(runtime.commands().get("stats").is_some());
        // And it answers through the catalog, not just registers.
        let outcome = runtime
            .commands()
            .run_async(&CommandInvocation {
                name: "stats".into(),
                args: String::new(),
            })
            .await;
        assert!(
            matches!(outcome, CommandOutcome::Emit(_)),
            "the command answers with its own popup: {outcome:?}"
        );
    }

    #[tokio::test]
    async fn a_disabled_command_is_absent_from_the_catalog() {
        // The filter is what a user's config turns off; the plugin must not be
        // able to route around it.
        let disabled: tact_kernel::capability::CapabilityId = "command:stats".parse().unwrap();
        let mut host = PluginHost::new(CapabilityFilter::new([disabled]));
        host.install(CommandPlugin::stats(stats())).await.unwrap();
        let runtime = host.seal().await.unwrap();
        assert!(runtime.commands().get("stats").is_none());
        assert!(
            runtime
                .skipped()
                .iter()
                .any(|skipped| skipped.capability.to_string() == "command:stats"),
            "the skip is recorded so the host can tell the user"
        );
    }

    #[tokio::test]
    async fn two_plugins_contributing_one_command_is_refused() {
        let mut host = PluginHost::new(CapabilityFilter::default());
        host.install(CommandPlugin::stats(stats())).await.unwrap();
        let error = host
            .install(CommandPlugin::stats(stats()))
            .await
            .unwrap_err();
        // The duplicate *id* is caught first; either way the second is refused
        // rather than silently shadowing the first.
        assert!(error.to_string().contains("duplicate plugin id"), "{error}");
    }

    #[tokio::test]
    async fn a_different_plugin_cannot_shadow_an_existing_command() {
        struct Impostor;
        #[async_trait::async_trait]
        impl Plugin for Impostor {
            fn manifest(&self) -> PluginManifest {
                PluginManifest::new(
                    PluginId::new("impostor").expect("a literal id"),
                    "impostor",
                    env!("CARGO_PKG_VERSION"),
                    PluginKind::Capability,
                    "attempts to shadow /stats",
                )
            }

            fn register(&self, registrar: &mut Registrar<'_>) -> Result<()> {
                registrar.command(crate::command::StatsCommand::new(stats()))
            }
        }

        let mut host = PluginHost::new(CapabilityFilter::default());
        host.install(CommandPlugin::stats(stats())).await.unwrap();
        let error = host.install(Impostor).await.unwrap_err();
        assert!(error.to_string().contains("stats"), "{error}");
    }
}

#[cfg(test)]
mod emit_tests {
    use super::*;
    use tact_kernel::capability::CapabilityFilter;
    use tact_kernel::capability::command::{
        Command, CommandInvocation, CommandMeta, CommandOutcome, Emitted,
    };
    use tact_kernel::plugin::PluginHost;

    /// A command that answers with a structured update.
    struct Emitter {
        meta: CommandMeta,
    }

    impl Emitter {
        fn new() -> Self {
            Self {
                meta: CommandMeta {
                    name: "emitting".into(),
                    aliases: Vec::new(),
                    summary: "answers with an update".into(),
                    usage: "/emitting".into(),
                    scope: tact_kernel::capability::command::CommandScope::Runtime,
                    needs_args: false,
                },
            }
        }
    }

    #[async_trait::async_trait]
    impl Command for Emitter {
        fn meta(&self) -> &CommandMeta {
            &self.meta
        }

        async fn run_async(&self, _invocation: &CommandInvocation) -> CommandOutcome {
            CommandOutcome::Emit(Emitted::new(tact_protocol::AgentUpdate::Info(
                "from a command".into(),
            )))
        }
    }

    #[tokio::test]
    async fn a_command_can_answer_with_a_structured_update() {
        // A read-out that drives a panel is not a string; flattening it would
        // make the host parse text back into the shape it already had.
        let mut host = PluginHost::new(CapabilityFilter::default());
        host.install(CommandPlugin::new("builtin.command.emitting", "test", {
            Arc::new(Emitter::new())
        }))
        .await
        .unwrap();
        let runtime = host.seal().await.unwrap();
        let outcome = runtime
            .commands()
            .run_async(&CommandInvocation {
                name: "emitting".into(),
                args: String::new(),
            })
            .await;
        let CommandOutcome::Emit(emitted) = outcome else {
            panic!("expected an update, got {outcome:?}");
        };
        assert!(
            matches!(emitted.get(), tact_protocol::AgentUpdate::Info(text) if text == "from a command"),
            "{:?}",
            emitted.get()
        );
    }
}
