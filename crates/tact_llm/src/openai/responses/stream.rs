use async_openai_responses::types::responses::{Response, ResponseStreamEvent};
use tact_protocol::{AgentUpdate, ThinkingChunk};

use crate::LlmError;

use super::normalize::{NormalizedResponse, normalize_response};

#[derive(Default)]
pub(crate) struct ResponsesStreamState {
    thinking_open: bool,
    output_text: String,
    terminal: Option<Response>,
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
            return Err(LlmError::Other(
                "OpenAI Responses stream emitted multiple terminal events".to_string(),
            ));
        }
        self.terminal = Some(response);
        Ok(self.close_thinking().into_iter().collect())
    }

    pub(crate) fn apply(
        &mut self,
        event: ResponseStreamEvent,
    ) -> Result<Vec<AgentUpdate>, LlmError> {
        if let ResponseStreamEvent::ResponseError(event) = event {
            let code = event.code.as_deref().unwrap_or("unknown_error");
            let param = event
                .param
                .as_deref()
                .map(|param| format!(" (param: {param})"))
                .unwrap_or_default();
            return Err(LlmError::Other(format!(
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
        let response = self.terminal.ok_or_else(|| {
            LlmError::Other("OpenAI Responses stream ended without a terminal event".into())
        })?;
        let mut normalized = normalize_response(response)?;
        if !self.output_text.is_empty()
            && !normalized
                .blocks
                .iter()
                .any(|block| matches!(block, crate::ContentBlock::Text { .. }))
        {
            normalized.blocks.push(crate::ContentBlock::Text {
                text: self.output_text,
            });
        }
        Ok(normalized)
    }
}

#[cfg(test)]
mod tests {
    use super::ResponsesStreamState;
    use crate::ContentBlock;
    use async_openai_responses::types::responses::ResponseStreamEvent;
    use tact_protocol::{AgentUpdate, ThinkingChunk};

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
}
