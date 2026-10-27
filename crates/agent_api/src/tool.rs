use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::{AgentContentBlock, CancelToken, HostYieldReason, SessionRef};

/// Tool specification exposed to the model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

/// Output stream classification for live tool progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolOutputStream {
    Stdout,
    Stderr,
    Other,
}

/// One ordered chunk of live tool output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolOutputChunk {
    pub stream: ToolOutputStream,
    pub text: String,
}

/// Raw tool call exactly as produced by the model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub input: serde_json::Value,
}

/// Host-visible description of a resolved tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolDescriptor {
    pub name: String,
    pub description: String,
    pub metadata: serde_json::Value,
}

/// Opaque resource namespace used for scheduling.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ResourceKey {
    pub namespace: String,
    pub components: Vec<String>,
}

impl ResourceKey {
    pub fn new(namespace: impl Into<String>, components: Vec<String>) -> Self {
        Self {
            namespace: namespace.into(),
            components,
        }
    }
}

/// One resource scope owned by a tool call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceScope {
    pub key: ResourceKey,
    pub covers_descendants: bool,
}

impl ResourceScope {
    pub fn exact(key: ResourceKey) -> Self {
        Self {
            key,
            covers_descendants: false,
        }
    }

    pub fn descendants(key: ResourceKey) -> Self {
        Self {
            key,
            covers_descendants: true,
        }
    }
}

/// Resources read, written, or otherwise touched by a resolved call.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceAccess {
    pub reads: Vec<ResourceScope>,
    pub writes: Vec<ResourceScope>,
    pub barrier: bool,
}

impl ResourceAccess {
    pub fn independent() -> Self {
        Self::default()
    }

    pub fn barrier() -> Self {
        Self {
            barrier: true,
            ..Self::default()
        }
    }
}

/// Tool call after adapter validation, resource resolution, and optional hook
/// mutation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedToolCall {
    pub call_id: String,
    pub name: String,
    pub input: serde_json::Value,
    pub access: ResourceAccess,
    pub descriptor: ToolDescriptor,
}

/// Snapshot returned before each request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolSnapshot {
    pub specs: Vec<ToolSpec>,
    pub generation: u64,
}

/// Ordered live-output sink supplied by the kernel.
pub trait ToolProgressSink: Send + Sync {
    fn emit(&self, chunks: Vec<ToolOutputChunk>);
}

/// Context passed to a tool execution adapter.
#[derive(Clone)]
pub struct ToolExecutionContext {
    pub call_id: String,
    pub progress: Arc<dyn ToolProgressSink>,
    pub cancel: CancelToken,
    /// Model calls permitted from the current turn onward, inclusive.
    pub remaining_turns: Option<u32>,
}

/// Typed tool result status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolExecutionStatus {
    Succeeded,
    Failed,
}

/// Stable source of an operational tool failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolExecutionErrorKind {
    Tool,
    OutputPersistence,
    Mcp,
    HostInteraction,
    Adapter,
}

/// Original operational error retained even when visible output is rewritten.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolExecutionError {
    pub kind: ToolExecutionErrorKind,
    pub message: String,
}

/// Product-neutral tool directives.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ToolDirective {
    RequestCompaction { focus: Option<String> },
    YieldToHost { reason: HostYieldReason },
    CompleteRun,
}

/// Logical result of a successful adapter call, including ordinary tool
/// failures.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolOutcome {
    pub content: Vec<AgentContentBlock>,
    pub directives: Vec<ToolDirective>,
    pub metadata: Option<serde_json::Value>,
}

impl ToolOutcome {
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            content: vec![AgentContentBlock::Text { text: text.into() }],
            ..Self::default()
        }
    }
}

/// A tool result. `Ok(ToolExecutionResult)` may represent an ordinary failed
/// tool; `Err(ToolError)` is an adapter/contract failure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolExecutionResult {
    pub status: ToolExecutionStatus,
    pub outcome: ToolOutcome,
    pub error: Option<ToolExecutionError>,
}

impl ToolExecutionResult {
    pub fn success(outcome: ToolOutcome) -> Self {
        Self {
            status: ToolExecutionStatus::Succeeded,
            outcome,
            error: None,
        }
    }

    pub fn failure(error: ToolExecutionError) -> Self {
        Self {
            status: ToolExecutionStatus::Failed,
            outcome: ToolOutcome::text(error.message.clone()),
            error: Some(error),
        }
    }
}

/// Closed tool adapter error classes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolErrorKind {
    UnknownTool,
    InvalidInput,
    Refresh,
    Adapter,
}

/// Contract or adapter failure returned by `ToolPort`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[error("{message}")]
pub struct ToolError {
    pub kind: ToolErrorKind,
    pub message: String,
}

impl ToolError {
    pub fn new(kind: ToolErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

/// Read-only tool resolution capability used by policy and hook adapters.
pub trait ToolResolver: Send + Sync {
    fn resolve(&self, call: &ToolCall) -> Result<ResolvedToolCall, ToolError>;
}

/// Required tool capability.
#[async_trait]
pub trait ToolPort: ToolResolver {
    async fn refresh(&self, cancel: CancelToken) -> Result<ToolSnapshot, ToolError>;

    async fn execute(
        &self,
        call: ResolvedToolCall,
        context: ToolExecutionContext,
    ) -> Result<ToolExecutionResult, ToolError>;
}

/// Context available to permission and hook policy.
#[derive(Debug, Clone)]
pub struct ToolGateContext {
    pub session: SessionRef,
    /// Cumulative main-loop request attempts dispatched by this logical agent.
    pub total_turns: u32,
    pub remaining_turns: Option<u32>,
    pub metadata: serde_json::Value,
}

/// Final pre-execution decision after all hook and permission rewrites.
#[derive(Debug, Clone)]
pub enum GateDecision {
    Allow {
        call: Box<ResolvedToolCall>,
        label: Option<String>,
    },
    Deny {
        reason: String,
    },
}

/// Permission/hook adapter failure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
pub enum ToolGateError {
    #[error("tool gate failed: {0}")]
    Failed(String),
    #[error("tool gate cancelled")]
    Cancelled,
}

/// Required permission and hook capability.
#[async_trait]
pub trait ToolGate: Send + Sync {
    async fn before(
        &self,
        call: ToolCall,
        resolver: &dyn ToolResolver,
        context: &ToolGateContext,
        cancel: CancelToken,
    ) -> Result<GateDecision, ToolGateError>;

    async fn after(
        &self,
        call: &ResolvedToolCall,
        result: ToolExecutionResult,
        context: &ToolGateContext,
        cancel: CancelToken,
    ) -> Result<ToolExecutionResult, ToolGateError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_directive_round_trips() {
        let values = [
            ToolDirective::RequestCompaction {
                focus: Some("tests".into()),
            },
            ToolDirective::YieldToHost {
                reason: HostYieldReason::NeedsUser,
            },
            ToolDirective::CompleteRun,
        ];
        for value in values {
            let json = serde_json::to_string(&value).unwrap();
            assert_eq!(serde_json::from_str::<ToolDirective>(&json).unwrap(), value);
        }
    }

    #[test]
    fn output_chunk_uses_the_neutral_stream_enum() {
        let chunk = ToolOutputChunk {
            stream: ToolOutputStream::Stderr,
            text: "warning".into(),
        };
        let json = serde_json::to_string(&chunk).unwrap();
        assert_eq!(
            serde_json::from_str::<ToolOutputChunk>(&json).unwrap(),
            chunk
        );
    }
}
