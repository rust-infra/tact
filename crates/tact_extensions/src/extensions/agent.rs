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
    ///
    /// Consumes the Agent. A host that still needs it after the run (the
    /// headless host reads its stats, final message, session-end hooks and
    /// shutdown) uses [`Self::from_shared`] instead.
    #[must_use]
    pub fn from_agent(agent: Agent) -> Self {
        let cancel_flag = agent.runtime.cancel_flag.clone();
        Self::from_shared(Arc::new(tokio::sync::Mutex::new(agent)), cancel_flag)
    }

    /// Wraps an Agent the host still needs to own, shared behind a mutex.
    ///
    /// The Router's `runs.start` handler locks the Agent for the duration of
    /// one run; the host waits for that call to return and then takes the lock
    /// back for teardown. Sharing the same value (rather than moving it in)
    /// is what keeps the run and its teardown on one conversation.
    ///
    /// The `cancel_flag` is passed separately, not read from inside the lock:
    /// [`AgentExecutor::cancel`] is synchronous and sets the flag in response
    /// to `runs.cancel` *while a run holds the Agent lock*, so it cannot
    /// `await` that lock to reach the flag. `from_agent` reads the same flag
    /// out of the Agent before moving it behind the mutex, so both paths share
    /// one flag.
    #[must_use]
    pub fn from_shared(
        agent: Arc<tokio::sync::Mutex<Agent>>,
        cancel_flag: Arc<std::sync::atomic::AtomicBool>,
    ) -> Self {
        Self::new(Arc::new(InProcessAgentExecutor::new(agent, cancel_flag)))
    }

    pub fn register(&self, runtime: &RuntimeContext) -> Result<(), KernelError> {
        let handler: Arc<dyn CapabilityHandler> = Arc::new(AgentRunHandler {
            executor: self.executor.clone(),
        });
        // `chat.start_run` stays this extension's: the Chat manifest declares
        // the run entry (a run belongs to a conversational turn), and the Agent
        // extension is what actually runs it.
        let chat_capability = super::chat::start_run_capability();
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
    /// Starts one turn with the caller-supplied user message.
    ///
    /// Takes the full [`tact_llm::Message`], not a bare string, so a client
    /// that needs blocks (images, structured input) is not silently narrowed
    /// to text at the extension boundary.
    async fn run(
        &self,
        context: InvocationContext,
        message: tact_llm::Message,
    ) -> Result<RunId, KernelError>;

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
    agent: Arc<tokio::sync::Mutex<Agent>>,
    /// Cloned from the agent at construction so `runs.cancel` can set it
    /// without waiting on the run that currently holds the agent lock.
    cancel_flag: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl InProcessAgentExecutor {
    fn new(
        agent: Arc<tokio::sync::Mutex<Agent>>,
        cancel_flag: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) -> Self {
        Self { agent, cancel_flag }
    }
}

#[async_trait]
impl AgentExecutor for InProcessAgentExecutor {
    fn cancel(&self, _run_id: &RunId) -> Result<(), KernelError> {
        self.cancel_flag
            .store(true, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }

    async fn run(
        &self,
        context: InvocationContext,
        message: tact_llm::Message,
    ) -> Result<RunId, KernelError> {
        let mut agent = self.agent.lock().await;
        let cancel_flag = agent.runtime.cancel_flag.clone();
        let cancellation = context.cancellation_token();
        let result = {
            let run = agent.agent_loop(Some(message));
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

/// The `runs.start` request body.
///
/// Exactly one of `message` (a plain-text prompt) or `content` (a full
/// [`tact_llm::MessageContent`], text or blocks) must be supplied. `message`
/// stays the common case and keeps every existing caller — the chat extension
/// and the integration tests — working unchanged.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AgentRunInput {
    /// A plain-text user prompt. Mutually exclusive with `content`.
    #[serde(default)]
    message: Option<String>,
    /// The full user message body, for clients that send blocks rather than
    /// a bare string. Mutually exclusive with `message`.
    #[serde(default)]
    content: Option<tact_llm::MessageContent>,
}

fn invalid_run_input(detail: impl std::fmt::Display) -> KernelError {
    KernelError::new(
        tact_protocol::ErrorCategory::InvalidRequest,
        format!("invalid runs.start request: {detail}"),
        "agent",
        false,
    )
}

/// Builds the user turn from the request, enforcing exactly one non-empty body.
fn user_message_from(request: AgentRunInput) -> Result<tact_llm::Message, KernelError> {
    use tact_llm::{Message, MessageKind, Role};

    match (request.message, request.content) {
        (Some(text), None) => {
            if text.trim().is_empty() {
                return Err(invalid_run_input("`message` cannot be empty"));
            }
            Ok(Message::new_text(Role::User, text))
        }
        (None, Some(content)) => {
            if message_content_is_empty(&content) {
                return Err(invalid_run_input("`content` cannot be empty"));
            }
            Ok(Message {
                role: Role::User,
                content,
                kind: MessageKind::Normal,
            })
        }
        (None, None) => Err(invalid_run_input(
            "exactly one of `message` or `content` is required",
        )),
        (Some(_), Some(_)) => Err(invalid_run_input(
            "`message` and `content` are mutually exclusive",
        )),
    }
}

fn message_content_is_empty(content: &tact_llm::MessageContent) -> bool {
    use tact_llm::{ContentBlock, MessageContent};

    match content {
        MessageContent::Text { content } => content.trim().is_empty(),
        MessageContent::Blocks { content } => {
            content.is_empty()
                || content.iter().all(|block| match block {
                    ContentBlock::Text { text } => text.trim().is_empty(),
                    _ => false,
                })
        }
    }
}

#[async_trait]
impl CapabilityHandler for AgentRunHandler {
    async fn invoke(&self, context: InvocationContext, input: Value) -> Result<Value, KernelError> {
        let request: AgentRunInput = serde_json::from_value(input).map_err(invalid_run_input)?;
        let message = user_message_from(request)?;
        let run_id = self.executor.run(context, message).await?;
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
                run_id: Some(request.run_id),
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
        // A run dispatches tool capabilities through the router, so the Tools
        // extension must be registered before this one.
        dependencies: vec![PluginId::from("tact.tools")],
    }
}
