//! Versioned provider-specific conversation state.
//!
//! Tact keeps its provider-independent `Vec<Message>` logical history separate
//! from the wire-level protocol state required to continue an OpenAI Responses
//! conversation. [`ProviderConversationState`] is that opaque, versioned
//! boundary: it stores the exact Responses input-item baseline as JSON so
//! unknown fields and future item types survive SDK upgrades **once they are
//! part of the input baseline**. Terminal *output* parsing is typed
//! (async-openai `OutputItem` has no `Unknown` variant), so a truly unknown
//! output item type is rejected as a hard protocol error at the adapter
//! boundary — never silently dropped or replaced by a fallback.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::content::Message;
use crate::error::LlmError;

/// Provider-specific conversation state carried alongside Tact's logical
/// `Vec<Message>` history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProviderConversationState {
    /// OpenAI Responses protocol state.
    OpenAiResponses(ResponsesConversationState),
}

/// Versioned OpenAI Responses conversation state.
///
/// `input_items` is the exact wire-level protocol baseline for the next
/// request. It may contain compaction, reasoning, function-call,
/// function-call-output, message, and other SDK-known item types. The state is
/// stored as JSON rather than an SDK-specific binary format so unknown fields
/// and future input item types survive SDK upgrades; terminal *output* item
/// types unknown to the typed SDK are rejected earlier as a hard protocol
/// error (see the adapter boundary), never silently dropped.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResponsesConversationState {
    pub version: u32,
    pub provider: String,
    pub base_url: String,
    pub model: String,
    pub input_items: Vec<serde_json::Value>,
    pub compaction_id: Option<String>,
    pub is_compacted: bool,
    pub logical_message_count: usize,
    pub logical_context_hash: String,
}

/// State update produced by an LLM call alongside its content blocks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderStateUpdate {
    /// The call did not change the provider conversation state.
    Unchanged,
    /// The call produced a replacement provider conversation state (for
    /// example, after a native compaction boundary).
    Replace(ProviderConversationState),
}

/// Computes the stable SHA-256 context hash for a prefix of Tact's logical
/// `Message` history.
///
/// The hash covers exactly the serialized `Message` slice supplied by the
/// caller: the bytes are `serde_json::to_vec(messages)`, so the caller
/// controls the prefix boundary. Returns a typed serialization error instead
/// of a sentinel value when the slice cannot be serialized.
pub fn context_hash(messages: &[Message]) -> Result<String, LlmError> {
    let serialized = serde_json::to_vec(messages)?;
    let mut hasher = Sha256::new();
    hasher.update(serialized);
    Ok(hex_encode(&hasher.finalize()))
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::{ProviderConversationState, ProviderStateUpdate, ResponsesConversationState};
    use crate::content::{Message, Role};

    #[test]
    fn explicit_compact_fixture_contains_one_non_empty_compaction_item() {
        let value: Value = serde_json::from_str(include_str!(
            "openai/responses/fixtures/explicit_compact.json"
        ))
        .unwrap();
        let output = value["output"].as_array().unwrap();
        let compactions = output
            .iter()
            .filter(|item| item["type"] == "compaction")
            .collect::<Vec<_>>();
        assert_eq!(compactions.len(), 1);
        assert!(
            !compactions[0]["encrypted_content"]
                .as_str()
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn automatic_compact_fixture_is_terminal_with_one_non_empty_compaction_item() {
        let value: Value = serde_json::from_str(include_str!(
            "openai/responses/fixtures/automatic_compact.json"
        ))
        .unwrap();
        // A terminal `/responses` response; the compaction item arrives only
        // when the automatic `context_management` compaction fired.
        assert_eq!(value["status"], "completed");
        let output = value["output"].as_array().unwrap();
        let compactions = output
            .iter()
            .filter(|item| item["type"] == "compaction")
            .collect::<Vec<_>>();
        assert_eq!(compactions.len(), 1);
        assert!(
            !compactions[0]["encrypted_content"]
                .as_str()
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn stream_compact_fixture_has_terminal_event_and_complete_done_sequence() {
        let events = include_str!("openai/responses/fixtures/stream_compact_events.jsonl")
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>();

        let terminal = events
            .iter()
            .find(|event| event["type"] == "response.completed")
            .expect("stream fixture must contain a terminal response.completed event");
        let terminal_output = terminal["response"]["output"]
            .as_array()
            .expect("terminal response must carry complete output");

        let compactions = terminal_output
            .iter()
            .filter(|item| item["type"] == "compaction")
            .collect::<Vec<_>>();
        assert_eq!(compactions.len(), 1);
        assert!(
            !compactions[0]["encrypted_content"]
                .as_str()
                .unwrap()
                .is_empty()
        );

        // Either the terminal output is complete or a complete
        // `output_item.done` sequence is present; the fixture provides both
        // and they must agree item-by-item.
        let done_events = events
            .iter()
            .filter(|event| event["type"] == "response.output_item.done")
            .collect::<Vec<_>>();
        assert_eq!(
            done_events.len(),
            terminal_output.len(),
            "done sequence must cover every terminal output item"
        );
        for (done, item) in done_events.iter().zip(terminal_output.iter()) {
            assert_eq!(done["item"]["type"], item["type"]);
            assert_eq!(done["item"]["id"], item["id"]);
        }
    }

    fn state_with_compaction_and_unknown_items() -> ResponsesConversationState {
        ResponsesConversationState {
            version: 1,
            provider: "openai_responses".to_string(),
            base_url: "https://api.openai.com/v1".to_string(),
            model: "gpt-5.4-mini".to_string(),
            input_items: vec![
                serde_json::json!({
                    "type": "compaction",
                    "id": "cmp_test_1",
                    "encrypted_content": "sanitized-encrypted-content"
                }),
                serde_json::json!({
                    "type": "future_unknown_item",
                    "opaque": { "any": ["shape"] }
                }),
            ],
            compaction_id: Some("cmp_test_1".to_string()),
            is_compacted: true,
            logical_message_count: 2,
            logical_context_hash: "abc123".to_string(),
        }
    }

    #[test]
    fn state_with_compaction_and_unknown_items_round_trips_byte_equivalently() {
        let state = state_with_compaction_and_unknown_items();

        let encoded = serde_json::to_vec(&state).unwrap();
        let decoded: ResponsesConversationState = serde_json::from_slice(&encoded).unwrap();
        let re_encoded = serde_json::to_vec(&decoded).unwrap();
        assert_eq!(encoded, re_encoded);

        // The unknown item survives verbatim as a JSON value.
        assert_eq!(
            decoded.input_items[1],
            serde_json::json!({
                "type": "future_unknown_item",
                "opaque": { "any": ["shape"] }
            })
        );
        assert_eq!(decoded.compaction_id.as_deref(), Some("cmp_test_1"));
        assert!(decoded.is_compacted);
    }

    #[test]
    fn provider_conversation_state_enum_round_trips() {
        let state =
            ProviderConversationState::OpenAiResponses(state_with_compaction_and_unknown_items());

        let encoded = serde_json::to_vec(&state).unwrap();
        let decoded: ProviderConversationState = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(decoded, state);
        assert!(matches!(
            decoded,
            ProviderConversationState::OpenAiResponses(ref inner)
                if inner.compaction_id.as_deref() == Some("cmp_test_1")
        ));
    }

    #[test]
    fn provider_state_update_variants_are_distinct() {
        let unchanged = ProviderStateUpdate::Unchanged;
        let replaced = ProviderStateUpdate::Replace(ProviderConversationState::OpenAiResponses(
            state_with_compaction_and_unknown_items(),
        ));
        assert_ne!(unchanged, replaced);
        assert!(matches!(unchanged, ProviderStateUpdate::Unchanged));
    }

    #[test]
    fn context_hash_is_stable_sha256_hex() {
        let messages = [
            Message::new_text(Role::User, "hello"),
            Message::new_text(Role::Assistant, "world"),
        ];

        let first = super::context_hash(&messages).unwrap();
        let second = super::context_hash(&messages).unwrap();
        assert_eq!(first, second);
        assert_eq!(first.len(), 64);
        assert!(first.bytes().all(|byte| byte.is_ascii_hexdigit()));
    }

    #[test]
    fn context_hash_changes_when_one_logical_message_changes() {
        let first = Message::new_text(Role::User, "hello");
        let base = [first.clone(), Message::new_text(Role::Assistant, "world")];
        let changed = [first, Message::new_text(Role::Assistant, "world!")];

        let base_hash = super::context_hash(&base).unwrap();
        let changed_hash = super::context_hash(&changed).unwrap();
        assert_ne!(base_hash, changed_hash);
    }

    #[test]
    fn context_hash_changes_with_prefix_length() {
        let first = Message::new_text(Role::User, "hello");
        let second = Message::new_text(Role::Assistant, "world");

        let prefix_hash = super::context_hash(std::slice::from_ref(&first)).unwrap();
        let full_hash = super::context_hash(&[first, second]).unwrap();
        assert_ne!(prefix_hash, full_hash);
    }
}
