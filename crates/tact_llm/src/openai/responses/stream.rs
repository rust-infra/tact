use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

use async_openai_responses::types::responses::{
    OutputItem, Response, ResponseStreamEvent, WebSearchCallStatus, WebSearchToolCall,
};
use tact_agent_api::{ProviderActivity, ProviderActivityKind, ProviderActivityPhase};

use super::normalize::{NormalizedResponse, normalize_response};
use crate::error::LlmError;
use crate::{ProviderProgress, ThinkingChunk};

/// Extracts the search query from a `WebSearchToolCall` (empty when the
/// provider has not populated the action yet — `output_item.added` events
/// can precede the action).
#[allow(deprecated)]
fn web_search_query(call: &WebSearchToolCall) -> String {
    match &call.action {
        Some(async_openai_responses::types::responses::WebSearchToolCallAction::Search(search)) => {
            search
                .queries
                .as_ref()
                .and_then(|queries| queries.first())
                .cloned()
                .or_else(|| search.query.clone())
                .unwrap_or_default()
        }
        _ => String::new(),
    }
}

/// Builds the diagnostic detail for a failed or non-terminal web search
/// call: the reported status, the query, and the provider's raw action JSON
/// (truncated) so a failure shows what the provider actually returned instead
/// of a bare "web search failed".
fn web_search_failure_detail(call: &WebSearchToolCall, query: &str) -> String {
    const MAX_ACTION_CHARS: usize = 300;
    let mut action = call
        .action
        .as_ref()
        .map(|action| {
            serde_json::to_string(action).unwrap_or_else(|_| "<unserializable>".to_string())
        })
        .unwrap_or_else(|| "none".to_string());
    if action.len() > MAX_ACTION_CHARS {
        action.truncate(MAX_ACTION_CHARS);
        action.push('…');
    }
    format!(
        "status: {:?}, query: {query:?}, action: {action}",
        call.status
    )
}

/// Builds the `StepStarted` update for an in-progress web search call.
fn web_search_activity_id(output_index: u32, call: &WebSearchToolCall) -> String {
    if call.id.is_empty() {
        format!("web-search-{output_index}")
    } else {
        call.id.clone()
    }
}

fn web_search_activity(
    output_index: u32,
    index: u32,
    call: &WebSearchToolCall,
    phase: ProviderActivityPhase,
    failure: Option<String>,
    duration_us: Option<u64>,
) -> ProviderProgress {
    let query = web_search_query(call);
    ProviderProgress::Activity(ProviderActivity {
        id: web_search_activity_id(output_index, call),
        step: index,
        kind: ProviderActivityKind::WebSearch {
            phase,
            query: (!query.is_empty()).then_some(query),
            failure,
            duration: duration_us.map(std::time::Duration::from_micros),
        },
    })
}

fn web_search_started(output_index: u32, index: u32, call: &WebSearchToolCall) -> ProviderProgress {
    web_search_activity(
        output_index,
        index,
        call,
        ProviderActivityPhase::Started,
        None,
        None,
    )
}

/// Builds the terminal progress for a completed or failed web search call.
fn web_search_finished(
    output_index: u32,
    index: u32,
    call: &WebSearchToolCall,
    duration_us: Option<u64>,
) -> ProviderProgress {
    let query = web_search_query(call);
    match call.status {
        WebSearchCallStatus::Failed => web_search_activity(
            output_index,
            index,
            call,
            ProviderActivityPhase::Failed,
            Some(format!(
                "web search failed ({})",
                web_search_failure_detail(call, &query)
            )),
            duration_us,
        ),
        WebSearchCallStatus::InProgress
        | WebSearchCallStatus::Searching
        | WebSearchCallStatus::Incomplete => web_search_activity(
            output_index,
            index,
            call,
            ProviderActivityPhase::Failed,
            Some(format!(
                "web search ended without terminal status ({})",
                web_search_failure_detail(call, &query)
            )),
            duration_us,
        ),
        WebSearchCallStatus::Completed => web_search_activity(
            output_index,
            index,
            call,
            ProviderActivityPhase::Completed,
            None,
            duration_us,
        ),
    }
}

#[derive(Default)]
pub(crate) struct ResponsesStreamState {
    thinking_open: bool,
    output_text: String,
    /// Number of reasoning deltas seen (`reasoning_summary_text.delta` /
    /// `reasoning_text.delta`).
    ///
    /// They are forwarded to the TUI but never accumulated as visible text, so
    /// without this counter `finish()` cannot tell "the endpoint sent only
    /// reasoning" from "the endpoint sent nothing" — and those two need
    /// different diagnostics (the first is a compatible endpoint omitting the
    /// terminal event; the second is an empty stream).
    reasoning_deltas: usize,
    terminal: Option<Response>,
    /// Completed output items keyed by `output_index`, in output order. Used
    /// to reconstruct the terminal output when compatible endpoints omit the
    /// `output` array from the terminal event.
    done_items: BTreeMap<u32, OutputItem>,
    /// Raw JSON for completed items, retained so a terminal-less recovery
    /// replays the provider's field shape rather than the typed SDK's shape.
    raw_done_items: BTreeMap<u32, serde_json::Value>,
    /// Output indices announced via `output_item.added` that have not yet been
    /// completed. A non-empty set means the output sequence is incomplete and
    /// must not be reconstructed from `done_items`.
    pending_added: BTreeSet<u32>,
    /// Output indices announced via `output_item.added` as compaction items
    /// that have not yet been completed. A missing compaction boundary must
    /// hard-fail in `finish()` rather than fall back to visible-text recovery,
    /// which would silently drop the compacted baseline.
    pending_compactions: BTreeSet<u32>,
    raw_terminal_output: Option<Vec<serde_json::Value>>,
    /// Maps Responses output indices to the zero-based ordinal of hosted tool
    /// calls in this response. `output_index` also counts messages/reasoning,
    /// so it must not be rendered as the tool step number.
    web_search_step_indices: BTreeMap<u32, usize>,
    web_search_started_at: BTreeMap<u32, Instant>,
    web_search_announced: BTreeSet<u32>,
}

impl ResponsesStreamState {
    pub(crate) fn close_thinking(&mut self) -> Option<ProviderProgress> {
        if self.thinking_open {
            self.thinking_open = false;
            Some(ProviderProgress::Thinking(ThinkingChunk::Finished))
        } else {
            None
        }
    }

    fn thinking_delta(&mut self, delta: String) -> Vec<ProviderProgress> {
        if delta.is_empty() {
            return Vec::new();
        }
        // Counted here (not in `visible_delta`): reasoning is the one content
        // class that reaches the user and leaves no trace in the state.
        self.reasoning_deltas += 1;
        let mut updates = Vec::with_capacity(2);
        if !self.thinking_open {
            self.thinking_open = true;
            updates.push(ProviderProgress::Thinking(ThinkingChunk::Started));
        }
        updates.push(ProviderProgress::Thinking(ThinkingChunk::Delta(delta)));
        updates
    }

    fn visible_delta(&mut self, delta: String) -> Vec<ProviderProgress> {
        if delta.is_empty() {
            return Vec::new();
        }
        self.output_text.push_str(&delta);
        let mut updates = Vec::with_capacity(2);
        if self.thinking_open {
            self.thinking_open = false;
            updates.push(ProviderProgress::Thinking(ThinkingChunk::Finished));
        }
        updates.push(ProviderProgress::TextDelta(delta));
        updates
    }

    fn set_terminal(&mut self, response: Response) -> Result<Vec<ProviderProgress>, LlmError> {
        if self.terminal.is_some() {
            return Err(LlmError::Unsupported(
                "multiple terminal events".to_string(),
            ));
        }
        self.terminal = Some(response);
        Ok(self.close_thinking().into_iter().collect())
    }

    #[cfg(test)]
    pub(crate) fn apply(
        &mut self,
        event: ResponseStreamEvent,
    ) -> Result<Vec<ProviderProgress>, LlmError> {
        self.apply_with_raw(event, None)
    }

    pub(crate) fn apply_with_raw(
        &mut self,
        event: ResponseStreamEvent,
        raw_output_items: Option<Vec<serde_json::Value>>,
    ) -> Result<Vec<ProviderProgress>, LlmError> {
        let done_index = match &event {
            ResponseStreamEvent::ResponseOutputItemDone(item) => Some(item.output_index),
            _ => None,
        };
        if let Some(index) = done_index {
            if let Some(item) = raw_output_items.and_then(|items| items.into_iter().next()) {
                self.raw_done_items.insert(index, item);
            }
        } else if raw_output_items.is_some() {
            self.raw_terminal_output = raw_output_items;
        }
        if let ResponseStreamEvent::ResponseError(_) = event {
            return Err(LlmError::StreamParse(
                "OpenAI Responses stream error".to_string(),
            ));
        }
        Ok(match event {
            ResponseStreamEvent::ResponseReasoningSummaryTextDelta(event) => {
                self.thinking_delta(event.delta)
            }
            ResponseStreamEvent::ResponseReasoningTextDelta(event) => {
                self.thinking_delta(event.delta)
            }
            ResponseStreamEvent::ResponseOutputTextDelta(event) => self.visible_delta(event.delta),
            ResponseStreamEvent::ResponseRefusalDelta(event) => self.visible_delta(event.delta),
            ResponseStreamEvent::ResponseOutputItemAdded(event) => {
                // A completed item is authoritative: ignore a duplicate
                // `added` event once the corresponding `done` item exists.
                // Otherwise remember the announced index so an item that is
                // added but never completed marks the sequence incomplete.
                // Compaction indices are tracked separately because a missing
                // compaction boundary must hard-fail rather than fall back to
                // visible-text recovery.
                if !self.done_items.contains_key(&event.output_index) {
                    self.pending_added.insert(event.output_index);
                    if matches!(&event.item, OutputItem::Compaction(_)) {
                        self.pending_compactions.insert(event.output_index);
                    }
                    // Surface in-progress web search as a running tool card
                    // (the provider may populate `action` only later, at
                    // `output_item.done`).
                    if let OutputItem::WebSearchCall(call) = &event.item {
                        self.web_search_started_at
                            .entry(event.output_index)
                            .or_insert_with(Instant::now);
                        let next_idx = self.web_search_step_indices.len();
                        let step_idx = *self
                            .web_search_step_indices
                            .entry(event.output_index)
                            .or_insert(next_idx);
                        if self.web_search_announced.insert(event.output_index) {
                            return Ok(vec![web_search_started(
                                event.output_index,
                                step_idx as u32,
                                call,
                            )]);
                        }
                    }
                }
                Vec::new()
            }
            ResponseStreamEvent::ResponseOutputItemDone(event) => {
                // Idempotent by `output_index`: a repeated `done` event
                // overwrites the same slot (last one wins) without
                // re-emitting — `insert` returns `None` only on first insert.
                self.pending_added.remove(&event.output_index);
                self.pending_compactions.remove(&event.output_index);
                if self
                    .done_items
                    .insert(event.output_index, event.item)
                    .is_none()
                    && let Some(OutputItem::WebSearchCall(call)) =
                        self.done_items.get(&event.output_index)
                {
                    let next_idx = self.web_search_step_indices.len();
                    let step_idx = *self
                        .web_search_step_indices
                        .entry(event.output_index)
                        .or_insert(next_idx);
                    let duration_us = self
                        .web_search_started_at
                        .remove(&event.output_index)
                        .map(|started| started.elapsed().as_micros() as u64);
                    let mut progress = Vec::with_capacity(2);
                    if self.web_search_announced.insert(event.output_index) {
                        progress.push(web_search_started(
                            event.output_index,
                            step_idx as u32,
                            call,
                        ));
                    }
                    progress.push(web_search_finished(
                        event.output_index,
                        step_idx as u32,
                        call,
                        duration_us,
                    ));
                    return Ok(progress);
                }
                Vec::new()
            }
            ResponseStreamEvent::ResponseCompleted(event) => {
                return self.set_terminal(event.response);
            }
            ResponseStreamEvent::ResponseIncomplete(event) => {
                return self.set_terminal(event.response);
            }
            ResponseStreamEvent::ResponseFailed(event) => {
                return self.set_terminal(event.response);
            }
            ResponseStreamEvent::ResponseCreated(_)
            | ResponseStreamEvent::ResponseQueued(_)
            | ResponseStreamEvent::ResponseInProgress(_)
            | ResponseStreamEvent::ResponseContentPartAdded(_)
            | ResponseStreamEvent::ResponseContentPartDone(_)
            | ResponseStreamEvent::ResponseOutputTextDone(_)
            | ResponseStreamEvent::ResponseRefusalDone(_)
            | ResponseStreamEvent::ResponseReasoningSummaryPartAdded(_)
            | ResponseStreamEvent::ResponseReasoningSummaryPartDone(_)
            | ResponseStreamEvent::ResponseReasoningSummaryTextDone(_)
            | ResponseStreamEvent::ResponseReasoningTextDone(_)
            | ResponseStreamEvent::ResponseFunctionCallArgumentsDelta(_)
            | ResponseStreamEvent::ResponseFunctionCallArgumentsDone(_) => Vec::new(),
            _ => Vec::new(),
        })
    }

    pub(crate) fn finish(self) -> Result<NormalizedResponse, LlmError> {
        let ResponsesStreamState {
            output_text,
            reasoning_deltas,
            terminal,
            done_items,
            raw_done_items,
            pending_added,
            pending_compactions,
            raw_terminal_output,
            ..
        } = self;
        // Exactly one output sequence is normalized: the terminal `output`
        // array when present, otherwise the complete `output_item.done`
        // sequence, otherwise the visible-text recovery below. The done
        // sequence is complete only when no announced item is still pending
        // and the indices form a contiguous 0-based range.
        let done_sequence = if done_items.is_empty() || !pending_added.is_empty() {
            None
        } else {
            let max_index = *done_items.keys().next_back().expect("non-empty map");
            if done_items.len() == max_index as usize + 1 {
                Some(done_items.values().cloned().collect::<Vec<_>>())
            } else {
                None
            }
        };
        let response = match terminal {
            Some(response) => response,
            None => {
                // Compatible endpoints may close the SSE stream without a
                // terminal event (no `response.completed` / `response.incomplete`
                // / `response.failed`). When the stream itself is complete — a
                // full `output_item.done` sequence or streamed visible text —
                // synthesize a minimal completed response so the recovery
                // branches below rebuild the output. A missing compaction
                // boundary and an empty stream remain hard protocol errors.
                if !pending_compactions.is_empty() {
                    return Err(LlmError::Unsupported(
                        "OpenAI Responses stream ended with an incomplete compaction item sequence"
                            .to_string(),
                    ));
                }
                if done_sequence.is_none() && output_text.is_empty() {
                    // The two causes need different actions, so name the one
                    // that actually happened instead of one wording for both.
                    let cause = if reasoning_deltas > 0 {
                        "the stream carried only reasoning, and a turn with no visible \
                         text and no completed output item is not a complete turn — a \
                         compatible endpoint is closing the stream without its terminal \
                         event"
                    } else {
                        "the stream carried no output at all, and the endpoint never sent \
                         response.completed / response.incomplete / response.failed"
                    };
                    return Err(LlmError::Unsupported(format!(
                        "OpenAI Responses stream ended without a terminal event \
                         (reasoning deltas: {reasoning_deltas}, completed output items: {}, \
                         announced but never completed: {}): {cause}",
                        done_items.len(),
                        pending_added.len(),
                    )));
                }
                serde_json::from_value(serde_json::json!({
                    "id": "compat-response",
                    "object": "response",
                    "created_at": 0,
                    "model": "compat-model",
                    "status": "completed",
                    "output": [],
                }))
                .map_err(|error| {
                    LlmError::StreamParse(format!(
                        "synthesize fallback Responses response: {error}"
                    ))
                })?
            }
        };
        let mut normalized = if !response.output.is_empty() {
            normalize_response(response)?
        } else if let Some(output_items) = done_sequence {
            let mut reconstructed = response;
            reconstructed.output = output_items;
            let mut normalized = normalize_response(reconstructed)?;
            if !raw_done_items.is_empty() {
                normalized.output_items = raw_done_items.into_values().collect();
            }
            normalized
        } else if !pending_compactions.is_empty() {
            // Hard protocol error: a compaction item was announced but never
            // completed. Neither the done-sequence reconstruction nor
            // visible-text recovery may run here, because both would silently
            // drop the compaction boundary and lose the compacted baseline
            // for the next turn.
            return Err(LlmError::Unsupported(
                "OpenAI Responses stream ended with an incomplete compaction item sequence"
                    .to_string(),
            ));
        } else if !output_text.is_empty() {
            // Compatible-endpoint visible-text recovery: no terminal output
            // and no complete done sequence, so the streamed delta is the
            // only available text source. This is text recovery, not a
            // compaction fallback: a missing compaction baseline remains a
            // hard protocol error in `normalize_response`.
            normalize_response(response)?
        } else {
            return Err(LlmError::Unsupported(format!(
                "OpenAI Responses terminal event carried no output and the \
                 output_item.done sequence is incomplete (reasoning deltas: {reasoning_deltas})"
            )));
        };
        // Never combine streamed deltas with authoritative message text: the
        // deltas are appended only when the normalized output has no text at
        // all, so text is never duplicated.
        if !output_text.is_empty()
            && !normalized
                .blocks
                .iter()
                .any(|block| matches!(block, crate::ContentBlock::Text { .. }))
        {
            normalized
                .blocks
                .push(crate::ContentBlock::Text { text: output_text });
        }
        if let Some(raw_output_items) = raw_terminal_output {
            normalized.output_items = raw_output_items;
        }
        Ok(normalized)
    }
}

#[cfg(test)]
#[cfg(test)]
mod tests;
