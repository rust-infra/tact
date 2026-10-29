use thiserror::Error;

/// Failure while constructing an agent or session from the builder.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum AgentBuildError {
    #[error("cannot configure a custom context policy with system_prompt or model_options")]
    ConflictingContextConfig,

    #[error("tools require an explicit tool gate")]
    MissingToolGate,

    #[error("restored session state must be initialized")]
    UninitializedState,

    #[error("restored session state belongs to a different session")]
    SessionMismatch,
}

/// Failure while composing a static tool set.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ToolSetError {
    #[error("duplicate tool name: {0}")]
    DuplicateTool(String),
}
