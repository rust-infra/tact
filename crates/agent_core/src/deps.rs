use std::sync::Arc;

use tact_agent_api::{
    AgentEvent, ContextPolicy, EventObserver, EventPort, ModelError, ModelPort, RunContext,
    RunPhase, RuntimeErrorInfo, ToolGate, ToolPort, TranscriptStore, UsageSink,
};

/// Required kernel dependencies. Every capability is mandatory and injected.
#[derive(Clone)]
pub struct KernelDeps {
    pub model: Arc<dyn ModelPort>,
    pub tools: Arc<dyn ToolPort>,
    pub gate: Arc<dyn ToolGate>,
    pub context: Arc<dyn ContextPolicy>,
    pub transcript: Arc<dyn TranscriptStore>,
    pub usage: Arc<dyn UsageSink>,
    pub events: Arc<dyn EventPort>,
}

/// Optional ordered extensions. Only observers exist in the first slice.
#[derive(Clone, Default)]
pub struct KernelExtensions {
    pub observers: Vec<Arc<dyn EventObserver>>,
}

/// Ordered event fan-out shared by kernel, model scope, and tool progress.
#[derive(Clone)]
pub(crate) struct EventBus {
    port: Arc<dyn EventPort>,
    observers: Vec<Arc<dyn EventObserver>>,
    context: Option<RunContext>,
}

impl EventBus {
    pub(crate) fn new(port: Arc<dyn EventPort>, observers: Vec<Arc<dyn EventObserver>>) -> Self {
        Self {
            port,
            observers,
            context: None,
        }
    }

    pub(crate) fn with_context(&self, context: RunContext) -> Self {
        Self {
            port: self.port.clone(),
            observers: self.observers.clone(),
            context: Some(context),
        }
    }

    pub(crate) fn emit(&self, event: AgentEvent) {
        let delivery = event.delivery();
        let event = Arc::new(event);
        if let Some(context) = &self.context {
            self.port
                .emit_shared_with_context(context, event.clone(), delivery);
            for observer in &self.observers {
                observer.observe_with_context(context, &event, delivery);
            }
        } else {
            self.port.emit_shared(event.clone(), delivery);
            for observer in &self.observers {
                observer.observe_with_delivery(&event, delivery);
            }
        }
    }
}

/// One host-facing kernel.
pub struct AgentKernel {
    pub(crate) deps: KernelDeps,
    pub(crate) events: EventBus,
}

impl AgentKernel {
    pub fn new(deps: KernelDeps) -> Self {
        Self::with_extensions(deps, KernelExtensions::default())
    }

    pub fn with_extensions(deps: KernelDeps, extensions: KernelExtensions) -> Self {
        let events = EventBus::new(deps.events.clone(), extensions.observers);
        Self { deps, events }
    }

    pub(crate) fn with_context(&self, context: RunContext) -> Self {
        Self {
            deps: self.deps.clone(),
            events: self.events.with_context(context),
        }
    }
}

/// Typed kernel failure.
#[derive(Debug, thiserror::Error)]
pub enum KernelError {
    #[error("run cancelled")]
    Cancelled,
    #[error("kernel contract failure: {0}")]
    Contract(String),
    #[error("transcript load failed: {0}")]
    Load(String),
    #[error("transcript commit failed: {0}")]
    Store(String),
    #[error("context preparation failed: {0}")]
    Context(String),
    #[error("model call failed: {0}")]
    Model(#[source] ModelError),
    #[error("tool refresh failed: {0}")]
    Tool(String),
    #[error("tool gate failed: {0}")]
    Gate(String),
    #[error("usage persistence failed: {0}")]
    Usage(String),
    #[error("run failed during {phase:?}: {message}")]
    Runtime {
        phase: RunPhase,
        message: String,
        detail: Option<String>,
    },
}

impl KernelError {
    pub fn phase(&self) -> RunPhase {
        match self {
            Self::Cancelled => RunPhase::Stream,
            Self::Load(_) => RunPhase::Load,
            Self::Context(_) => RunPhase::PrepareRun,
            Self::Model(_) => RunPhase::Stream,
            Self::Tool(_) | Self::Gate(_) => RunPhase::Execute,
            Self::Store(_) | Self::Usage(_) => RunPhase::Commit,
            Self::Contract(_) => RunPhase::PrepareRun,
            Self::Runtime { phase, .. } => *phase,
        }
    }

    pub(crate) fn info(&self) -> RuntimeErrorInfo {
        RuntimeErrorInfo {
            phase: self.phase(),
            message: self.to_string(),
            // The action the caller can take, kept next to a machine-readable
            // phase: a host that only sees the event still knows whether to
            // offer a retry or a config change.
            detail: self.advisory(),
            retryable: self.is_retryable(),
        }
    }

    /// Whether resending the same work could plausibly succeed.
    ///
    /// Only a transient model failure qualifies. Everything else — a rejected
    /// credential, an unknown stop reason, a store failure — reproduces on the
    /// next attempt, so a host must not offer a button that cannot work.
    fn is_retryable(&self) -> bool {
        match self {
            Self::Model(error) => error.is_retryable(),
            _ => false,
        }
    }

    /// One line telling the reader what to change, when that is knowable.
    fn advisory(&self) -> Option<String> {
        match self {
            Self::Model(error) => Some(match error.kind {
                tact_agent_api::ModelErrorKind::Transient => {
                    "the provider stayed unreachable after every retry; check the network or proxy \
                     and retry"
                        .to_string()
                }
                tact_agent_api::ModelErrorKind::Authentication => {
                    "the provider rejected the credential; replace the API key for this provider \
                     in settings or the environment, then retry"
                        .to_string()
                }
                tact_agent_api::ModelErrorKind::ContextOverflow => {
                    "the conversation no longer fits the model's context window; run /compact or \
                     start a new session"
                        .to_string()
                }
                tact_agent_api::ModelErrorKind::Cancelled => return None,
                tact_agent_api::ModelErrorKind::Unsupported => {
                    "this provider does not implement the requested operation; switch provider or \
                     disable the feature"
                        .to_string()
                }
                // `Permanent` and any kind a newer adapter adds: do not retry,
                // and point at the three settings that cause it most often.
                _ => "the provider rejected this request; check the model name, base URL, and \
                      credentials for this provider"
                    .to_string(),
            }),
            Self::Cancelled => None,
            _ => None,
        }
    }
}
