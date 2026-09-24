use std::collections::BTreeMap;

use async_openai_responses::types::responses::{ReasoningItem, ReasoningItemContent, SummaryPart};
use serde::{Deserialize, Serialize};

use crate::LlmError;

const PREFIX: &str = "openai-responses-v1:";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ResponsesHistoryState {
    pub reasoning: ReasoningItem,
    pub function_call_item_ids: BTreeMap<String, String>,
}

pub(crate) fn encode(
    reasoning: ReasoningItem,
    function_call_item_ids: BTreeMap<String, String>,
) -> Result<String, LlmError> {
    let json = serde_json::to_string(&ResponsesHistoryState {
        reasoning,
        function_call_item_ids,
    })?;
    Ok(format!("{PREFIX}{json}"))
}

pub(crate) fn decode(signature: &str) -> Result<Option<ResponsesHistoryState>, LlmError> {
    let Some(json) = signature.strip_prefix(PREFIX) else {
        return Ok(None);
    };
    serde_json::from_str(json).map(Some).map_err(|error| {
        LlmError::Unsupported(format!("parse persisted Responses history state: {error}"))
    })
}

/// The reasoning text a `ReasoningItem` carries, for display.
///
/// Both shapes hold text. OpenAI's models fill `summary` and leave `content`
/// empty; a compatible endpoint (DeepSeek's responses mode, among others) does
/// the reverse. The live path folds either delta into one trace, so anything
/// that reads the item — persisting it, or redrawing it — has to look at both.
pub(crate) fn visible_text(reasoning: &ReasoningItem) -> String {
    reasoning
        .summary
        .iter()
        .map(|part| match part {
            SummaryPart::SummaryText(summary) => summary.text.as_str(),
        })
        .chain(reasoning.content.iter().flatten().map(|part| match part {
            ReasoningItemContent::ReasoningText(text) => text.text.as_str(),
        }))
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// The reasoning text held only inside a persisted signature.
///
/// A session stored before [`visible_text`] reached the visible block kept an
/// empty `thinking` and the text here instead, so a redraw has to ask for it —
/// otherwise the card comes back empty and the transcript drops it. `None` when
/// the signature is not a Responses one, cannot be parsed, or carries no text.
pub fn reasoning_text(signature: &str) -> Option<String> {
    let state = decode(signature).ok().flatten()?;
    let text = visible_text(&state.reasoning);
    (!text.is_empty()).then_some(text)
}
