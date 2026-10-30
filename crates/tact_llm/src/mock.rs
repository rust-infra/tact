//! Deterministic mock LLM client for tests.

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use crate::client::{LlmClient, LlmResponse};
use crate::error::LlmError;
use crate::provider_state::{ProviderConversationState, ProviderStateUpdate};
use crate::{
    ContentBlock, CreateMessageParams, ProviderProgress, ProviderProgressSink, StopReason,
    ThinkingChunk, TokenUsageInfo,
};

/// A single canned LLM turn for [`MockClient`].
struct MockTurn {
    blocks: Vec<ContentBlock>,
    stop_reason: Option<StopReason>,
    usage: Option<TokenUsageInfo>,
}

type MockTurnResult = Result<
    (
        Vec<ContentBlock>,
        Option<StopReason>,
        Option<TokenUsageInfo>,
    ),
    LlmError,
>;

/// Backing behavior for [`MockClient`].
trait MockClientInner: Send + Sync {
    /// Produce the next turn. `idx` is the 0-based call counter.
    fn next_turn(&self, request: &CreateMessageParams, idx: usize) -> MockTurnResult;
}

struct CannedMockInner {
    responses: Vec<MockTurn>,
}

impl MockClientInner for CannedMockInner {
    fn next_turn(
        &self,
        _request: &CreateMessageParams,
        idx: usize,
    ) -> Result<
        (
            Vec<ContentBlock>,
            Option<StopReason>,
            Option<TokenUsageInfo>,
        ),
        LlmError,
    > {
        let turn = &self.responses[idx % self.responses.len()];
        Ok((
            turn.blocks.clone(),
            clone_stop_reason(&turn.stop_reason),
            turn.usage.clone(),
        ))
    }
}

struct DynamicMockInner<F> {
    responder: F,
}

impl<F> MockClientInner for DynamicMockInner<F>
where
    F: Fn(
            &CreateMessageParams,
            usize,
        ) -> Result<
            (
                Vec<ContentBlock>,
                Option<StopReason>,
                Option<TokenUsageInfo>,
            ),
            LlmError,
        > + Send
        + Sync,
{
    fn next_turn(
        &self,
        request: &CreateMessageParams,
        idx: usize,
    ) -> Result<
        (
            Vec<ContentBlock>,
            Option<StopReason>,
            Option<TokenUsageInfo>,
        ),
        LlmError,
    > {
        (self.responder)(request, idx)
    }
}

fn clone_stop_reason(stop_reason: &Option<StopReason>) -> Option<StopReason> {
    stop_reason.clone()
}

fn clone_llm_error(e: &LlmError) -> LlmError {
    LlmError::Mock(e.to_string())
}

/// Deterministic mock LLM client that returns scripted or dynamic responses.
///
/// Supports:
/// - Fixed sequences of turns (`new`, `with_usage`)
/// - Dynamic request-aware responses (`with_responder`)
/// - Turn-by-turn error injection (`with_error`)
/// - Optional streaming `StreamChunk` emission (`with_streaming_chunks`)
#[derive(Clone)]
pub struct MockClient {
    inner: Arc<dyn MockClientInner + Send + Sync>,
    counter: Arc<AtomicUsize>,
    emit_stream_chunks: bool,
}

impl MockClient {
    /// Create a mock client that cycles through the given responses.
    ///
    /// Each tuple provides content blocks and a stop reason. Token usage and
    /// the serialised request body are always `None`.
    pub fn new(responses: Vec<(Vec<ContentBlock>, Option<StopReason>)>) -> Self {
        Self::with_inner(
            Arc::new(CannedMockInner {
                responses: responses
                    .into_iter()
                    .map(|(blocks, stop_reason)| MockTurn {
                        blocks,
                        stop_reason,
                        usage: None,
                    })
                    .collect(),
            }),
            false,
        )
    }

    /// Like [`Self::new`], but attaches token usage to each turn (and emits
    /// [`ProviderProgress::Usage`] on `stream_message` when a sink is attached).
    pub fn with_usage(
        responses: Vec<(Vec<ContentBlock>, Option<StopReason>, TokenUsageInfo)>,
    ) -> Self {
        Self::with_inner(
            Arc::new(CannedMockInner {
                responses: responses
                    .into_iter()
                    .map(|(blocks, stop_reason, usage)| MockTurn {
                        blocks,
                        stop_reason,
                        usage: Some(usage),
                    })
                    .collect(),
            }),
            false,
        )
    }

    /// Create a mock client driven by a closure.
    ///
    /// The closure receives the full LLM request and the 0-based call counter,
    /// and returns either a successful turn `(blocks, stop_reason, usage)` or
    /// an [`LlmError`]. This makes it possible to assert on the request body,
    /// branch on previous tool results, and inject failures.
    pub fn with_responder<F>(responder: F) -> Self
    where
        F: Fn(
                &CreateMessageParams,
                usize,
            ) -> Result<
                (
                    Vec<ContentBlock>,
                    Option<StopReason>,
                    Option<TokenUsageInfo>,
                ),
                LlmError,
            > + Send
            + Sync
            + 'static,
    {
        Self::with_inner(Arc::new(DynamicMockInner { responder }), false)
    }

    /// Create a mock client where the given errors are returned in order.
    ///
    /// If a call exceeds the error list, the client falls back to an empty
    /// successful turn.
    pub fn with_error(errors: Vec<LlmError>) -> Self {
        Self::with_responder(move |_request, idx| {
            errors
                .get(idx)
                .map(|e| Err(clone_llm_error(e)))
                .unwrap_or_else(|| Ok((vec![], None, None)))
        })
    }

    /// Enable emission of [`ProviderProgress::TextDelta`] events during
    /// `stream_message` by splitting text blocks into word-sized chunks.
    pub fn with_streaming_chunks(self) -> Self {
        Self {
            emit_stream_chunks: true,
            ..self
        }
    }

    fn with_inner(inner: Arc<dyn MockClientInner + Send + Sync>, emit_stream_chunks: bool) -> Self {
        Self {
            inner,
            counter: Arc::new(AtomicUsize::new(0)),
            emit_stream_chunks,
        }
    }

    fn next_turn(&self, request: &CreateMessageParams) -> MockTurnResult {
        let idx = self.counter.fetch_add(1, Ordering::Relaxed);
        self.inner.next_turn(request, idx)
    }

    fn emit_token_usage(events: &dyn ProviderProgressSink, usage: &TokenUsageInfo) {
        events.send(ProviderProgress::Usage(usage.clone()));
    }

    fn emit_stream_chunks(events: &dyn ProviderProgressSink, blocks: &[ContentBlock]) {
        for block in blocks {
            match block {
                ContentBlock::Thinking { thinking, .. } => {
                    events.send(ProviderProgress::Thinking(ThinkingChunk::Started));
                    if !thinking.is_empty() {
                        events.send(ProviderProgress::Thinking(ThinkingChunk::Delta(
                            thinking.clone(),
                        )));
                    }
                    events.send(ProviderProgress::Thinking(ThinkingChunk::Finished));
                }
                ContentBlock::Text { text } => {
                    // Emit word-by-word to simulate streaming without overloading the channel.
                    let words: Vec<&str> = text.split_whitespace().collect();
                    let n = words.len();
                    for (i, word) in words.into_iter().enumerate() {
                        let chunk = if i + 1 == n {
                            word.to_string()
                        } else {
                            format!("{word} ")
                        };
                        events.send(ProviderProgress::TextDelta(chunk));
                    }
                }
                _ => {}
            }
        }
    }
}

impl LlmClient for MockClient {
    async fn stream_message(
        &self,
        request: &CreateMessageParams,
        _provider_state: Option<&ProviderConversationState>,
        events: &dyn ProviderProgressSink,
    ) -> Result<LlmResponse, LlmError> {
        let (blocks, stop_reason, usage) = self.next_turn(request)?;
        if let Some(ref u) = usage {
            Self::emit_token_usage(events, u);
        }
        if self.emit_stream_chunks {
            Self::emit_stream_chunks(events, &blocks);
        }
        Ok(LlmResponse {
            blocks,
            stop_reason,
            usage,
            request_body: None,
            state_update: ProviderStateUpdate::Unchanged,
        })
    }

    async fn create_message(
        &self,
        request: &CreateMessageParams,
        _provider_state: Option<&ProviderConversationState>,
    ) -> Result<LlmResponse, LlmError> {
        let (blocks, stop_reason, usage) = self.next_turn(request)?;
        Ok(LlmResponse {
            blocks,
            stop_reason,
            usage,
            request_body: None,
            state_update: ProviderStateUpdate::Unchanged,
        })
    }

    /// Native mock compaction: returns the next scripted turn.
    ///
    /// Keeps the mock turn counter semantics shared with `stream_message` /
    /// `create_message` so compaction participates in scripted sequences.
    async fn compact(
        &self,
        request: &CreateMessageParams,
        _provider_state: Option<&ProviderConversationState>,
    ) -> Result<LlmResponse, LlmError> {
        let (blocks, stop_reason, usage) = self.next_turn(request)?;
        Ok(LlmResponse {
            blocks,
            stop_reason,
            usage,
            request_body: None,
            state_update: ProviderStateUpdate::Unchanged,
        })
    }
}
