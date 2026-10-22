//! Shared OpenAI-compatible `LlmClient` wiring: assemble body → transport.

use serde_json::Value;
use tact_protocol::UpdateSink;

use super::OpenAiAdapter;
use crate::{CreateMessageParams, LlmError, LlmResponse, ProviderConversationState};

pub(crate) async fn stream_assembled(
    adapter: &OpenAiAdapter,
    request: &CreateMessageParams,
    provider_state: Option<&ProviderConversationState>,
    events: &UpdateSink,
    assemble: impl FnOnce(&CreateMessageParams, bool) -> Result<Value, LlmError>,
) -> Result<LlmResponse, LlmError> {
    let body = assemble(request, true)?;
    adapter
        .stream_completion(&body, provider_state, events)
        .await
}

pub(crate) async fn create_assembled(
    adapter: &OpenAiAdapter,
    request: &CreateMessageParams,
    provider_state: Option<&ProviderConversationState>,
    assemble: impl FnOnce(&CreateMessageParams, bool) -> Result<Value, LlmError>,
) -> Result<LlmResponse, LlmError> {
    let body = assemble(request, false)?;
    adapter.create_completion(&body, provider_state).await
}
