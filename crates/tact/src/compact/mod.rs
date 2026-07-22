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

use std::{
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::Context as _;
use tact_llm::{ContentBlock, Message, MessageContent, Role};
use tokio::io::AsyncWriteExt;

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
const _: () = assert!(COMPACTED_TOOL_RESULT.len() < 120);
const OMITTED_IMAGE: &str = "[Earlier image attachment omitted during compaction.]";
const MAX_COMPACT_ARTIFACTS: usize = 100;

/// Lead-in for compaction handoff messages. Used both as the summary body
/// prefix and to detect prior summaries so they are not stacked.
pub const SUMMARY_PREFIX: &str =
    "This conversation was compacted so the agent can continue working.";

/// Maximum estimated tokens of recent real-user messages to retain when
/// rebuilding compacted history (roughly 80k ASCII characters).
pub const KEEP_USER_MESSAGE_TOKENS: usize = 20_000;
const COMPACT_REBUILD_HEADROOM_PERCENT: usize = 20;
const AUTO_COMPACT_THRESHOLD_PERCENT: usize = 80;

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

/// Conservative token estimate: ASCII averages four characters per token,
/// while non-ASCII characters count as one token each.
pub(crate) fn approx_text_tokens(text: &str) -> usize {
    let (ascii, non_ascii) = text.chars().fold((0usize, 0usize), |counts, ch| {
        if ch.is_ascii() {
            (counts.0.saturating_add(1), counts.1)
        } else {
            (counts.0, counts.1.saturating_add(1))
        }
    });
    ascii.div_ceil(4).saturating_add(non_ascii)
}

/// Estimates the token size of the serialized message list.
///
/// On serialization failure, returns a large sentinel so auto-compact prefers
/// to run rather than underestimating and overflowing the provider.
pub fn estimate_context_tokens(messages: &[Message]) -> usize {
    match serde_json::to_string(messages) {
        Ok(serialized) => approx_text_tokens(&serialized),
        Err(error) => {
            tracing::warn!(
                %error,
                message_count = messages.len(),
                "failed to serialize context for token estimate; treating as oversized"
            );
            usize::MAX / 2
        }
    }
}

/// Estimated tokens of a single serialized message.
pub fn estimate_message_tokens(message: &Message) -> usize {
    estimate_context_tokens(std::slice::from_ref(message))
}

/// Token budget available for retained real-user messages after reserving
/// output tokens, system/tool/summary input, and safety headroom.
pub(crate) fn retained_user_message_token_budget(
    model_context_window: usize,
    max_output_tokens: usize,
    non_retained_input_tokens: usize,
) -> usize {
    if model_context_window == 0 {
        return KEEP_USER_MESSAGE_TOKENS;
    }
    let headroom_tokens = compact_rebuild_headroom_tokens(model_context_window);
    let reserved_tokens = max_output_tokens
        .saturating_add(non_retained_input_tokens)
        .saturating_add(headroom_tokens);
    model_context_window
        .saturating_sub(reserved_tokens)
        .min(KEEP_USER_MESSAGE_TOKENS)
}

pub(crate) fn compact_rebuild_headroom_tokens(model_context_window: usize) -> usize {
    model_context_window
        .saturating_mul(COMPACT_REBUILD_HEADROOM_PERCENT)
        .div_ceil(100)
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

fn is_real_user_message(message: &Message) -> bool {
    if !matches!(message.role, Role::User) {
        return false;
    }
    match &message.content {
        MessageContent::Text { content } => !is_summary_message(content),
        MessageContent::Blocks { content } => content
            .iter()
            .any(|block| !matches!(block, ContentBlock::ToolResult { .. })),
    }
}

fn user_message_tokens(message: &Message) -> usize {
    estimate_message_tokens(message)
}

fn block_text_tail(message: &Message, max_tokens: usize) -> Option<Message> {
    let MessageContent::Blocks { content } = &message.content else {
        return None;
    };
    let text = content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<String>();
    if text.is_empty() {
        (approx_text_tokens(OMITTED_IMAGE) <= max_tokens)
            .then(|| Message::new_text(Role::User, OMITTED_IMAGE))
    } else {
        Some(Message::new_text(
            Role::User,
            take_last_tokens(&text, max_tokens),
        ))
    }
}

fn summary_message_fallback(message: &Message, max_tokens: usize) -> Option<Message> {
    if max_tokens == 0 {
        return None;
    }
    let text = match &message.content {
        MessageContent::Text { content } => content.clone(),
        MessageContent::Blocks { content } => {
            let mut parts = Vec::with_capacity(content.len());
            for block in content {
                match block {
                    ContentBlock::Text { text } | ContentBlock::Thinking { thinking: text, .. } => {
                        parts.push(text.clone());
                    }
                    ContentBlock::Image { .. } => parts.push(OMITTED_IMAGE.to_string()),
                    ContentBlock::ToolUse { name, .. } => {
                        parts.push(format!("[Tool call: {name}]"));
                    }
                    ContentBlock::ToolResult {
                        tool_use_id,
                        content,
                    } => parts.push(format!("[Tool result {tool_use_id}]\n{content}")),
                    ContentBlock::RedactedThinking { .. } => {
                        parts.push("[Redacted thinking omitted.]".to_string());
                    }
                }
            }
            parts.join("\n")
        }
    };
    if text.is_empty() {
        return None;
    }

    let mut content_budget = max_tokens;
    loop {
        let candidate = Message::new_text(message.role, take_last_tokens(&text, content_budget));
        let tokens = estimate_message_tokens(&candidate);
        if tokens <= max_tokens {
            return Some(candidate);
        }
        let next_budget = content_budget.saturating_sub(tokens.saturating_sub(max_tokens).max(1));
        if next_budget == content_budget || next_budget == 0 {
            return None;
        }
        content_budget = next_budget;
    }
}

/// Serializes the most recent messages that fit a summary-input token budget.
/// Oversized messages are converted to a text-only view so image base64 and
/// large structured payloads are never sliced into invalid JSON.
pub(crate) fn recent_messages_for_summary(
    messages: &[Message],
    max_tokens: usize,
) -> anyhow::Result<String> {
    if messages.is_empty() || max_tokens == 0 {
        return Ok("[]".to_string());
    }

    let mut remaining = max_tokens.saturating_sub(1);
    let mut selected = Vec::with_capacity(messages.len().min(64));
    for message in messages.iter().rev() {
        let tokens = estimate_message_tokens(message);
        if tokens <= remaining {
            selected.push(message.clone());
            remaining = remaining.saturating_sub(tokens);
        } else if let Some(fallback) = summary_message_fallback(message, remaining) {
            remaining = remaining.saturating_sub(estimate_message_tokens(&fallback));
            selected.push(fallback);
        } else if selected.is_empty() {
            continue;
        } else {
            break;
        }
    }
    // Reverses the selected messages so they are in chronological order.
    selected.reverse();

    while !selected.is_empty() {
        let serialized = serde_json::to_string(&selected)
            .context("failed to serialize messages for compact prompt")?;
        if approx_text_tokens(&serialized) <= max_tokens {
            return Ok(serialized);
        }
        tracing::warn!(
            prompt_tokens = approx_text_tokens(&serialized),
            needed_tokens = max_tokens,
            "compact summary prompt too large, dropping the oldest message"
        );
        // Removes the oldest message and tries again.
        selected.remove(0);
    }
    Ok("[]".to_string())
}

/// Longest text tail that fits the conservative token estimate.
fn take_last_tokens(text: &str, max_tokens: usize) -> String {
    let mut ascii = 0usize;
    let mut non_ascii = 0usize;
    let mut start = text.len();
    for (index, ch) in text.char_indices().rev() {
        let (next_ascii, next_non_ascii) = if ch.is_ascii() {
            (ascii.saturating_add(1), non_ascii)
        } else {
            (ascii, non_ascii.saturating_add(1))
        };
        if next_ascii.div_ceil(4).saturating_add(next_non_ascii) > max_tokens {
            break;
        }
        ascii = next_ascii;
        non_ascii = next_non_ascii;
        start = index;
    }
    text[start..].to_string()
}

/// Collect real user turns, skipping tool-result-only users and prior summaries.
pub fn collect_user_messages(messages: &[Message]) -> Vec<Message> {
    let mut kept = Vec::new();
    for message in messages {
        if !is_real_user_message(message) {
            continue;
        }
        kept.push(message.clone());
    }
    kept
}

/// Rebuild compacted history: recent real user messages (token budget from the
/// tail) followed by a single summary user message.
///
/// When a single user message exceeds the remaining budget, its **tail** is
/// kept (most recent content within that turn).
pub fn build_compacted_history(
    user_messages: &[Message],
    summary_text: String,
    max_tokens: usize,
) -> Vec<Message> {
    let mut selected: Vec<Message> = Vec::with_capacity(user_messages.len().saturating_add(1));
    if max_tokens > 0 {
        let mut remaining = max_tokens;
        for message in user_messages.iter().rev() {
            if remaining == 0 {
                break;
            }
            let tokens = user_message_tokens(message);
            if tokens <= remaining {
                selected.push(message.clone());
                remaining = remaining.saturating_sub(tokens);
            } else if let Some(text) = user_text_content(message) {
                selected.push(Message::new_text(
                    Role::User,
                    take_last_tokens(text, remaining),
                ));
                break;
            } else if let Some(truncated) = block_text_tail(message, remaining) {
                selected.push(truncated);
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
/// `<workdir>/.tact/transcripts/`.  Returns the written file path.
pub async fn write_transcript(
    tact_path: &TactPath,
    messages: &[Message],
) -> anyhow::Result<PathBuf> {
    let transcript_dir = tact_path.transcript_dir();
    tokio::fs::create_dir_all(&transcript_dir)
        .await
        .with_context(|| format!("failed to create {}", transcript_dir.display()))?;

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before UNIX_EPOCH")?
        .as_nanos();
    let mut created = None;
    for collision in 0..100u8 {
        let path = transcript_dir.join(format!("transcript_{timestamp}_{collision}.jsonl"));
        match tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .await
        {
            Ok(file) => {
                created = Some((path, file));
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(error).with_context(|| format!("failed to create {}", path.display()));
            }
        }
    }
    let (transcript_path, file) = created.context("failed to allocate a unique transcript name")?;
    let mut writer = tokio::io::BufWriter::new(file);
    for message in messages {
        let line = serde_json::to_string(message).with_context(|| {
            format!(
                "failed to serialize message to {}",
                transcript_path.display()
            )
        })?;
        writer.write_all(line.as_bytes()).await?;
        writer.write_all(b"\n").await?;
    }
    writer.flush().await?;
    prune_compact_artifacts(&transcript_dir, MAX_COMPACT_ARTIFACTS, &transcript_path).await?;
    Ok(transcript_path)
}

/// If `output` exceeds [`PERSIST_THRESHOLD`] characters, writes it to disk
/// under `<workdir>/.tact/tool-results/` and returns a preview with the file path.
/// Otherwise returns the output unchanged.
pub async fn persist_large_output(
    tact_path: &TactPath,
    tool_use_id: &str,
    output: &str,
) -> anyhow::Result<String> {
    if output.chars().count() <= PERSIST_THRESHOLD {
        return Ok(output.to_string());
    }

    let output_dir = tact_path.tool_results_dir();
    tokio::fs::create_dir_all(&output_dir)
        .await
        .with_context(|| format!("failed to create {}", output_dir.display()))?;
    let output_path = output_dir.join(format!("{tool_use_id}.txt"));

    tokio::fs::write(&output_path, output)
        .await
        .with_context(|| format!("failed to write {}", output_path.display()))?;
    prune_compact_artifacts(&output_dir, MAX_COMPACT_ARTIFACTS, &output_path).await?;
    let preview = output.chars().take(PREVIEW_CHARS).collect::<String>();
    Ok(format!(
        "<persisted-output>\nFull output saved to: {}\nPreview:\n{}\n</persisted-output>",
        output_path.display(),
        preview
    ))
}

/// Prunes compact artifacts from the output directory, keeping only the most recent `max_files` files.
async fn prune_compact_artifacts(
    dir: &Path,
    max_files: usize,
    preserve: &Path,
) -> anyhow::Result<()> {
    let mut entries = tokio::fs::read_dir(dir)
        .await
        .with_context(|| format!("failed to read {}", dir.display()))?;
    let mut files = Vec::new();
    while let Some(entry) = entries
        .next_entry()
        .await
        .with_context(|| format!("failed to enumerate {}", dir.display()))?
    {
        let metadata = entry
            .metadata()
            .await
            .with_context(|| format!("failed to inspect {}", entry.path().display()))?;
        if metadata.is_file() {
            files.push((metadata.modified().unwrap_or(UNIX_EPOCH), entry.path()));
        }
    }
    if files.len() <= max_files {
        return Ok(());
    }
    files.sort_unstable_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));
    let remove_count = files.len().saturating_sub(max_files);
    for (_, path) in files
        .into_iter()
        .filter(|(_, path)| path != preserve)
        .take(remove_count)
    {
        tokio::fs::remove_file(&path)
            .await
            .with_context(|| format!("failed to remove old compact artifact {}", path.display()))?;
    }
    Ok(())
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
    let mut positions = Vec::with_capacity(messages.len());
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
/// Triggers when either:
/// - last token total (+ approx tokens for an *incoming* turn not yet in
///   context) reaches the early-compaction threshold, or
/// - estimated context tokens (+ incoming turn tokens) reaches that threshold.
pub(crate) fn should_auto_compact(
    last_token_total: u32,
    model_context_window: usize,
    estimated_context_tokens: usize,
    incoming_turn_tokens: usize,
    max_tokens: usize,
) -> bool {
    if model_context_window == 0 {
        return false;
    }
    let threshold = model_context_window
        .saturating_mul(AUTO_COMPACT_THRESHOLD_PERCENT)
        .div_ceil(100);
    if last_token_total > 0 {
        let projected = (last_token_total as usize)
            .saturating_add(incoming_turn_tokens)
            .saturating_add(max_tokens);
        if projected >= threshold {
            return true;
        }
    }
    estimated_context_tokens
        .saturating_add(incoming_turn_tokens)
        .saturating_add(max_tokens)
        >= threshold
}

#[cfg(test)]
mod tests {
    use tact_llm::{ContentBlock, ImageSource, Message, Role};

    use super::{
        KEEP_USER_MESSAGE_TOKENS, MAX_COMPACT_ARTIFACTS, OMITTED_IMAGE, SUMMARY_PREFIX,
        approx_text_tokens, build_compacted_history, collect_user_messages,
        estimate_context_tokens, estimate_message_tokens, is_summary_message, persist_large_output,
        recent_messages_for_summary, retained_user_message_token_budget, should_auto_compact,
        take_last_tokens, write_transcript,
    };

    #[test]
    fn should_auto_compact_uses_last_token_total_when_present() {
        assert!(!should_auto_compact(79, 100, 0, 0, 0));
        assert!(should_auto_compact(80, 100, 0, 0, 0));
        assert!(should_auto_compact(150, 100, 0, 0, 0));
        assert!(!should_auto_compact(1, 0, 10_000, 0, 0));
    }

    #[test]
    fn should_auto_compact_uses_estimated_tokens_when_no_usage() {
        assert!(!should_auto_compact(0, 100, 79, 0, 0));
        assert!(should_auto_compact(0, 100, 80, 0, 0));
        assert!(!should_auto_compact(0, 0, 10_000, 0, 0));
    }

    #[test]
    fn should_auto_compact_token_estimate_covers_post_tool_growth() {
        assert!(should_auto_compact(50, 100, 80, 0, 0));
        assert!(!should_auto_compact(50, 100, 25, 0, 0));
    }

    #[test]
    fn should_auto_compact_reserves_incoming_turn_tokens() {
        assert!(!should_auto_compact(0, 100, 60, 0, 0));
        assert!(should_auto_compact(0, 100, 60, 20, 0));
        // Token path: last_token under window until incoming approx tokens added.
        assert!(!should_auto_compact(70, 100, 0, 0, 0));
        assert!(should_auto_compact(70, 100, 0, 10, 0));
    }

    #[test]
    fn should_auto_compact_reserves_max_output_tokens() {
        // Estimate path: 60 + 0 + 20 = 80 >= 80, triggers because max_tokens pushes over.
        assert!(!should_auto_compact(0, 100, 60, 0, 0));
        assert!(should_auto_compact(0, 100, 60, 0, 20));
        // Token path: last_token_total 60 + incoming 0 + max_tokens 20 = 80 >= 80.
        assert!(!should_auto_compact(60, 100, 0, 0, 0));
        assert!(should_auto_compact(60, 100, 0, 0, 20));
        // Both axes together: 40 context + 10 incoming + 30 max = 80 >= 80.
        assert!(!should_auto_compact(0, 100, 40, 0, 0));
        assert!(!should_auto_compact(0, 100, 40, 10, 0));
        assert!(should_auto_compact(0, 100, 40, 10, 30));
        // max_tokens alone should not trigger when window is disabled.
        assert!(!should_auto_compact(0, 0, 0, 0, 100));
    }

    #[test]
    fn retained_user_budget_uses_window_reservations_and_hard_cap() {
        assert_eq!(
            retained_user_message_token_budget(200_000, 8_000, 0),
            KEEP_USER_MESSAGE_TOKENS
        );
        // 20k window - 8k output - 2k input overhead - 4k headroom = 6k.
        assert_eq!(
            retained_user_message_token_budget(20_000, 8_000, 2_000),
            6_000
        );
        // Reservations consume the full window, so no verbatim users remain.
        assert_eq!(retained_user_message_token_budget(10_000, 8_000, 2_000), 0);
    }

    #[test]
    fn retained_user_budget_keeps_legacy_cap_when_window_is_disabled() {
        assert_eq!(
            retained_user_message_token_budget(0, usize::MAX, usize::MAX),
            KEEP_USER_MESSAGE_TOKENS
        );
    }

    #[test]
    fn token_estimate_is_conservative_for_non_ascii_text() {
        assert_eq!(approx_text_tokens("abcdefgh"), 2);
        assert_eq!(approx_text_tokens("中文测试"), 4);
        assert_eq!(approx_text_tokens("abcd中文"), 3);
    }

    #[test]
    fn is_summary_message_detects_prefix() {
        assert!(is_summary_message(&format!("{SUMMARY_PREFIX}\nhandoff")));
        assert!(!is_summary_message("please fix the bug"));
    }

    #[test]
    fn collect_user_messages_keeps_real_users_skips_summary_and_tool_results() {
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
            Message::new_blocks(
                Role::User,
                vec![ContentBlock::Text {
                    text: "goal from UI".into(),
                }],
            ),
            Message::new_text(Role::User, format!("{SUMMARY_PREFIX}\nold summary")),
            Message::new_text(Role::User, "goal B"),
        ];
        let kept = collect_user_messages(&messages);
        assert_eq!(kept.len(), 3);
        assert!(matches!(
            &kept[0].content,
            tact_llm::MessageContent::Text { content } if content == "goal A"
        ));
        assert!(matches!(
            &kept[1].content,
            tact_llm::MessageContent::Blocks { content }
                if matches!(&content[..], [ContentBlock::Text { text }] if text == "goal from UI")
        ));
        assert!(matches!(
            &kept[2].content,
            tact_llm::MessageContent::Text { content } if content == "goal B"
        ));
    }

    #[test]
    fn build_compacted_history_keeps_block_user_verbatim_when_it_fits() {
        let user = Message::new_blocks(
            Role::User,
            vec![
                ContentBlock::Text {
                    text: "UI prompt".into(),
                },
                ContentBlock::Image {
                    source: ImageSource {
                        type_: "base64".into(),
                        media_type: "image/png".into(),
                        data: "aGVsbG8=".into(),
                    },
                },
            ],
        );
        let budget = estimate_message_tokens(&user);
        let history = build_compacted_history(std::slice::from_ref(&user), "sum".into(), budget);
        assert_eq!(history.len(), 2);
        assert!(matches!(
            &history[0].content,
            tact_llm::MessageContent::Blocks { content }
                if matches!(
                    &content[..],
                    [ContentBlock::Text { text }, ContentBlock::Image { source }]
                        if text == "UI prompt" && source.data == "aGVsbG8="
                )
        ));
    }

    #[test]
    fn build_compacted_history_degrades_oversized_blocks_to_text_tail() {
        let user = Message::new_blocks(
            Role::User,
            vec![ContentBlock::Text {
                text: "abcdefghij".into(),
            }],
        );
        let history = build_compacted_history(&[user], "sum".into(), 1);
        assert_eq!(history.len(), 2);
        assert!(matches!(
            &history[0].content,
            tact_llm::MessageContent::Text { content } if content == "ghij"
        ));
    }

    #[test]
    fn build_compacted_history_replaces_oversized_pure_image() {
        let user = Message::new_blocks(
            Role::User,
            vec![ContentBlock::Image {
                source: ImageSource {
                    type_: "base64".into(),
                    media_type: "image/png".into(),
                    data: "a".repeat(1_000),
                },
            }],
        );
        let history =
            build_compacted_history(&[user], "sum".into(), approx_text_tokens(OMITTED_IMAGE));
        assert!(matches!(
            &history[0].content,
            tact_llm::MessageContent::Text { content } if content == OMITTED_IMAGE
        ));
    }

    #[test]
    fn summary_input_omits_oversized_image_base64_but_keeps_placeholder() {
        let message = Message::new_blocks(
            Role::User,
            vec![ContentBlock::Image {
                source: ImageSource {
                    type_: "base64".into(),
                    media_type: "image/png".into(),
                    data: "a".repeat(10_000),
                },
            }],
        );
        let serialized = recent_messages_for_summary(&[message], 100).unwrap();
        assert!(serialized.contains(OMITTED_IMAGE));
        assert!(!serialized.contains(&"a".repeat(100)));
        assert!(approx_text_tokens(&serialized) <= 100);
    }

    #[test]
    fn build_compacted_history_keeps_tail_users_then_summary() {
        let users = vec![
            Message::new_text(Role::User, "old"),
            Message::new_text(Role::User, "newer"),
        ];
        let history =
            build_compacted_history(&users, "handoff body".into(), KEEP_USER_MESSAGE_TOKENS);
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
    fn build_compacted_history_respects_token_budget_from_tail() {
        let users = vec![
            Message::new_text(Role::User, "aaaaaaaaaa"), // 10
            Message::new_text(Role::User, "bbbbbbbbbb"), // 10
            Message::new_text(Role::User, "cccccccccc"), // 10
        ];
        // Only enough room for the last message (+ maybe truncated earlier).
        let history = build_compacted_history(&users, "sum".into(), 3);
        assert_eq!(history.len(), 2); // one retained user + summary
        assert!(matches!(
            &history[0].content,
            tact_llm::MessageContent::Text { content } if content == "cccccccccc"
        ));
    }

    #[test]
    fn build_compacted_history_truncates_oversized_message_keeping_tail() {
        let users = vec![Message::new_text(Role::User, "abcdefghij")]; // 10
        let history = build_compacted_history(&users, "sum".into(), 1);
        assert_eq!(history.len(), 2);
        assert!(matches!(
            &history[0].content,
            tact_llm::MessageContent::Text { content } if content == "ghij"
        ));
        assert_eq!(take_last_tokens("abcdefghij", 1), "ghij");
    }

    #[test]
    fn estimate_message_tokens_counts_serialized_content() {
        let msg = Message::new_text(Role::User, "hi");
        let n = estimate_message_tokens(&msg);
        assert!(n > 0);
        assert_eq!(n, estimate_context_tokens(std::slice::from_ref(&msg)));
    }

    #[tokio::test]
    async fn transcript_names_are_unique_for_rapid_writes() {
        let dir = tempfile::tempdir().unwrap();
        let tact_path = crate::consts::TactPath::new(dir.path());
        let messages = [Message::new_text(Role::User, "hello")];
        let first = write_transcript(&tact_path, &messages).await.unwrap();
        let second = write_transcript(&tact_path, &messages).await.unwrap();
        assert_ne!(first, second);
        assert!(first.exists());
        assert!(second.exists());
    }

    #[tokio::test]
    async fn compact_artifacts_are_pruned_after_writes() {
        let dir = tempfile::tempdir().unwrap();
        let tact_path = crate::consts::TactPath::new(dir.path());
        let output_dir = tact_path.tool_results_dir();
        tokio::fs::create_dir_all(&output_dir).await.unwrap();
        for index in 0..MAX_COMPACT_ARTIFACTS {
            tokio::fs::write(output_dir.join(format!("old-{index}.txt")), "old")
                .await
                .unwrap();
        }
        let large = "x".repeat(30_001);
        persist_large_output(&tact_path, "new", &large)
            .await
            .unwrap();
        let mut entries = tokio::fs::read_dir(&output_dir).await.unwrap();
        let mut count = 0;
        while entries.next_entry().await.unwrap().is_some() {
            count += 1;
        }
        assert_eq!(count, MAX_COMPACT_ARTIFACTS);
        assert!(output_dir.join("new.txt").exists());
    }
}
