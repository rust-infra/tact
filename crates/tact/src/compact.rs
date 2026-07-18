//! Context compaction and transcript persistence.
//!
//! When the conversation grows too large this module provides:
//! - [`micro_compact`]: truncates old tool results while keeping the most
//!   recent `KEEP_RECENT_TOOL_RESULTS` entries intact.
//! - [`persist_large_output`]: writes oversized tool outputs to disk and
//!   returns a preview with a file path.
//! - [`write_transcript`]: serialises the entire conversation as JSONL.
//! - [`compacted_context`]: generates a replacement context containing
//!   a summary of what was compacted.

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
/// summary of what was compacted, so the agent can continue without
/// re-reading the full history.
pub fn compacted_context(summary: String) -> Vec<Message> {
    vec![Message::new_text(
        Role::User,
        format!("This conversation was compacted so the agent can continue working.\n\n{summary}"),
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
/// - the last reported token total is already at/over the window, or
/// - the current context's char estimate exceeds the window (covers growth
///   from tool results appended *after* the last TokenUsage update).
pub(crate) fn should_auto_compact(
    last_token_total: u32,
    model_context_window: usize,
    estimated_chars: usize,
) -> bool {
    if model_context_window == 0 {
        return false;
    }
    if last_token_total > 0 && (last_token_total as usize) >= model_context_window {
        return true;
    }
    // Char estimate vs token window is coarse, but catches post-tool growth
    // that last_token_total cannot see until the next LLM response.
    // TODO: replace once we can estimate tokens pre-call.
    estimated_chars > model_context_window
}

#[cfg(test)]
mod tests {
    use super::should_auto_compact;

    #[test]
    fn should_auto_compact_uses_last_token_total_when_present() {
        assert!(!should_auto_compact(99, 100, 0));
        assert!(should_auto_compact(100, 100, 0));
        assert!(should_auto_compact(150, 100, 0));
        assert!(!should_auto_compact(1, 0, 10_000));
    }

    #[test]
    fn should_auto_compact_falls_back_to_chars_when_no_usage() {
        assert!(!should_auto_compact(0, 100, 100));
        assert!(should_auto_compact(0, 100, 101));
        assert!(!should_auto_compact(0, 0, 10_000));
    }

    #[test]
    fn should_auto_compact_char_estimate_covers_post_tool_growth() {
        // last_token_total is still under the window (from the previous LLM
        // call), but tool results since then pushed the char estimate over.
        assert!(should_auto_compact(50, 100, 10_000));
        assert!(!should_auto_compact(50, 100, 100));
    }
}
