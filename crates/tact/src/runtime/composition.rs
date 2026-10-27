//! The only production wiring point for Tact's kernel adapters.

use std::sync::Arc;

use tact_agent_api::{
    EventPort, ModelCapabilities, ModelPort, ToolPort, TranscriptStore, UsageSink,
};
use tact_agent_core::{AgentKernel, KernelDeps};
use tact_llm::LlmProvider;
use tact_protocol::UpdateSink;

use super::{
    activation_view::ModelIdentity,
    compaction_adapter::TactCompactor,
    context_adapter::TactContextPolicy,
    event_adapter::TactEventBridge,
    facade_state::{FacadeReceiptTracker, FacadeReceipts},
    gate_adapter::TactToolGate,
    model_adapter::TactModelPort,
    noop_transcript::NoopTranscriptStore,
    prompt_adapter::{PromptCaches, SystemPromptAssembler},
    services::TactSharedServices,
    store_adapter::SqliteTranscriptStore,
    tool_adapter::TactToolPort,
    usage_adapter::{NoopUsageSink, SqliteUsageSink},
};
use crate::{
    hook::Hook,
    mcp::McpCatalogHandle,
    security::{SecurityConfig, sensitive::Scanner},
    store::DynSessionStore,
    tool::{ToolContext, ToolRouter},
};

pub(crate) struct TactKernelConfig {
    pub provider: LlmProvider,
    pub cancel_flag: Arc<std::sync::atomic::AtomicBool>,
    pub capabilities: ModelCapabilities,
    pub prompt: Arc<SystemPromptAssembler>,
    pub compactor: Arc<TactCompactor>,
    pub model_metadata: serde_json::Value,
    pub model: ModelIdentity,
    pub session_id: String,
    pub initial_session_context: Vec<crate::hook::HookContextChunk>,
    pub services: Arc<TactSharedServices>,
    pub hooks: Vec<Hook>,
    pub ui: crate::ui_responder::UiResponder,
    pub is_subagent: bool,
    pub tools: ToolRouter,
    pub mcp: McpCatalogHandle,
    pub tool_context: ToolContext,
    pub security: SecurityConfig,
    pub scanner: Scanner,
    pub hide_compact: bool,
    pub store: Option<DynSessionStore>,
    pub receipts: Arc<FacadeReceiptTracker>,
    pub updates: UpdateSink,
}

pub(crate) struct TactKernelComposition {
    kernel: AgentKernel,
    context: Arc<TactContextPolicy>,
    prompt: Arc<SystemPromptAssembler>,
    compactor: Arc<TactCompactor>,
    receipts: Arc<FacadeReceiptTracker>,
}

impl TactKernelComposition {
    pub(crate) fn new(config: TactKernelConfig) -> Self {
        let receipts = config.receipts.clone();
        let goal_active = config.tool_context.active_goal_id.is_some();
        let events: Arc<dyn EventPort> = Arc::new(TactEventBridge::new(config.updates.clone()));
        let model: Arc<dyn ModelPort> = Arc::new(
            TactModelPort::from_provider(config.provider, config.capabilities)
                .with_cancel_flag(config.cancel_flag.clone()),
        );
        let tools: Arc<dyn ToolPort> = Arc::new(TactToolPort::new(
            config.tools,
            config.mcp,
            config.tool_context,
            config.security,
            config.scanner,
            config.hide_compact,
        ));
        let gate = Arc::new(TactToolGate::new(
            config.services.clone(),
            config.ui,
            &config.hooks,
            None,
            config.model.clone(),
            config.compactor.session_start_source(),
            config.is_subagent,
        ));
        let context = Arc::new(TactContextPolicy::new(
            config.services.clone(),
            config.session_id.clone(),
            config.prompt.clone(),
            config.compactor.clone() as Arc<dyn super::compaction_adapter::Compactor>,
            config.model,
            config.initial_session_context,
            &config.hooks,
            config.model_metadata,
            goal_active,
        ));
        let transcript: Arc<dyn TranscriptStore> = match config.store.clone() {
            Some(store) => Arc::new(SqliteTranscriptStore::with_receipts(
                store,
                config.receipts.clone(),
            )),
            None => Arc::new(NoopTranscriptStore::default()),
        };
        let usage: Arc<dyn UsageSink> = match config.store {
            Some(store) => Arc::new(SqliteUsageSink::new(store, config.session_id)),
            None => Arc::new(NoopUsageSink::default()),
        };
        let kernel = AgentKernel::new(KernelDeps {
            model,
            tools,
            gate,
            context: context.clone(),
            transcript,
            usage,
            events,
        });
        Self {
            kernel,
            context,
            prompt: config.prompt,
            compactor: config.compactor,
            receipts,
        }
    }

    pub(crate) fn kernel(&self) -> &AgentKernel {
        &self.kernel
    }

    pub(crate) fn session_start_pending(&self) -> bool {
        self.context.session_start_pending()
    }

    pub(crate) fn pending_session_context(&self) -> Vec<crate::hook::HookContextChunk> {
        self.context.pending_session_context()
    }

    pub(crate) fn session_start_source(&self) -> crate::hook::SessionStartSource {
        self.context.session_start_source()
    }

    pub(crate) fn compact_state(&self) -> crate::compact::CompactState {
        self.compactor.compact_state()
    }

    pub(crate) fn prompt_caches(&self) -> PromptCaches {
        self.prompt.caches()
    }

    pub(crate) fn receipts(&self) -> FacadeReceipts {
        self.receipts.snapshot()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use tact_agent_api::{
        AgentMessage, AgentRole, AgentState, CancelToken, MessageOrigin, RunInput, RunStop,
        SessionRef,
    };
    use tact_llm::{ContentBlock, LlmProvider, MockClient, ProviderKind, StopReason};

    use super::*;
    use crate::{
        mcp::{MCPToolRouter, McpCatalogHandle},
        permission::{PermissionManager, PermissionMode},
        runtime::permission_authority::PermissionAuthority,
        stats::SessionStats,
        tool::{ToolContext, ToolRouter},
    };

    fn tool_context() -> ToolContext {
        crate::tool::test_support::test_context("kernel-composition")
    }

    #[tokio::test]
    async fn sessionless_composition_runs_a_simple_text_activation() {
        let services = Arc::new(TactSharedServices::new(
            UpdateSink::default(),
            Arc::new(std::sync::RwLock::new(SessionStats::default())),
            PermissionAuthority::new(PermissionManager::try_new(PermissionMode::Default).unwrap()),
            Arc::new(Mutex::new(std::collections::VecDeque::new())),
            Arc::new(Mutex::new(std::collections::VecDeque::new())),
            Arc::new(std::sync::atomic::AtomicU64::new(1)),
            McpCatalogHandle::new(MCPToolRouter::new()),
        ));
        let context = tool_context();
        let prompt = Arc::new(SystemPromptAssembler::static_text("system"));
        let compactor = Arc::new(TactCompactor::new(
            services.clone(),
            prompt.clone(),
            "sessionless",
            ModelIdentity {
                name: "mock".into(),
            },
            context.work_dir.clone(),
            crate::config::test_support::resolved_config().agent,
            Vec::new(),
            ProviderKind::OpenAi,
            false,
            false,
            crate::hook::SessionStartSource::Startup,
            crate::compact::CompactState::default(),
            &[],
        ));
        let composition = TactKernelComposition::new(TactKernelConfig {
            provider: LlmProvider::Mock(MockClient::new(vec![(
                vec![ContentBlock::Text {
                    text: "hello".into(),
                }],
                Some(StopReason::EndTurn),
            )])),
            cancel_flag: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            capabilities: ModelCapabilities {
                model: "mock".into(),
                context_window: 1_000,
                max_output_tokens: 100,
                supports_streaming: true,
                supports_native_compaction: false,
            },
            prompt,
            compactor,
            model_metadata: serde_json::json!({}),
            model: ModelIdentity {
                name: "mock".into(),
            },
            session_id: String::new(),
            initial_session_context: Vec::new(),
            services,
            hooks: Vec::new(),
            ui: crate::ui_responder::UiResponder::new(),
            is_subagent: false,
            tools: ToolRouter::new(),
            mcp: McpCatalogHandle::new(MCPToolRouter::new()),
            tool_context: context,
            security: PermissionManager::try_new(PermissionMode::Default)
                .unwrap()
                .security_config(),
            scanner: PermissionManager::try_new(PermissionMode::Default)
                .unwrap()
                .scanner(),
            hide_compact: false,
            store: None,
            receipts: FacadeReceiptTracker::new(FacadeReceipts::default()),
            updates: UpdateSink::default(),
        });
        let mut state = AgentState {
            initialized: true,
            ..AgentState::default()
        };
        let outcome = composition
            .kernel()
            .run(
                &mut state,
                RunInput {
                    session: SessionRef::new("sessionless"),
                    inputs: vec![AgentMessage::text(
                        AgentRole::User,
                        MessageOrigin::User,
                        "hello",
                    )],
                    max_total_turns: None,
                },
                CancelToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(outcome.stop, RunStop::Completed);
    }
}
