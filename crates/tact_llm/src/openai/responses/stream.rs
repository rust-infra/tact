use std::collections::{BTreeMap, BTreeSet};

use async_openai_responses::types::responses::{OutputItem, Response, ResponseStreamEvent};
use tact_protocol::{AgentUpdate, ThinkingChunk};

use super::normalize::{NormalizedResponse, normalize_response};
use crate::LlmError;

#[derive(Default)]
pub(crate) struct ResponsesStreamState {
    thinking_open: bool,
    output_text: String,
    terminal: Option<Response>,
    /// Completed output items keyed by `output_index`, in output order. Used
    /// to reconstruct the terminal output when compatible endpoints omit the
    /// `output` array from the terminal event.
    done_items: BTreeMap<u32, OutputItem>,
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
}

impl ResponsesStreamState {
    pub(crate) fn close_thinking(&mut self) -> Option<AgentUpdate> {
        if self.thinking_open {
            self.thinking_open = false;
            Some(AgentUpdate::ThinkingChunk(ThinkingChunk::Finished))
        } else {
            None
        }
    }

    fn thinking_delta(&mut self, delta: String) -> Vec<AgentUpdate> {
        if delta.is_empty() {
            return Vec::new();
        }
        let mut updates = Vec::with_capacity(2);
        if !self.thinking_open {
            self.thinking_open = true;
            updates.push(AgentUpdate::ThinkingChunk(ThinkingChunk::Started));
        }
        updates.push(AgentUpdate::ThinkingChunk(ThinkingChunk::Delta(delta)));
        updates
    }

    fn visible_delta(&mut self, delta: String) -> Vec<AgentUpdate> {
        if delta.is_empty() {
            return Vec::new();
        }
        self.output_text.push_str(&delta);
        let mut updates = Vec::with_capacity(2);
        if self.thinking_open {
            self.thinking_open = false;
            updates.push(AgentUpdate::ThinkingChunk(ThinkingChunk::Finished));
        }
        updates.push(AgentUpdate::StreamChunk(delta));
        updates
    }

    fn set_terminal(&mut self, response: Response) -> Result<Vec<AgentUpdate>, LlmError> {
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
    ) -> Result<Vec<AgentUpdate>, LlmError> {
        self.apply_with_raw(event, None)
    }

    pub(crate) fn apply_with_raw(
        &mut self,
        event: ResponseStreamEvent,
        raw_output_items: Option<Vec<serde_json::Value>>,
    ) -> Result<Vec<AgentUpdate>, LlmError> {
        if raw_output_items.is_some() {
            self.raw_terminal_output = raw_output_items;
        }
        if let ResponseStreamEvent::ResponseError(event) = event {
            let code = event.code.as_deref().unwrap_or("unknown_error");
            let param = event
                .param
                .as_deref()
                .map(|param| format!(" (param: {param})"))
                .unwrap_or_default();
            return Err(LlmError::StreamParse(format!(
                "OpenAI Responses stream error {code}: {}{param}",
                event.message
            )));
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
                }
                Vec::new()
            }
            ResponseStreamEvent::ResponseOutputItemDone(event) => {
                // Idempotent by `output_index`: a repeated `done` event
                // overwrites the same slot and never duplicates output.
                self.pending_added.remove(&event.output_index);
                self.pending_compactions.remove(&event.output_index);
                self.done_items.insert(event.output_index, event.item);
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
            _ => Vec::new(),
        })
    }

    pub(crate) fn finish(self) -> Result<NormalizedResponse, LlmError> {
        let ResponsesStreamState {
            output_text,
            terminal,
            done_items,
            pending_added,
            pending_compactions,
            raw_terminal_output,
            ..
        } = self;
        let response = terminal.ok_or_else(|| {
            LlmError::Unsupported("OpenAI Responses stream ended without a terminal event".into())
        })?;
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
        let mut normalized = if !response.output.is_empty() {
            normalize_response(response)?
        } else if let Some(output_items) = done_sequence {
            let mut reconstructed = response;
            reconstructed.output = output_items;
            normalize_response(reconstructed)?
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
            return Err(LlmError::Unsupported(
                "OpenAI Responses terminal event carried no output and the \
                 output_item.done sequence is incomplete"
                    .to_string(),
            ));
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
mod tests {
    use async_openai_responses::types::responses::ResponseStreamEvent;
    use tact_protocol::{AgentUpdate, ThinkingChunk};

    use super::ResponsesStreamState;
    use crate::ContentBlock;

    fn event(value: serde_json::Value) -> ResponseStreamEvent {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn streams_thinking_before_text_and_uses_terminal_response_for_final_blocks() {
        let mut state = ResponsesStreamState::default();
        let thinking = state
            .apply(event(serde_json::json!({
                "type": "response.reasoning_summary_text.delta",
                "sequence_number": 1,
                "item_id": "rs_1",
                "output_index": 0,
                "summary_index": 0,
                "delta": "plan"
            })))
            .unwrap();
        assert!(matches!(
            thinking.as_slice(),
            [
                AgentUpdate::ThinkingChunk(ThinkingChunk::Started),
                AgentUpdate::ThinkingChunk(ThinkingChunk::Delta(delta))
            ] if delta == "plan"
        ));

        let text = state
            .apply(event(serde_json::json!({
                "type": "response.output_text.delta",
                "sequence_number": 2,
                "item_id": "msg_1",
                "output_index": 1,
                "content_index": 0,
                "delta": "answer",
                "logprobs": []
            })))
            .unwrap();
        assert!(matches!(
            text.as_slice(),
            [
                AgentUpdate::ThinkingChunk(ThinkingChunk::Finished),
                AgentUpdate::StreamChunk(delta)
            ] if delta == "answer"
        ));

        let ignored_arguments = state
            .apply(event(serde_json::json!({
                "type": "response.function_call_arguments.delta",
                "sequence_number": 3,
                "item_id": "fc_1",
                "output_index": 2,
                "delta": "{\"cmd\":"
            })))
            .unwrap();
        assert!(ignored_arguments.is_empty());

        let terminal = state
            .apply(event(serde_json::json!({
                "type": "response.completed",
                "sequence_number": 4,
                "response": super::super::normalize::tests::completed_response_json()
            })))
            .unwrap();
        assert!(terminal.is_empty());

        let normalized = state.finish().unwrap();
        assert!(matches!(
            normalized.blocks.last(),
            Some(ContentBlock::ToolUse { id, .. }) if id == "call_1"
        ));
    }

    #[test]
    fn terminal_event_finishes_open_thinking_once() {
        let mut state = ResponsesStreamState::default();
        state
            .apply(event(serde_json::json!({
                "type": "response.reasoning_text.delta",
                "sequence_number": 1,
                "item_id": "rs_1",
                "output_index": 0,
                "content_index": 0,
                "delta": "plan"
            })))
            .unwrap();
        let updates = state
            .apply(event(serde_json::json!({
                "type": "response.completed",
                "sequence_number": 2,
                "response": super::super::normalize::tests::completed_response_json()
            })))
            .unwrap();
        assert!(matches!(
            updates.as_slice(),
            [AgentUpdate::ThinkingChunk(ThinkingChunk::Finished)]
        ));
        assert!(state.finish().is_ok());
    }

    #[test]
    fn preserves_streamed_text_when_terminal_response_has_no_message_output() {
        let mut state = ResponsesStreamState::default();
        state
            .apply(event(serde_json::json!({
                "type": "response.output_text.delta",
                "sequence_number": 1,
                "item_id": "msg_1",
                "output_index": 0,
                "content_index": 0,
                "delta": "100 - 200 = -100",
                "logprobs": []
            })))
            .unwrap();

        let mut terminal = super::super::normalize::tests::completed_response_json();
        terminal["output"] = serde_json::json!([]);
        state
            .apply(event(serde_json::json!({
                "type": "response.completed",
                "sequence_number": 2,
                "response": terminal
            })))
            .unwrap();

        let normalized = state.finish().unwrap();
        assert!(normalized.blocks.iter().any(|block| {
            matches!(block, ContentBlock::Text { text } if text == "100 - 200 = -100")
        }));
    }

    #[test]
    fn response_error_event_preserves_api_details() {
        let mut state = ResponsesStreamState::default();
        let error = state
            .apply(event(serde_json::json!({
                "type": "error",
                "sequence_number": 1,
                "code": "rate_limit_exceeded",
                "message": "slow down",
                "param": "input"
            })))
            .unwrap_err()
            .to_string();
        assert!(error.contains("rate_limit_exceeded"));
        assert!(error.contains("slow down"));
        assert!(error.contains("input"));
    }

    #[test]
    fn duplicate_terminal_event_is_an_error() {
        let mut state = ResponsesStreamState::default();
        state
            .apply(event(serde_json::json!({
                "type": "response.completed",
                "sequence_number": 1,
                "response": super::super::normalize::tests::completed_response_json()
            })))
            .unwrap();
        let error = state
            .apply(event(serde_json::json!({
                "type": "response.completed",
                "sequence_number": 2,
                "response": super::super::normalize::tests::completed_response_json()
            })))
            .unwrap_err()
            .to_string();
        assert!(error.contains("multiple terminal events"));
    }

    fn fixture_stream_events() -> Vec<serde_json::Value> {
        include_str!("fixtures/stream_compact_events.jsonl")
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    fn output_item_added(index: u32, item: serde_json::Value) -> ResponseStreamEvent {
        event(serde_json::json!({
            "type": "response.output_item.added",
            "sequence_number": index as u64 * 2 + 1,
            "output_index": index,
            "item": item
        }))
    }

    fn output_item_done(index: u32, item: serde_json::Value) -> ResponseStreamEvent {
        event(serde_json::json!({
            "type": "response.output_item.done",
            "sequence_number": index as u64 * 2 + 2,
            "output_index": index,
            "item": item
        }))
    }

    fn completed_with_output(output: serde_json::Value) -> ResponseStreamEvent {
        let mut response = super::super::normalize::tests::completed_response_json();
        response["output"] = output;
        event(serde_json::json!({
            "type": "response.completed",
            "sequence_number": 100,
            "response": response
        }))
    }

    fn message_item(text: &str) -> serde_json::Value {
        serde_json::json!({
            "type": "message",
            "id": "msg_1",
            "role": "assistant",
            "status": "completed",
            "content": [{
                "type": "output_text",
                "text": text,
                "annotations": []
            }]
        })
    }

    fn function_call_item() -> serde_json::Value {
        serde_json::json!({
            "type": "function_call",
            "arguments": "{\"cmd\":\"pwd\"}",
            "call_id": "call_1",
            "name": "bash",
            "id": "fc_1",
            "status": "completed"
        })
    }

    #[test]
    fn collects_done_output_items_from_fixture_without_exposing_encrypted_content() {
        let mut state = ResponsesStreamState::default();
        for value in fixture_stream_events() {
            state.apply(event(value)).unwrap();
        }

        let normalized = state.finish().unwrap();
        assert!(matches!(
            normalized.blocks.as_slice(),
            [
                ContentBlock::Thinking { thinking, .. },
                ContentBlock::Text { text }
            ] if thinking == "sanitized reasoning summary" && text == "sanitized assistant answer"
        ));
        assert!(!normalized.blocks.iter().any(|block| {
            matches!(block, ContentBlock::Text { text } if text.contains("encrypted"))
        }));

        let update = normalized
            .provider_state_update(
                vec![serde_json::json!({"type": "message", "role": "user", "content": "old"})],
                "openai_responses",
                "https://api.openai.com/v1",
                "gpt-5.4-mini",
                &[
                    crate::Message::new_text(crate::Role::User, "old turn"),
                    crate::Message::new_text(crate::Role::Assistant, "prior answer"),
                    crate::Message::new_text(crate::Role::User, "current turn"),
                ],
            )
            .unwrap();
        assert!(matches!(
            update,
            crate::ProviderStateUpdate::Replace(crate::ProviderConversationState::OpenAiResponses(
                _
            ))
        ));
        let crate::ProviderStateUpdate::Replace(crate::ProviderConversationState::OpenAiResponses(
            state,
        )) = update
        else {
            unreachable!("asserted above")
        };
        // The compaction item is retained as protocol state, never as content.
        assert_eq!(state.input_items.len(), 3);
        assert_eq!(state.input_items[0]["type"], "compaction");
        assert_eq!(state.input_items[0]["id"], "cmp_sanitized_01");
        assert_eq!(
            state.input_items[0]["encrypted_content"],
            "sanitized-encrypted-compaction-content-placeholder"
        );
        assert_eq!(state.compaction_id.as_deref(), Some("cmp_sanitized_01"));
        assert!(state.is_compacted);
    }

    #[test]
    fn terminal_output_takes_precedence_over_done_items() {
        let mut state = ResponsesStreamState::default();
        state
            .apply(output_item_added(0, message_item("streamed answer")))
            .unwrap();
        state
            .apply(output_item_done(0, message_item("streamed answer")))
            .unwrap();
        state
            .apply(completed_with_output(serde_json::json!([message_item(
                "terminal answer"
            )])))
            .unwrap();

        let normalized = state.finish().unwrap();
        assert!(matches!(
            normalized.blocks.as_slice(),
            [ContentBlock::Text { text }] if text == "terminal answer"
        ));
    }

    #[test]
    fn duplicate_added_and_done_events_do_not_duplicate_output_items() {
        let mut state = ResponsesStreamState::default();
        state
            .apply(output_item_added(0, message_item("single")))
            .unwrap();
        state
            .apply(output_item_done(0, message_item("single")))
            .unwrap();
        // A duplicate added event after the done event is ignored.
        state
            .apply(output_item_added(0, message_item("single")))
            .unwrap();
        // A duplicate done event is idempotent: it must not duplicate output.
        state
            .apply(output_item_done(0, message_item("single")))
            .unwrap();
        state
            .apply(completed_with_output(serde_json::json!([])))
            .unwrap();

        let normalized = state.finish().unwrap();
        assert!(matches!(
            normalized.blocks.as_slice(),
            [ContentBlock::Text { text }] if text == "single"
        ));
        assert_eq!(normalized.output_items.len(), 1);
    }

    #[test]
    fn incomplete_done_sequence_with_gap_is_an_error() {
        let mut state = ResponsesStreamState::default();
        state
            .apply(output_item_done(0, message_item("first")))
            .unwrap();
        state
            .apply(output_item_done(2, function_call_item()))
            .unwrap();
        state
            .apply(completed_with_output(serde_json::json!([])))
            .unwrap();

        let error = state.finish().unwrap_err().to_string();
        assert!(error.contains("incomplete"));
    }

    #[test]
    fn added_output_item_without_done_is_incomplete() {
        let mut state = ResponsesStreamState::default();
        state
            .apply(output_item_added(0, message_item("never done")))
            .unwrap();
        state
            .apply(completed_with_output(serde_json::json!([])))
            .unwrap();

        let error = state.finish().unwrap_err().to_string();
        assert!(error.contains("incomplete"));
    }

    #[test]
    fn added_item_never_done_marks_the_sequence_incomplete() {
        let mut state = ResponsesStreamState::default();
        state
            .apply(output_item_added(0, message_item("done")))
            .unwrap();
        state
            .apply(output_item_done(0, message_item("done")))
            .unwrap();
        // Announced but never completed: the done sequence alone would look
        // contiguous, but an item is missing.
        state
            .apply(output_item_added(1, message_item("never done")))
            .unwrap();
        state
            .apply(completed_with_output(serde_json::json!([])))
            .unwrap();

        let error = state.finish().unwrap_err().to_string();
        assert!(error.contains("incomplete"));
    }

    #[test]
    fn reconstructs_output_from_done_items_when_terminal_output_is_absent() {
        let events = fixture_stream_events();
        let mut state = ResponsesStreamState::default();
        // added(0), done(0), added(1), done(1), added(2), done(2) then a
        // terminal event whose output array is empty (compatible endpoints).
        for value in &events[..6] {
            state.apply(event(value.clone())).unwrap();
        }
        let mut terminal = super::super::normalize::tests::completed_response_json();
        terminal["output"] = serde_json::json!([]);
        state
            .apply(event(serde_json::json!({
                "type": "response.completed",
                "sequence_number": 7,
                "response": terminal
            })))
            .unwrap();

        let normalized = state.finish().unwrap();
        assert!(matches!(
            normalized.blocks.as_slice(),
            [
                ContentBlock::Thinking { thinking, .. },
                ContentBlock::Text { text }
            ] if thinking == "sanitized reasoning summary" && text == "sanitized assistant answer"
        ));
        assert!(!normalized.blocks.iter().any(|block| {
            matches!(block, ContentBlock::Text { text } if text.contains("encrypted"))
        }));
        // The compaction item is reconstructed from the done sequence.
        assert_eq!(normalized.output_items.len(), 3);
        assert_eq!(normalized.output_items[2]["type"], "compaction");
    }

    #[test]
    fn does_not_duplicate_streamed_text_when_done_items_carry_text() {
        let mut state = ResponsesStreamState::default();
        state
            .apply(event(serde_json::json!({
                "type": "response.output_text.delta",
                "sequence_number": 1,
                "item_id": "msg_1",
                "output_index": 0,
                "content_index": 0,
                "delta": "streamed",
                "logprobs": []
            })))
            .unwrap();
        state
            .apply(output_item_done(0, message_item("done text")))
            .unwrap();
        state
            .apply(completed_with_output(serde_json::json!([])))
            .unwrap();

        let normalized = state.finish().unwrap();
        assert!(matches!(
            normalized.blocks.as_slice(),
            [ContentBlock::Text { text }] if text == "done text"
        ));
    }

    #[test]
    fn incomplete_added_compaction_never_uses_visible_text_recovery() {
        let mut state = ResponsesStreamState::default();
        state
            .apply(output_item_added(
                0,
                serde_json::json!({
                    "type": "compaction",
                    "id": "cmp_1",
                    "encrypted_content": "opaque"
                }),
            ))
            .unwrap();
        state
            .apply(event(serde_json::json!({
                "type": "response.output_text.delta",
                "sequence_number": 2,
                "item_id": "msg_1",
                "output_index": 1,
                "content_index": 0,
                "delta": "visible fallback",
                "logprobs": []
            })))
            .unwrap();
        state
            .apply(completed_with_output(serde_json::json!([])))
            .unwrap();

        let error = state.finish().unwrap_err().to_string();
        assert!(error.contains("compaction") || error.contains("incomplete"));
        assert!(!error.contains("visible fallback"));
    }

    #[test]
    fn compaction_added_with_done_at_other_index_still_fails_on_empty_terminal() {
        let mut state = ResponsesStreamState::default();
        // Compaction announced at index 0 but only ever completed at index 1:
        // the compaction baseline is missing from the done sequence.
        state
            .apply(output_item_added(
                0,
                serde_json::json!({
                    "type": "compaction",
                    "id": "cmp_1",
                    "encrypted_content": "opaque"
                }),
            ))
            .unwrap();
        state
            .apply(output_item_done(1, message_item("answer")))
            .unwrap();
        // Visible deltas exist, so without the compaction guard finish()
        // would recover text and silently drop the missing compaction.
        state
            .apply(event(serde_json::json!({
                "type": "response.output_text.delta",
                "sequence_number": 2,
                "item_id": "msg_1",
                "output_index": 1,
                "content_index": 0,
                "delta": "visible fallback",
                "logprobs": []
            })))
            .unwrap();
        state
            .apply(completed_with_output(serde_json::json!([])))
            .unwrap();

        let error = state.finish().unwrap_err().to_string();
        assert!(error.contains("compaction") || error.contains("incomplete"));
        // Must not reconstruct a baseline that silently drops the compaction.
        assert!(!error.contains("visible fallback"));
    }

    #[test]
    fn stream_finish_is_equivalent_to_terminal_normalization_for_the_fixture() {
        let events = fixture_stream_events();
        let mut state = ResponsesStreamState::default();
        for value in &events {
            state.apply(event(value.clone())).unwrap();
        }
        let streamed = state.finish().unwrap();

        let terminal: async_openai_responses::types::responses::Response =
            serde_json::from_value(events.last().unwrap().get("response").unwrap().clone())
                .unwrap();
        let direct = super::super::normalize::normalize_response(terminal).unwrap();

        assert_eq!(streamed.blocks, direct.blocks);
        assert_eq!(streamed.stop_reason, direct.stop_reason);
        assert_eq!(streamed.output_items, direct.output_items);
        let streamed_usage = streamed.usage.as_ref().expect("fixture carries usage");
        let direct_usage = direct.usage.as_ref().expect("fixture carries usage");
        assert_eq!(streamed_usage.prompt, direct_usage.prompt);
        assert_eq!(streamed_usage.completion, direct_usage.completion);
        assert_eq!(streamed_usage.total, direct_usage.total);
        assert_eq!(
            streamed_usage.reasoning_tokens,
            direct_usage.reasoning_tokens
        );

        let input = vec![serde_json::json!({"type": "message", "role": "user", "content": "old"})];
        let messages = [
            crate::Message::new_text(crate::Role::User, "old turn"),
            crate::Message::new_text(crate::Role::Assistant, "prior answer"),
            crate::Message::new_text(crate::Role::User, "current turn"),
        ];
        let streamed_update = streamed
            .provider_state_update(
                input.clone(),
                "openai_responses",
                "https://api.openai.com/v1",
                "gpt-5.4-mini",
                &messages,
            )
            .unwrap();
        let direct_update = direct
            .provider_state_update(
                input,
                "openai_responses",
                "https://api.openai.com/v1",
                "gpt-5.4-mini",
                &messages,
            )
            .unwrap();
        assert_eq!(streamed_update, direct_update);
    }
}
