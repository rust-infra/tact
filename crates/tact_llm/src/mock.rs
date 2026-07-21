//! Deterministic mock LLM client for tests.

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use tact_protocol::{AgentUpdate, TokenUsageInfo};
use tokio::sync::mpsc::UnboundedSender;

use crate::{ContentBlock, CreateMessageParams, LlmClient, LlmError, LlmRequestBody, StopReason};

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
    LlmError::Other(e.to_string())
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
    /// [`AgentUpdate::TokenUsage`] on `stream_message` when `ui_tx` is set).
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

    /// Enable emission of [`AgentUpdate::StreamChunk`] events during
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

    fn emit_token_usage(ui_tx: &Option<UnboundedSender<AgentUpdate>>, usage: &TokenUsageInfo) {
        if let Some(tx) = ui_tx {
            let _ = tx.send(AgentUpdate::TokenUsage(usage.clone()));
        }
    }

    fn emit_stream_chunks(ui_tx: &Option<UnboundedSender<AgentUpdate>>, blocks: &[ContentBlock]) {
        let Some(tx) = ui_tx else { return };
        for block in blocks {
            if let ContentBlock::Text { text } = block {
                // Emit word-by-word to simulate streaming without overloading the channel.
                let words: Vec<&str> = text.split_whitespace().collect();
                let n = words.len();
                for (i, word) in words.into_iter().enumerate() {
                    let chunk = if i + 1 == n {
                        word.to_string()
                    } else {
                        format!("{word} ")
                    };
                    let _ = tx.send(AgentUpdate::StreamChunk(chunk));
                }
            }
        }
    }
}

#[async_trait::async_trait]
impl LlmClient for MockClient {
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
        let (blocks, stop_reason, usage) = self.next_turn(request)?;
        if let Some(ref u) = usage {
            Self::emit_token_usage(&ui_tx, u);
        }
        if self.emit_stream_chunks {
            Self::emit_stream_chunks(&ui_tx, &blocks);
        }
        Ok((blocks, stop_reason, usage, None))
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
        let (blocks, stop_reason, usage) = self.next_turn(request)?;
        Ok((blocks, stop_reason, usage, None))
    }
}
