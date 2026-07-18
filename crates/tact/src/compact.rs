//! Context compaction and transcript persistence.
//!
//! When the conversation grows too large this module provides:
//! - [`micro_compact`]: truncates old tool results while keeping the most
//!   recent `KEEP_RECENT_TOOL_RESULTS` entries intact.
//! - [`persist_large_output`]: writes oversized tool outputs to disk and
//!   returns a preview with a file path.
//! - [`write_transcript`]: serialises the entire conversation as JSONL.
//! - [`compacted_context`]: legacy single-summary replacement context.
//! - Codex-style rebuild: [`collect_user_messages`] + [`build_compacted_history`].

use anyhow::Context as _;
use std::{
    fs::{self, File},
    io::{BufWriter, Write},
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};
use tact_llm::{ContentBlock, Message, MessageContent, Role};

use crate::consts::TactPath;

/// How many recent tool results to preserve during micro-compaction;
/// all earlier tool results with > 120 chars are replaced with a stub.
const KEEP_RECENT_TOOL_RESULTS: usize = 12;
/// Character threshold above which a single tool output is persisted to
/// disk instead of being kept in the context.
const PERSIST_THRESHOLD: usize = 30_000;
/// Number of characters shown in the preview when persisting a large output.
const PREVIEW_CHARS: usize = 2_000;
const COMPACTED_TOOL_RESULT: &str = "[Earlier tool result compacted. If you need the full content to continue editing, re-read the relevant file.]";

/// Lead-in for compaction handoff messages. Used both as the summary body
/// prefix and to detect prior summaries so they are not stacked.
pub const SUMMARY_PREFIX: &str =
    "This conversation was compacted so the agent can continue working.";

/// How many characters of recent *real* user-message text to keep verbatim
/// when rebuilding compacted history (Codex-style).
pub const KEEP_USER_MESSAGE_CHARS: usize = 80_000;

/// Running compaction state for a session.
///
/// Tracks whether compaction has occurred, the last summary produced,
/// and which files have been touched recently.
#[derive(Debug, Default)]
pub struct CompactState {
    pub has_compacted: bool,
    pub last_summary: Option<String>,
    pub recent_files: Vec<String>,
}

/// Truncates old tool-result blocks in-place, preserving the most recent
/// [`KEEP_RECENT_TOOL_RESULTS`] entries.  Any older result longer than 120
/// characters is replaced with a stub message.
pub fn micro_compact(messages: &mut [Message], enabled: bool) {
    if !enabled {
        return;
    }
    let tool_result_positions = collect_tool_result_positions(messages);
    if tool_result_positions.len() <= KEEP_RECENT_TOOL_RESULTS {
        return;
    }

    let compact_until = tool_result_positions.len() - KEEP_RECENT_TOOL_RESULTS;
    for (message_idx, block_idx) in tool_result_positions.into_iter().take(compact_until) {
        let Some(message) = messages.get_mut(message_idx) else {
            continue;
        };
        let MessageContent::Blocks { content } = &mut message.content else {
            continue;
        };
        let Some(ContentBlock::ToolResult {
            content: tool_content,
            ..
        }) = content.get_mut(block_idx)
        else {
            continue;
        };

        if tool_content.chars().count() > 120 {
            *tool_content = COMPACTED_TOOL_RESULT.to_string();
        }
    }
}

/// Estimates the serialised JSON size of the message list in characters.
/// Used to decide when compaction is needed.
pub fn estimate_context_size(messages: &[Message]) -> usize {
    serde_json::to_string(messages)
        .map(|serialized| serialized.chars().count())
        .unwrap_or_default()
}

/// Serialized JSON character size of a single message.
pub fn estimate_message_size(message: &Message) -> usize {
    estimate_context_size(std::slice::from_ref(message))
}

/// Coarse chars→tokens conversion for reserving an incoming user turn against
/// a token-denominated `model_context_window`.
pub(crate) fn approx_chars_as_tokens(chars: usize) -> usize {
    chars.div_ceil(4)
}

/// Whether `text` is a prior compaction handoff (must not be re-kept as a
/// "real" user message).
pub fn is_summary_message(text: &str) -> bool {
    text.starts_with(SUMMARY_PREFIX)
}

fn user_text_content(message: &Message) -> Option<&str> {
    match &message.content {
        MessageContent::Text { content } => Some(content.as_str()),
        MessageContent::Blocks { .. } => None,
    }
}

/// Collect real user text turns, skipping tool-result users and prior summaries.
pub fn collect_user_messages(messages: &[Message]) -> Vec<Message> {
    messages
        .iter()
        .filter(|message| matches!(message.role, Role::User))
        .filter(|message| user_text_content(message).is_some_and(|text| !is_summary_message(text)))
        .cloned()
        .collect()
}

/// Rebuild compacted history: recent real user messages (char budget from the
/// tail) followed by a single summary user message.
pub fn build_compacted_history(
    user_messages: &[Message],
    summary_text: String,
    max_chars: usize,
) -> Vec<Message> {
    let mut selected: Vec<Message> = Vec::new();
    if max_chars > 0 {
        let mut remaining = max_chars;
        for message in user_messages.iter().rev() {
            if remaining == 0 {
                break;
            }
            let Some(text) = user_text_content(message) else {
                continue;
            };
            let chars = text.chars().count();
            if chars <= remaining {
                selected.push(message.clone());
                remaining = remaining.saturating_sub(chars);
            } else {
                let truncated: String = text.chars().take(remaining).collect();
                selected.push(Message::new_text(Role::User, truncated));
                break;
            }
        }
        selected.reverse();
    }

    let summary_body = if summary_text.is_empty() {
        "(no summary available)".to_string()
    } else {
        summary_text
    };
    selected.push(Message::new_text(
        Role::User,
        format!("{SUMMARY_PREFIX}\n\n{summary_body}"),
    ));
    selected
}

/// Writes the full conversation to a timestamped JSONL file under
/// `<workdir>/.claude/transcripts/`.  Returns the written file path.
pub fn write_transcript(tact_path: &TactPath, messages: &[Message]) -> anyhow::Result<PathBuf> {
    let transcript_dir = tact_path.transcript_dir();
    fs::create_dir_all(&transcript_dir)
        .with_context(|| format!("failed to create {}", transcript_dir.display()))?;

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before UNIX_EPOCH")?
        .as_secs();
    let transcript_path = transcript_dir.join(format!("transcript_{timestamp}.jsonl"));

    let file = File::create(&transcript_path)
        .with_context(|| format!("failed to create {}", transcript_path.display()))?;
    let mut writer = BufWriter::new(file);
    for message in messages {
        serde_json::to_writer(&mut writer, message).with_context(|| {
            format!(
                "failed to serialize message to {}",
                transcript_path.display()
            )
        })?;
        writer.write_all(b"\n")?;
    }
    writer.flush()?;
    Ok(transcript_path)
}

/// If `output` exceeds [`PERSIST_THRESHOLD`] characters, writes it to disk
/// under `<workdir>/.claude/tool-results/` and returns a preview with the file path.
/// Otherwise returns the output unchanged.
pub fn persist_large_output(
    tact_path: &TactPath,
    tool_use_id: &str,
    output: &str,
) -> anyhow::Result<String> {
    if output.chars().count() <= PERSIST_THRESHOLD {
        return Ok(output.to_string());
    }

    let output_dir = tact_path.tool_results_dir();
    fs::create_dir_all(&output_dir)
        .with_context(|| format!("failed to create {}", output_dir.display()))?;
    let output_path = output_dir.join(format!("{tool_use_id}.txt"));

    fs::write(&output_path, output)
        .with_context(|| format!("failed to write {}", output_path.display()))?;
    let preview = output.chars().take(PREVIEW_CHARS).collect::<String>();
    Ok(format!(
        "<persisted-output>\nFull output saved to: {}\nPreview:\n{}\n</persisted-output>",
        output_path.display(),
        preview
    ))
}

/// Produces a replacement context (single user message) containing a
/// summary of what was compacted. Used by [`crate::agent::Agent::compact_history_legacy`].
pub fn compacted_context(summary: String) -> Vec<Message> {
    vec![Message::new_text(
        Role::User,
        format!("{SUMMARY_PREFIX}\n\n{summary}"),
    )]
}

fn collect_tool_result_positions(messages: &[Message]) -> Vec<(usize, usize)> {
    let mut positions = Vec::new();
    for (message_idx, message) in messages.iter().enumerate() {
        if !matches!(message.role, Role::User) {
            continue;
        }
        let MessageContent::Blocks { content } = &message.content else {
            continue;
        };
        for (block_idx, block) in content.iter().enumerate() {
            if matches!(block, ContentBlock::ToolResult { .. }) {
                positions.push((message_idx, block_idx));
            }
        }
    }
    positions
}

/// Decide whether auto-compact should run before the next LLM call.
///
/// TODO: align closer to Codex (12K baseline / effective window %).
/// TODO(compact): trigger earlier (e.g. at 70-80% of the window) instead of
/// waiting until tokens >= window, so compaction happens before the provider
/// starts rejecting prompts.
///
/// Triggers when either:
/// - last token total (+ approx tokens for an *incoming* turn not yet in
///   context) is at/over the window, or
/// - estimated context chars (+ incoming turn chars) exceed the window.
pub(crate) fn should_auto_compact(
    last_token_total: u32,
    model_context_window: usize,
    estimated_chars: usize,
    incoming_turn_chars: usize,
) -> bool {
    if model_context_window == 0 {
        return false;
    }
    if last_token_total > 0 {
        let projected =
            (last_token_total as usize).saturating_add(approx_chars_as_tokens(incoming_turn_chars));
        if projected >= model_context_window {
            return true;
        }
    }
    // Char estimate vs token window is coarse, but catches post-tool growth
    // and (at entry) a large incoming user turn not yet pushed into context.
    // TODO: replace once we can estimate tokens pre-call.
    estimated_chars.saturating_add(incoming_turn_chars) > model_context_window
}

#[cfg(test)]
mod tests {
    use super::{
        KEEP_USER_MESSAGE_CHARS, SUMMARY_PREFIX, approx_chars_as_tokens, build_compacted_history,
        collect_user_messages, estimate_context_size, estimate_message_size, is_summary_message,
        should_auto_compact,
    };
    use tact_llm::{ContentBlock, Message, Role};

    #[test]
    fn should_auto_compact_uses_last_token_total_when_present() {
        assert!(!should_auto_compact(99, 100, 0, 0));
        assert!(should_auto_compact(100, 100, 0, 0));
        assert!(should_auto_compact(150, 100, 0, 0));
        assert!(!should_auto_compact(1, 0, 10_000, 0));
    }

    #[test]
    fn should_auto_compact_falls_back_to_chars_when_no_usage() {
        assert!(!should_auto_compact(0, 100, 100, 0));
        assert!(should_auto_compact(0, 100, 101, 0));
        assert!(!should_auto_compact(0, 0, 10_000, 0));
    }

    #[test]
    fn should_auto_compact_char_estimate_covers_post_tool_growth() {
        assert!(should_auto_compact(50, 100, 10_000, 0));
        assert!(!should_auto_compact(50, 100, 100, 0));
    }

    #[test]
    fn should_auto_compact_reserves_incoming_turn_chars() {
        // Old context alone is under the window, but + incoming tips over.
        assert!(!should_auto_compact(0, 100, 80, 0));
        assert!(should_auto_compact(0, 100, 80, 30));
        // Token path: last_token under window until incoming approx tokens added.
        assert!(!should_auto_compact(90, 100, 0, 0));
        let incoming = 4 * 20; // approx_chars_as_tokens → 20
        assert_eq!(approx_chars_as_tokens(incoming), 20);
        assert!(should_auto_compact(90, 100, 0, incoming));
    }

    #[test]
    fn is_summary_message_detects_prefix() {
        assert!(is_summary_message(&format!("{SUMMARY_PREFIX}\nhandoff")));
        assert!(!is_summary_message("please fix the bug"));
    }

    #[test]
    fn collect_user_messages_keeps_text_users_skips_summary_and_tool_results() {
        let messages = vec![
            Message::new_text(Role::User, "goal A"),
            Message::new_text(Role::Assistant, "thinking"),
            Message::new_blocks(
                Role::User,
                vec![ContentBlock::ToolResult {
                    tool_use_id: "t1".into(),
                    content: "file dump".into(),
                }],
            ),
            Message::new_text(Role::User, format!("{SUMMARY_PREFIX}\nold summary")),
            Message::new_text(Role::User, "goal B"),
        ];
        let kept = collect_user_messages(&messages);
        assert_eq!(kept.len(), 2);
        assert!(matches!(
            &kept[0].content,
            tact_llm::MessageContent::Text { content } if content == "goal A"
        ));
        assert!(matches!(
            &kept[1].content,
            tact_llm::MessageContent::Text { content } if content == "goal B"
        ));
    }

    #[test]
    fn build_compacted_history_keeps_tail_users_then_summary() {
        let users = vec![
            Message::new_text(Role::User, "old"),
            Message::new_text(Role::User, "newer"),
        ];
        let history =
            build_compacted_history(&users, "handoff body".into(), KEEP_USER_MESSAGE_CHARS);
        assert_eq!(history.len(), 3);
        assert!(matches!(
            &history[0].content,
            tact_llm::MessageContent::Text { content } if content == "old"
        ));
        assert!(matches!(
            &history[1].content,
            tact_llm::MessageContent::Text { content } if content == "newer"
        ));
        assert!(matches!(
            &history[2].content,
            tact_llm::MessageContent::Text { content }
                if content.starts_with(SUMMARY_PREFIX) && content.contains("handoff body")
        ));
    }

    #[test]
    fn build_compacted_history_respects_char_budget_from_tail() {
        let users = vec![
            Message::new_text(Role::User, "aaaaaaaaaa"), // 10
            Message::new_text(Role::User, "bbbbbbbbbb"), // 10
            Message::new_text(Role::User, "cccccccccc"), // 10
        ];
        // Only enough room for the last message (+ maybe truncated earlier).
        let history = build_compacted_history(&users, "sum".into(), 10);
        assert_eq!(history.len(), 2); // one retained user + summary
        assert!(matches!(
            &history[0].content,
            tact_llm::MessageContent::Text { content } if content == "cccccccccc"
        ));
    }

    #[test]
    fn estimate_message_size_counts_serialized_chars() {
        let msg = Message::new_text(Role::User, "hi");
        let n = estimate_message_size(&msg);
        assert!(n > 0);
        assert_eq!(n, estimate_context_size(std::slice::from_ref(&msg)));
    }
}
