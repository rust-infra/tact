//! How a subagent is built, contributed as a port.
//!
//! The design lists `AgentFactory` as what `AgentPlugin` contributes alongside the
//! agent service, and the reason is the same: a host that wants a different child —
//! its own prompt, its own toolset, its own turn budget — should not have to fork
//! `spawn_subagent`. The tool asks this port instead of assembling an `Agent` itself.
//!
//! The factory is synchronous because building an agent is: `Agent::new` and its
//! builders do no IO. Everything that does IO (the session row, the store) happens
//! around it, in the tool, where the await already is.

use std::sync::Arc;

use anyhow::Result;

use crate::tool::ToolContext;

/// Everything needed to build one child agent.
///
/// A struct rather than eight parameters: the fields are all distinct types today,
/// but a caller that swapped the prompt and the settings would still compile, and
/// the failure would be a child running with the wrong instructions.
pub struct SubagentSpec {
    pub client: tact_llm::LlmProvider,
    /// The parent's context, already adjusted for a child (no goal binding, and
    /// `is_subagent` set so its prompts are distinguishable from the parent's).
    pub context: ToolContext,
    pub tools: crate::tool::ToolRouter,
    /// The child's MCP router.
    ///
    /// Passed in rather than built here: a child currently gets an empty one, and
    /// that is the caller's policy, not the factory's. It also keeps this module
    /// free of an `mcp` edge, which the module-boundary guard enforces for
    /// everything under `agent/`.
    pub mcp_router: crate::mcp::MCPToolRouter,
    pub permission_manager: crate::permission::PermissionManager,
    pub system_prompt: String,
    pub settings: crate::config::AgentSettings,
    pub max_turns: Option<u32>,
    pub session_id: String,
    pub store: crate::store::DynSessionStore,
}

/// Builds a child agent from a spec.
pub trait AgentFactory: Send + Sync {
    fn build(&self, spec: SubagentSpec) -> crate::Agent;
}

/// The factory a host gets when it installs the built-in agent plugin.
pub struct BuiltinAgentFactory;

impl AgentFactory for BuiltinAgentFactory {
    fn build(&self, spec: SubagentSpec) -> crate::Agent {
        crate::Agent::new(
            spec.client,
            spec.context,
            spec.tools,
            spec.mcp_router,
            spec.permission_manager,
            crate::AgentSystemPrompt::Static(spec.system_prompt),
        )
        .with_agent_settings(spec.settings)
        .with_max_turns(spec.max_turns)
        .with_session(spec.session_id, spec.store)
    }
}

/// The factory an [`AgentPlugin`](crate::agent::plugin::AgentPlugin) provided.
///
/// A concrete newtype because the registry keys on `TypeId`: providing
/// `Arc<dyn AgentFactory>` would make it unretrievable as the type a consumer asks
/// for.
#[derive(Clone)]
pub struct AgentFactoryHandle(pub Arc<dyn AgentFactory>);

/// The factory a host resolved from its registry.
///
/// # Errors
///
/// Returns an error when no plugin provided one. `spawn_subagent` reports that by
/// name rather than building a child nobody can replace.
pub fn agent_factory(
    services: &tact_kernel::service::ServiceRegistry,
) -> Result<Arc<dyn AgentFactory>> {
    services
        .get::<AgentFactoryHandle>()
        .map(|handle| handle.0.clone())
        .ok_or_else(|| anyhow::anyhow!("no agent factory installed; install AgentPlugin"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tact_kernel::capability::CapabilityFilter;
    use tact_kernel::plugin::{
        Plugin, PluginHost, PluginId, PluginKind, PluginManifest, Registrar,
    };

    /// A plugin that contributes the built-in factory.
    struct FactoryPlugin;

    impl Plugin for FactoryPlugin {
        fn manifest(&self) -> PluginManifest {
            PluginManifest::new(
                PluginId::new("test.agent_factory").expect("a literal id"),
                "Agent factory",
                "0.1.0",
                PluginKind::Runtime,
                "test double",
            )
        }

        fn register(&self, registrar: &mut Registrar<'_>) -> Result<()> {
            registrar.service(Arc::new(AgentFactoryHandle(Arc::new(BuiltinAgentFactory))))
        }
    }

    #[tokio::test]
    async fn the_factory_resolves_by_name() {
        let mut host = PluginHost::new(CapabilityFilter::default());
        host.install(FactoryPlugin).await.unwrap();
        let runtime = host.seal().await.unwrap();
        assert!(agent_factory(runtime.services()).is_ok());
    }

    #[tokio::test]
    async fn a_host_without_the_plugin_says_so() {
        let mut host = PluginHost::new(CapabilityFilter::default());
        let runtime = host.seal().await.unwrap();
        // `expect_err` needs the ok type to be `Debug`, and `Arc<dyn AgentFactory>`
        // deliberately is not: a trait object has no meaningful debug form.
        let error = match agent_factory(runtime.services()) {
            Ok(_) => panic!("a host with no factory must fail to resolve one"),
            Err(error) => error,
        };
        assert!(
            error.to_string().contains("install AgentPlugin"),
            "the error must name the missing plugin, got: {error}"
        );
    }

    #[test]
    fn the_plugin_manifest_names_the_agent_domain() {
        let manifest = FactoryPlugin.manifest();
        assert_eq!(manifest.kind, PluginKind::Runtime);
    }
}
