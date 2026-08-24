//! Re-exports of the generic subset of `tact_protocol` — the kit's public
//! wire contract.
//!
//! Deliberately **not** re-exported: `tact_protocol::biz` (balance/quota —
//! Tact business, handled by the host extension), and any future Tact-only
//! protocol additions.

pub use tact_protocol::agent::{
    AgentErrorKind, AgentUpdate, ModelCallParams, PlanStep, StepResult, StepStatus, TaskSnapshot,
    TaskStatusSnapshot, TasksChangeReason, ThinkingChunk, TokenUsageInfo, ToolDetailKind,
    ToolPopupKind, ToolPresentationInfo, ToolVisualKind,
};
pub use tact_protocol::tool_output::{
    SubagentSection, SubagentSectionBlock, THINKING_SECTION_HEADER, ToolOutputBuffer,
    ToolOutputChunk, ToolOutputLine, ToolOutputSpan, ToolOutputStream,
};
