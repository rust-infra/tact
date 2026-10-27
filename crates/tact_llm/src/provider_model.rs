//! `ModelPort` implementation over the existing provider adapters.

use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use tact_agent_api::{
    CancelToken, MessageDelta, MessageId, ModelCapabilities, ModelCursor, ModelError,
    ModelErrorKind, ModelEvent, ModelPort, ModelRequest, ModelResponse, ModelStream, ThinkingDelta,
    TokenUsage,
};
use tokio::sync::mpsc;

use crate::{
    CreateMessageParams, LlmClient, LlmProvider, ModelCallParams, ProviderConversationState,
    ProviderProgress, ProviderProgressSink, ThinkingChunk,
    neutral::{create_message_params, model_error, model_response, provider_state},
};

struct ProgressSink {
    tx: mpsc::Sender<Result<ModelEvent, ModelError>>,
    message_id: MessageId,
    cancel: CancelToken,
}

impl ProgressSink {
    fn send_event(&self, event: ModelEvent) {
        if !self.cancel.is_cancelled() {
            // Progress is advisory and may be dropped when the consumer falls
            // behind. The terminal response is sent with `send().await`, so a
            // slow stream cannot turn into unbounded memory growth.
            let _ = self.tx.try_send(Ok(event));
        }
    }
}

impl ProviderProgressSink for ProgressSink {
    fn send(&self, progress: ProviderProgress) {
        match progress {
            ProviderProgress::ModelInfo(ModelCallParams { .. }) => {}
            ProviderProgress::TextDelta(delta) => self.send_event(ModelEvent::MessageDelta {
                message_id: self.message_id.clone(),
                delta: MessageDelta::Text { delta },
            }),
            ProviderProgress::Thinking(chunk) => {
                let phase = match chunk {
                    ThinkingChunk::Started => ThinkingDelta::Started,
                    ThinkingChunk::Delta(delta) => ThinkingDelta::Delta { delta },
                    ThinkingChunk::Finished => ThinkingDelta::Finished,
                };
                self.send_event(ModelEvent::MessageDelta {
                    message_id: self.message_id.clone(),
                    delta: MessageDelta::Thinking { phase },
                });
            }
            ProviderProgress::Usage(usage) => {
                let usage: TokenUsage = usage.into();
                self.send_event(ModelEvent::Usage { usage });
            }
            ProviderProgress::Activity(activity) => {
                self.send_event(ModelEvent::ProviderActivity { activity });
            }
        }
    }
}

/// Provider adapter that implements the neutral model contract.
pub struct ProviderModel {
    client: LlmProvider,
    capabilities: ModelCapabilities,
    next_message_id: AtomicU64,
}

impl ProviderModel {
    pub fn new(client: LlmProvider, capabilities: ModelCapabilities) -> Self {
        Self {
            client,
            capabilities,
            next_message_id: AtomicU64::new(1),
        }
    }

    pub fn from_provider(provider: LlmProvider, capabilities: ModelCapabilities) -> Self {
        Self::new(provider, capabilities)
    }

    fn next_message_id(&self) -> MessageId {
        MessageId::new(format!(
            "model-message-{}",
            self.next_message_id.fetch_add(1, Ordering::Relaxed)
        ))
    }

    pub async fn stream_params(
        &self,
        request: CreateMessageParams,
        provider_state: Option<ProviderConversationState>,
        cancel: CancelToken,
    ) -> Result<ModelStream, ModelError> {
        if cancel.is_cancelled() {
            return Err(ModelError::new(
                ModelErrorKind::Cancelled,
                "model call cancelled before dispatch",
            ));
        }
        let client = self.client.clone();
        let message_id = self.next_message_id();
        let (tx, rx) = mpsc::channel(256);
        let stream_message_id = message_id.clone();
        let task_cancel = cancel.clone();
        tokio::spawn(async move {
            if tx
                .send(Ok(ModelEvent::MessageStarted {
                    message_id: stream_message_id.clone(),
                    role: tact_agent_api::AgentRole::Assistant,
                }))
                .await
                .is_err()
            {
                return;
            }
            let sink = ProgressSink {
                tx: tx.clone(),
                message_id: stream_message_id.clone(),
                cancel: task_cancel.clone(),
            };
            let result = tokio::select! {
                biased;
                result = client.stream_message(&request, provider_state.as_ref(), &sink) => {
                    result.map_err(model_error)
                }
                _ = task_cancel.cancelled() => Err(ModelError::new(
                    ModelErrorKind::Cancelled,
                    "model call cancelled",
                )),
            };
            match result {
                Ok(response) => {
                    let terminal = model_response(response, stream_message_id);
                    let _ = tx.send(Ok(ModelEvent::Finished(terminal))).await;
                }
                Err(error) => {
                    let _ = tx.send(Err(error)).await;
                }
            }
        });
        Ok(Box::pin(futures_util::stream::unfold(
            rx,
            |mut rx| async move { rx.recv().await.map(|item| (item, rx)) },
        )))
    }

    pub async fn complete_params(
        &self,
        request: CreateMessageParams,
        provider_state: Option<ProviderConversationState>,
        cancel: CancelToken,
    ) -> Result<ModelResponse, ModelError> {
        let message_id = self.next_message_id();
        tokio::select! {
            biased;
            result = self.client.create_message(&request, provider_state.as_ref()) => {
                result
                    .map(|response| model_response(response, message_id))
                    .map_err(model_error)
            },
            _ = cancel.cancelled() => Err(ModelError::new(
                ModelErrorKind::Cancelled,
                "model call cancelled",
            )),
        }
    }

    pub async fn compact_params(
        &self,
        request: CreateMessageParams,
        provider_state: Option<ProviderConversationState>,
        cancel: CancelToken,
    ) -> Result<ModelResponse, ModelError> {
        let message_id = self.next_message_id();
        tokio::select! {
            biased;
            result = self.client.compact(&request, provider_state.as_ref()) => {
                result
                    .map(|response| model_response(response, message_id))
                    .map_err(model_error)
            },
            _ = cancel.cancelled() => Err(ModelError::new(
                ModelErrorKind::Cancelled,
                "model call cancelled",
            )),
        }
    }
}

#[async_trait]
impl ModelPort for ProviderModel {
    fn capabilities(&self) -> ModelCapabilities {
        self.capabilities.clone()
    }

    async fn stream(
        &self,
        request: ModelRequest,
        cursor: Option<&ModelCursor>,
        cancel: CancelToken,
    ) -> Result<ModelStream, ModelError> {
        let params = create_message_params(&request)?;
        let state = cursor.map(provider_state).transpose()?;
        self.stream_params(params, state, cancel).await
    }

    async fn complete(
        &self,
        request: ModelRequest,
        cursor: Option<&ModelCursor>,
        cancel: CancelToken,
    ) -> Result<ModelResponse, ModelError> {
        let params = create_message_params(&request)?;
        let state = cursor.map(provider_state).transpose()?;
        self.complete_params(params, state, cancel).await
    }

    async fn compact(
        &self,
        request: ModelRequest,
        cursor: Option<&ModelCursor>,
        cancel: CancelToken,
    ) -> Result<ModelResponse, ModelError> {
        let params = create_message_params(&request)?;
        let state = cursor.map(provider_state).transpose()?;
        self.compact_params(params, state, cancel).await
    }
}

#[cfg(test)]
mod tests {
    use futures_util::StreamExt;
    use tact_agent_api::{
        AgentContentBlock, AgentMessage, AgentRole, MessageOrigin, ModelEvent, ModelMessage,
        ModelPort, ModelRequest, StopReason, ThinkingDelta,
    };

    use crate::{ContentBlock, MockClient, StopReason as LlmStopReason};

    use super::*;

    fn model() -> ProviderModel {
        let client = MockClient::new(vec![(
            vec![
                ContentBlock::Thinking {
                    thinking: "plan".into(),
                    signature: "sig".into(),
                },
                ContentBlock::Text {
                    text: "answer".into(),
                },
            ],
            Some(LlmStopReason::EndTurn),
        )])
        .with_streaming_chunks();
        ProviderModel::new(
            LlmProvider::Mock(client),
            ModelCapabilities {
                model: "mock".into(),
                context_window: 1_000,
                max_output_tokens: 100,
                supports_streaming: true,
                supports_native_compaction: true,
            },
        )
    }

    fn request() -> ModelRequest {
        ModelRequest {
            model: "mock".into(),
            system: "system".into(),
            messages: vec![ModelMessage {
                role: AgentRole::User,
                content: vec![AgentContentBlock::Text {
                    text: "hello".into(),
                }],
            }],
            tools: Vec::new(),
            max_output_tokens: 100,
            options: tact_agent_api::ModelCallOptions::default(),
        }
    }

    #[tokio::test]
    async fn stream_ends_with_exactly_one_terminal_response() {
        let model = model();
        let mut stream = model
            .stream(request(), None, CancelToken::new())
            .await
            .unwrap();
        let mut events = Vec::new();
        while let Some(event) = stream.next().await {
            events.push(event.unwrap());
        }
        assert!(matches!(
            events.first(),
            Some(ModelEvent::MessageStarted { .. })
        ));
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, ModelEvent::Finished(_)))
                .count(),
            1
        );
        let ModelEvent::Finished(response) = events.last().unwrap() else {
            panic!("last event must be terminal");
        };
        assert_eq!(response.stop_reason, StopReason::EndTurn);
        assert_eq!(
            response.message,
            AgentMessage::new(
                AgentRole::Assistant,
                MessageOrigin::Assistant,
                vec![
                    AgentContentBlock::Thinking {
                        thinking: "plan".into(),
                        signature: "sig".into(),
                    },
                    AgentContentBlock::Text {
                        text: "answer".into(),
                    },
                ],
            )
        );
        assert!(events.iter().any(|event| matches!(
            event,
            ModelEvent::MessageDelta {
                delta: MessageDelta::Thinking {
                    phase: ThinkingDelta::Started
                },
                ..
            }
        )));
    }

    #[tokio::test]
    async fn complete_preserves_usage_and_stop_reason() {
        let usage = crate::TokenUsageInfo {
            prompt: 7,
            completion: 3,
            total: 10,
            prompt_cache_hit_tokens: 2,
            prompt_cache_miss_tokens: 5,
            reasoning_tokens: 4,
        };
        let model = ProviderModel::new(
            LlmProvider::Mock(MockClient::with_usage(vec![(
                vec![ContentBlock::Text {
                    text: "done".into(),
                }],
                Some(LlmStopReason::ToolUse),
                usage.clone(),
            )])),
            ModelCapabilities {
                model: "mock".into(),
                context_window: 1_000,
                max_output_tokens: 100,
                supports_streaming: true,
                supports_native_compaction: true,
            },
        );

        let response = model
            .complete(request(), None, CancelToken::new())
            .await
            .unwrap();
        assert_eq!(response.stop_reason, StopReason::ToolUse);
        assert_eq!(response.usage, Some(usage.into()));
        assert!(response.request_body.is_none());
        assert_eq!(response.cursor, tact_agent_api::CursorUpdate::Unchanged);
    }

    #[tokio::test]
    async fn native_compact_returns_the_same_complete_response_metadata() {
        let usage = crate::TokenUsageInfo {
            prompt: 4,
            completion: 1,
            total: 5,
            ..crate::TokenUsageInfo::default()
        };
        let model = ProviderModel::new(
            LlmProvider::Mock(MockClient::with_usage(vec![(
                vec![ContentBlock::Text {
                    text: "compacted".into(),
                }],
                Some(LlmStopReason::EndTurn),
                usage.clone(),
            )])),
            ModelCapabilities {
                model: "mock".into(),
                context_window: 1_000,
                max_output_tokens: 100,
                supports_streaming: true,
                supports_native_compaction: true,
            },
        );

        let response = model
            .compact(request(), None, CancelToken::new())
            .await
            .unwrap();
        assert_eq!(response.stop_reason, StopReason::EndTurn);
        assert_eq!(response.usage, Some(usage.into()));
        assert_eq!(
            response.message.content,
            vec![AgentContentBlock::Text {
                text: "compacted".into(),
            }]
        );
    }

    #[tokio::test]
    async fn unsupported_native_compaction_stays_typed() {
        let model = ProviderModel::new(
            LlmProvider::Anthropic(crate::anthropic::AnthropicAdapter::new(
                "sk-test",
                "https://api.anthropic.com",
            )),
            ModelCapabilities {
                model: "claude".into(),
                context_window: 1_000,
                max_output_tokens: 100,
                supports_streaming: true,
                supports_native_compaction: false,
            },
        );

        let error = model
            .compact(request(), None, CancelToken::new())
            .await
            .expect_err("Anthropic has no native compaction endpoint");
        assert_eq!(error.kind, ModelErrorKind::Unsupported);
    }
}
