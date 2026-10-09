//! Agent extension entry point over the Kernel capability protocol.

use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use tact_protocol::{
    CapabilityDeclaration, CapabilityKind, CapabilityRisk, PluginId, ProtocolVersion, RunId,
};

use crate::{Agent, plugin::RuntimePluginManifest};
use tact::{
    CapabilityHandler, CapabilityRegistration, InvocationContext, KernelError, RuntimeContext,
};

/// The official Agent extension's manifest and capability entry point.
#[derive(Clone)]
pub struct AgentExtension {
    executor: Arc<dyn AgentExecutor>,
}

impl AgentExtension {
    #[must_use]
    pub fn new(executor: Arc<dyn AgentExecutor>) -> Self {
        Self { executor }
    }

    /// Wraps the in-process Agent implementation behind the extension API.
    #[must_use]
    pub fn from_agent(agent: Agent) -> Self {
        Self::new(Arc::new(InProcessAgentExecutor::new(agent)))
    }

    pub fn register(&self, runtime: &RuntimeContext) -> Result<(), KernelError> {
        let handler: Arc<dyn CapabilityHandler> = Arc::new(AgentRunHandler {
            executor: self.executor.clone(),
        });
        let chat_capability = super::chat::manifest()
            .capabilities
            .into_iter()
            .next()
            .expect("Chat declares its start-run capability");
        let cancel_handler: Arc<dyn CapabilityHandler> = Arc::new(AgentCancelHandler {
            executor: self.executor.clone(),
        });
        runtime.router().register_many(vec![
            CapabilityRegistration::new(run_capability(), handler.clone()),
            CapabilityRegistration::new(cancel_capability(), cancel_handler),
            CapabilityRegistration::new(chat_capability, handler),
        ])
    }
}

/// Runs one user turn and returns the stable run identity.
#[async_trait]
pub trait AgentExecutor: Send + Sync {
    async fn run(&self, context: InvocationContext, message: String) -> Result<RunId, KernelError>;

    /// Requests cooperative cancellation of the in-flight run. Best-effort: the
    /// agent loop observes the flag at its next checkpoint.
    fn cancel(&self, _run_id: &RunId) -> Result<(), KernelError> {
        Err(KernelError::new(
            tact_protocol::ErrorCategory::CapabilityNotFound,
            "run cancellation is not available",
            "agent",
            false,
        ))
    }
}

struct InProcessAgentExecutor {
    agent: tokio::sync::Mutex<Agent>,
    /// Cloned from the agent at construction so `runs.cancel` can set it
    /// without waiting on the run that currently holds the agent lock.
    cancel_flag: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl InProcessAgentExecutor {
    fn new(agent: Agent) -> Self {
        let cancel_flag = agent.runtime.cancel_flag.clone();
        Self {
            agent: tokio::sync::Mutex::new(agent),
            cancel_flag,
        }
    }
}

#[async_trait]
impl AgentExecutor for InProcessAgentExecutor {
    fn cancel(&self, _run_id: &RunId) -> Result<(), KernelError> {
        self.cancel_flag
            .store(true, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }

    async fn run(&self, context: InvocationContext, message: String) -> Result<RunId, KernelError> {
        use tact_llm::{Message, Role};

        let mut agent = self.agent.lock().await;
        let cancel_flag = agent.runtime.cancel_flag.clone();
        let cancellation = context.cancellation_token();
        let result = {
            let run = agent.agent_loop(Some(Message::new_text(Role::User, message)));
            tokio::pin!(run);
            tokio::select! {
                result = &mut run => {
                    result.map_err(|error| {
                        KernelError::new(
                            tact_protocol::ErrorCategory::InternalError,
                            error.to_string(),
                            "agent",
                            false,
                        )
                    })
                }
                _ = cancellation.cancelled() => {
                    cancel_flag.store(true, std::sync::atomic::Ordering::SeqCst);
                    Err(KernelError::cancelled())
                }
            }
        };
        result?;
        agent.runtime.current_run_id.clone().ok_or_else(|| {
            KernelError::new(
                tact_protocol::ErrorCategory::InternalError,
                "agent completed without a run identity",
                "agent",
                false,
            )
        })
    }
}

struct AgentRunHandler {
    executor: Arc<dyn AgentExecutor>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AgentRunInput {
    message: String,
}

#[async_trait]
impl CapabilityHandler for AgentRunHandler {
    async fn invoke(&self, context: InvocationContext, input: Value) -> Result<Value, KernelError> {
        let request: AgentRunInput = serde_json::from_value(input).map_err(|error| {
            KernelError::new(
                tact_protocol::ErrorCategory::InvalidRequest,
                format!("invalid runs.start request: {error}"),
                "agent",
                false,
            )
        })?;
        if request.message.trim().is_empty() {
            return Err(KernelError::new(
                tact_protocol::ErrorCategory::InvalidRequest,
                "runs.start message cannot be empty",
                "agent",
                false,
            ));
        }
        let run_id = self.executor.run(context, request.message).await?;
        Ok(json!({"run_id": run_id}))
    }
}

fn run_capability() -> CapabilityDeclaration {
    CapabilityDeclaration {
        name: "runs.start".into(),
        kind: CapabilityKind::App,
        version: "1".into(),
        description: Some("Start an Agent run and stream its Runtime events".into()),
        input_schema: None,
        output_schema: None,
        risk: CapabilityRisk::Medium,
    }
}

fn cancel_capability() -> CapabilityDeclaration {
    CapabilityDeclaration {
        name: "runs.cancel".into(),
        kind: CapabilityKind::Command,
        version: "1".into(),
        description: Some("Request cancellation of a running Agent run".into()),
        input_schema: None,
        output_schema: None,
        risk: CapabilityRisk::Medium,
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AgentCancelInput {
    run_id: RunId,
}

struct AgentCancelHandler {
    executor: Arc<dyn AgentExecutor>,
}

#[async_trait]
impl CapabilityHandler for AgentCancelHandler {
    async fn invoke(&self, context: InvocationContext, input: Value) -> Result<Value, KernelError> {
        let request: AgentCancelInput = serde_json::from_value(input).map_err(|error| {
            KernelError::new(
                tact_protocol::ErrorCategory::InvalidRequest,
                format!("invalid runs.cancel request: {error}"),
                "agent",
                false,
            )
        })?;
        self.executor.cancel(&request.run_id)?;
        let _ = context
            .events()
            .publish(tact_protocol::RuntimeEvent::Cancelled {
                run_id: request.run_id,
            })
            .await;
        Ok(json!({"cancelled": true}))
    }
}

pub fn manifest() -> RuntimePluginManifest {
    RuntimePluginManifest {
        id: PluginId::from("tact.agent"),
        version: env!("CARGO_PKG_VERSION").into(),
        protocol: ProtocolVersion::CURRENT,
        capabilities: vec![run_capability(), cancel_capability()],
    }
}
