mod convert;
mod history;
mod normalize;
mod stream;

use async_openai_responses::{
    Client,
    config::OpenAIConfig,
    types::responses::{Response, ResponseStreamEvent},
};
use futures_util::StreamExt;
use tact_protocol::{AgentUpdate, TokenUsageInfo};
use tokio::sync::mpsc::UnboundedSender;

use crate::{
    ContentBlock, CreateMessageParams, LlmClient, LlmError, LlmRequestBody, OpenAiReasoningEffort,
    StopReason,
};

use self::{convert::create_response, normalize::NormalizedResponse, stream::ResponsesStreamState};

/// OpenAI Responses API adapter backed by async-openai 0.41.x.
#[derive(Clone)]
pub struct OpenAiResponsesAdapter {
    client: Client<OpenAIConfig>,
    base_url: String,
    reasoning_effort: Option<OpenAiReasoningEffort>,
}

impl OpenAiResponsesAdapter {
    pub fn new(
        api_key: impl Into<String>,
        base_url: impl Into<String>,
        reasoning_effort: Option<OpenAiReasoningEffort>,
    ) -> Self {
        let base_url = base_url.into().trim_end_matches('/').to_string();
        let config = OpenAIConfig::new()
            .with_api_key(api_key)
            .with_api_base(base_url.clone())
            .with_org_id("")
            .with_project_id("");
        Self {
            client: Client::with_config(config),
            base_url,
            reasoning_effort,
        }
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    fn into_result(
        normalized: NormalizedResponse,
        request_body: LlmRequestBody,
    ) -> (
        Vec<ContentBlock>,
        Option<StopReason>,
        Option<TokenUsageInfo>,
        Option<LlmRequestBody>,
    ) {
        (
            normalized.blocks,
            normalized.stop_reason,
            normalized.usage,
            Some(request_body),
        )
    }
}

#[async_trait::async_trait]
impl LlmClient for OpenAiResponsesAdapter {
    async fn stream_message(
        &self,
        request: &CreateMessageParams,
        ui_tx: Option<UnboundedSender<AgentUpdate>>,
    ) -> Result<
        (
            Vec<ContentBlock>,
            Option<StopReason>,
            Option<TokenUsageInfo>,
            Option<LlmRequestBody>,
        ),
        LlmError,
    > {
        let mut wire_request = create_response(request, self.reasoning_effort)?;
        wire_request["stream"] = serde_json::Value::Bool(true);
        let request_body = serde_json::to_vec(&wire_request)
            .map_err(|error| LlmError::Other(format!("serialize Responses request: {error}")))?;
        let mut response_stream = self
            .client
            .responses()
            .create_stream_byot::<_, ResponseStreamEvent>(wire_request)
            .await
            .map_err(LlmError::from)?;
        let mut state = ResponsesStreamState::default();

        while let Some(result) = response_stream.next().await {
            let event = match result {
                Ok(event) => event,
                Err(error) => {
                    if let Some(update) = state.close_thinking()
                        && let Some(tx) = &ui_tx
                    {
                        let _ = tx.send(update);
                    }
                    return Err(LlmError::from(error));
                }
            };
            let updates = match state.apply(event) {
                Ok(updates) => updates,
                Err(error) => {
                    if let Some(update) = state.close_thinking()
                        && let Some(tx) = &ui_tx
                    {
                        let _ = tx.send(update);
                    }
                    return Err(error);
                }
            };
            for update in updates {
                if let Some(tx) = &ui_tx {
                    let _ = tx.send(update);
                }
            }
        }

        if let Some(update) = state.close_thinking()
            && let Some(tx) = &ui_tx
        {
            let _ = tx.send(update);
        }
        let normalized = state.finish()?;
        if let Some(usage) = &normalized.usage
            && let Some(tx) = &ui_tx
        {
            let _ = tx.send(AgentUpdate::TokenUsage(usage.clone()));
        }
        Ok(Self::into_result(normalized, request_body))
    }

    async fn create_message(
        &self,
        request: &CreateMessageParams,
    ) -> Result<
        (
            Vec<ContentBlock>,
            Option<StopReason>,
            Option<TokenUsageInfo>,
            Option<LlmRequestBody>,
        ),
        LlmError,
    > {
        let mut wire_request = create_response(request, self.reasoning_effort)?;
        wire_request["stream"] = serde_json::Value::Bool(false);
        let request_body = serde_json::to_vec(&wire_request)
            .map_err(|error| LlmError::Other(format!("serialize Responses request: {error}")))?;
        let response = self
            .client
            .responses()
            .create_byot::<_, Response>(wire_request)
            .await
            .map_err(LlmError::from)?;
        Ok(Self::into_result(
            normalize::normalize_response(response)?,
            request_body,
        ))
    }
}
