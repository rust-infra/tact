use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::{
    AgentMessage, AgentRole, CompactionKind, MessageDelta, MessageId, MessageRange, ModelCallKind,
    ProviderActivity, RunId, RunOutcome, RuntimeErrorInfo, TokenUsage, ToolCall, ToolDescriptor,
    ToolExecutionResult, ToolOutputChunk,
};

/// Ordered kernel event with no presentation-specific fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum AgentEvent {
    RunStarted {
        run_id: RunId,
    },
    TurnStarted {
        run_id: RunId,
        turn: u32,
        activation_turn: u32,
        max_total_turns: Option<u32>,
    },
    ModelCallStarted {
        call_id: String,
        kind: ModelCallKind,
        model: String,
        max_output_tokens: u32,
        metadata: serde_json::Value,
    },
    MessageStarted {
        message_id: MessageId,
        role: AgentRole,
    },
    MessageDelta {
        message_id: MessageId,
        delta: MessageDelta,
    },
    MessageFinished {
        message_id: MessageId,
        message: AgentMessage,
    },
    ProviderActivity {
        activity: ProviderActivity,
    },
    ToolStarted {
        call: ToolCall,
        descriptor: Option<ToolDescriptor>,
    },
    ToolProgress {
        call_id: String,
        chunks: Vec<ToolOutputChunk>,
    },
    ToolFinished {
        call_id: String,
        result: ToolExecutionResult,
        permission_label: Option<String>,
        duration: Duration,
    },
    Usage {
        call_id: String,
        kind: ModelCallKind,
        usage: Option<TokenUsage>,
        request_body: Option<Vec<u8>>,
        message_range: Option<MessageRange>,
    },
    ContextCompacted {
        kind: CompactionKind,
    },
    Notice {
        notice: AgentNotice,
    },
    TurnFinished {
        turn: u32,
        activation_turn: u32,
    },
    RunFinished {
        outcome: RunOutcome,
    },
    RunFailed {
        error: RuntimeErrorInfo,
    },
}

/// Product-neutral informational event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "notice", rename_all = "snake_case")]
pub enum AgentNotice {
    TurnLimitReached {
        max: u32,
    },
    CancellationRequested,
    RecoveryBackoff {
        attempt: u32,
        max_attempts: u32,
        delay: Duration,
        summary: String,
    },
    Continuation {
        attempt: u32,
        max_attempts: u32,
    },
    CompactionStarted {
        kind: CompactionKind,
    },
}
