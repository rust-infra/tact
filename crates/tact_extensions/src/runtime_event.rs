//! Builders for the protocol events the runtime emits.
//!
//! A producer that cannot know which run it is serving builds its event with
//! `run_id: None`; `ViewUpdateEmitter` attributes it
//! ([`tact_protocol::RuntimeEvent::with_run_id`]). These wrappers exist so a call
//! site reads as one expression instead of a struct literal carrying a field the
//! caller must always leave empty.

use tact_protocol::{ModelCallParams, PlanStep, RuntimeEvent, ThinkingChunk, TokenUsageInfo};
use tact_view::AgentErrorKind;

/// Every builder here mirrors the legacy `AgentUpdate` variant it replaced, so a
/// migrated call site keeps its meaning. `AgentErrorKind` is stringified because
/// the protocol carries the message, not the classification.
pub fn info(content: impl Into<String>) -> RuntimeEvent {
    RuntimeEvent::Info {
        run_id: None,
        content: content.into(),
    }
}

pub fn md_info(content: impl Into<String>) -> RuntimeEvent {
    RuntimeEvent::MdInfo {
        run_id: None,
        content: content.into(),
    }
}

pub fn error(error: AgentErrorKind) -> RuntimeEvent {
    RuntimeEvent::Error {
        run_id: None,
        message: error.to_string(),
    }
}

/// Streaming assistant text.
pub fn text(content: impl Into<String>) -> RuntimeEvent {
    RuntimeEvent::Text {
        run_id: None,
        role: "assistant".into(),
        content: content.into(),
    }
}

pub fn thinking(chunk: ThinkingChunk) -> RuntimeEvent {
    RuntimeEvent::Thinking {
        run_id: None,
        chunk,
    }
}

pub fn step_added(step: PlanStep) -> RuntimeEvent {
    RuntimeEvent::StepAdded { run_id: None, step }
}

pub fn token_usage(usage: TokenUsageInfo) -> RuntimeEvent {
    RuntimeEvent::TokenUsage {
        run_id: None,
        usage,
    }
}

pub fn model_info(params: ModelCallParams) -> RuntimeEvent {
    RuntimeEvent::ModelInfo {
        run_id: None,
        params,
    }
}

pub fn task_complete(content: impl Into<String>) -> RuntimeEvent {
    RuntimeEvent::TaskComplete {
        run_id: None,
        content: content.into(),
    }
}

pub fn task_cancelled() -> RuntimeEvent {
    RuntimeEvent::Cancelled { run_id: None }
}
