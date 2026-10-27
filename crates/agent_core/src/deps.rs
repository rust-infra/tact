use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use tact_agent_api::{
    AgentEvent, ContextPolicy, EventObserver, EventPort, ModelPort, RunPhase, RuntimeErrorInfo,
    ToolGate, ToolPort, TranscriptStore, UsageSink,
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
}

impl EventBus {
    pub(crate) fn new(port: Arc<dyn EventPort>, observers: Vec<Arc<dyn EventObserver>>) -> Self {
        Self { port, observers }
    }

    pub(crate) fn emit(&self, event: AgentEvent) {
        self.port.emit(event.clone());
        for observer in &self.observers {
            observer.observe(&event);
        }
    }
}

/// One host-facing kernel.
pub struct AgentKernel {
    pub(crate) deps: KernelDeps,
    pub(crate) events: EventBus,
    next_run_id: AtomicU64,
}

impl AgentKernel {
    pub fn new(deps: KernelDeps) -> Self {
        Self::with_extensions(deps, KernelExtensions::default())
    }

    pub fn with_extensions(deps: KernelDeps, extensions: KernelExtensions) -> Self {
        let events = EventBus::new(deps.events.clone(), extensions.observers);
        Self {
            deps,
            events,
            next_run_id: AtomicU64::new(1),
        }
    }

    pub(crate) fn next_run_id(&self) -> u64 {
        self.next_run_id.fetch_add(1, Ordering::Relaxed)
    }
}

/// Typed kernel failure.
#[derive(Debug, thiserror::Error)]
pub enum KernelError {
    #[error("kernel contract failure: {0}")]
    Contract(String),
    #[error("transcript load failed: {0}")]
    Load(String),
    #[error("transcript commit failed: {0}")]
    Store(String),
    #[error("context preparation failed: {0}")]
    Context(String),
    #[error("model call failed: {0}")]
    Model(String),
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
            detail: None,
        }
    }
}
