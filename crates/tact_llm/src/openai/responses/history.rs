use std::collections::BTreeMap;

use async_openai_responses::types::responses::ReasoningItem;
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
    })
    .map_err(|error| LlmError::Other(format!("serialize Responses history state: {error}")))?;
    Ok(format!("{PREFIX}{json}"))
}

pub(crate) fn decode(signature: &str) -> Result<Option<ResponsesHistoryState>, LlmError> {
    let Some(json) = signature.strip_prefix(PREFIX) else {
        return Ok(None);
    };
    serde_json::from_str(json).map(Some).map_err(|error| {
        LlmError::Other(format!("parse persisted Responses history state: {error}"))
    })
}
