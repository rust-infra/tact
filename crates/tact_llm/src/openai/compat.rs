//! Shared OpenAI-compatible `LlmClient` wiring: assemble body → transport.

use serde_json::Value;
use tact_protocol::{AgentUpdate, TokenUsageInfo};
use tokio::sync::mpsc::UnboundedSender;

use super::OpenAiAdapter;
use crate::{ContentBlock, CreateMessageParams, LlmError, LlmRequestBody, StopReason};

pub(crate) async fn stream_assembled(
    adapter: &OpenAiAdapter,
    request: &CreateMessageParams,
    ui_tx: Option<UnboundedSender<AgentUpdate>>,
    assemble: impl FnOnce(&CreateMessageParams, bool) -> Result<Value, LlmError>,
) -> Result<(Vec<ContentBlock>, Option<StopReason>, Option<TokenUsageInfo>, Option<LlmRequestBody>), LlmError> {
    let body = assemble(request, true)?;
    adapter.stream_completion(&body, ui_tx).await
}

pub(crate) async fn create_assembled(
    adapter: &OpenAiAdapter,
    request: &CreateMessageParams,
    assemble: impl FnOnce(&CreateMessageParams, bool) -> Result<Value, LlmError>,
) -> Result<(Vec<ContentBlock>, Option<StopReason>, Option<TokenUsageInfo>, Option<LlmRequestBody>), LlmError> {
    let body = assemble(request, false)?;
    adapter.create_completion(&body).await
}
