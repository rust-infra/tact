//! Persisted conversation history, for front ends that redraw a transcript.
//!
//! The store keeps the canonical block vector for every logical message, so a
//! window that reopens a session can redraw it instead of starting on a blank
//! page. This module flattens those rows into a presentation-neutral shape: the
//! desktop client maps it onto its own rows and neither front end has to depend
//! on the store or the LLM content types.
//!
//! Redrawing is honest about what was never stored. A live transcript carries
//! tool durations, request cards and progress lines that are presentation
//! state, not conversation state, so a reopened session shows the blocks and
//! nothing more.

use std::path::Path;

use serde_json::Value;
use tact::consts::TactPath;
use tact_llm::{ContentBlock, Message, MessageContent, Role};

/// Who wrote a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryRole {
    User,
    Assistant,
}

/// One block of a message, in producer order.
///
/// Order matters: it is what lets a front end redraw `thinking -> tool ->
/// answer` in the sequence the model actually produced it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HistoryBlock {
    /// Prose the user typed or the assistant streamed.
    Text(String),
    /// Reasoning the model emitted before answering.
    Thinking(String),
    /// A tool invocation. `detail` is a one-line summary of its input.
    ToolUse {
        id: String,
        name: String,
        detail: String,
    },
    /// The result of `tool_use_id`, which always follows its [`HistoryBlock::ToolUse`].
    ToolResult { tool_use_id: String, output: String },
}

/// One logical message with its blocks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryMessage {
    pub role: HistoryRole,
    pub blocks: Vec<HistoryBlock>,
}

impl HistoryMessage {
    /// Whether this message holds anything a transcript can draw.
    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }
}

/// Longest tool-input summary a redrawn card shows before eliding.
const DETAIL_LIMIT: usize = 120;

/// The persisted conversation for `session_id`, oldest first.
///
/// An unknown session is an empty conversation rather than an error: a front
/// end asking for a session it just created should draw a blank page, not fail.
/// A session that does not exist in this workspace *is* an error, because that
/// means the caller asked the wrong store.
pub fn history(workdir: &Path, session_id: &str) -> anyhow::Result<Vec<HistoryMessage>> {
    let tact_path = TactPath::new(workdir.to_path_buf());
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;

    runtime.block_on(async move {
        let store = tact::store::open_sqlite_session_store(&tact_path.session_db_path()).await?;
        let messages = store.load_session(session_id).await?;
        Ok(messages.into_iter().map(convert).collect())
    })
}

/// Reshape one stored message into the shared history type.
fn convert(message: Message) -> HistoryMessage {
    let role = match message.role {
        Role::User => HistoryRole::User,
        Role::Assistant => HistoryRole::Assistant,
    };
    let blocks = match message.content {
        MessageContent::Text { content } => vec![HistoryBlock::Text(content)],
        MessageContent::Blocks { content } => content.into_iter().filter_map(block).collect(),
    };
    HistoryMessage { role, blocks }
}

/// Reshape one stored block, dropping the ones a transcript cannot redraw.
fn block(block: ContentBlock) -> Option<HistoryBlock> {
    match block {
        ContentBlock::Text { text } => Some(HistoryBlock::Text(text)),
        ContentBlock::Thinking { thinking, .. } => Some(HistoryBlock::Thinking(thinking)),
        ContentBlock::ToolUse { id, name, input } => Some(HistoryBlock::ToolUse {
            id,
            name,
            detail: tool_detail(&input),
        }),
        ContentBlock::ToolResult {
            tool_use_id,
            content,
        } => Some(HistoryBlock::ToolResult {
            tool_use_id,
            output: content,
        }),
        // An image attachment and redacted reasoning carry no text a
        // transcript row can show; a replay skips them rather than inventing
        // a placeholder that claims the block was empty.
        ContentBlock::Image { .. } | ContentBlock::RedactedThinking { .. } => None,
    }
}

/// Flatten a tool's raw input into the one line a card shows.
///
/// The live path shows the producer's own `arg_summary`, which is presentation
/// state and is not persisted. Redrawing falls back to the well-known scalar
/// arguments and, failing those, the compact JSON.
fn tool_detail(input: &Value) -> String {
    /// Arguments that read as the tool's subject, most specific first.
    const KEYS: [&str; 8] = [
        "command",
        "cmd",
        "file_path",
        "path",
        "pattern",
        "query",
        "url",
        "prompt",
    ];

    if let Value::Object(map) = input {
        for key in KEYS {
            if let Some(Value::String(value)) = map.get(key) {
                return elide(value);
            }
        }
    }
    match input {
        Value::Null => String::new(),
        Value::String(value) => elide(value),
        other => elide(&other.to_string()),
    }
}

/// Collapse whitespace and cut at [`DETAIL_LIMIT`] characters.
fn elide(text: &str) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= DETAIL_LIMIT {
        return collapsed;
    }
    let mut cut: String = collapsed.chars().take(DETAIL_LIMIT).collect();
    cut.push('…');
    cut
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn temp_workspace() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "tact_session_history_{}_{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).expect("temp workspace");
        dir
    }

    fn write(workspace: &Path, session_id: &str, messages: Vec<Message>) {
        let tact_path = TactPath::new(workspace.to_path_buf());
        let root_dir = workspace.display().to_string();
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime")
            .block_on(async {
                let store = tact::store::open_sqlite_session_store(&tact_path.session_db_path())
                    .await
                    .expect("store");
                store
                    .ensure_session_row(session_id, &root_dir, "")
                    .await
                    .expect("row");
                for (ordinal, message) in messages.iter().enumerate() {
                    store
                        .append_message(session_id, message.role, &message.content, ordinal as i64)
                        .await
                        .expect("append");
                }
            });
    }

    #[test]
    fn a_reopened_session_redraws_thinking_tool_and_answer_in_order() {
        let workspace = temp_workspace();
        let session = "33333333-cccc";
        write(
            &workspace,
            session,
            vec![
                Message::new_text(Role::User, "check the build"),
                Message::new_blocks(
                    Role::Assistant,
                    vec![
                        ContentBlock::Thinking {
                            thinking: "weighing".into(),
                            signature: "sig".into(),
                        },
                        ContentBlock::ToolUse {
                            id: "tool_1".into(),
                            name: "bash".into(),
                            input: serde_json::json!({"command": "cargo test"}),
                        },
                    ],
                ),
                Message::new_blocks(
                    Role::User,
                    vec![ContentBlock::ToolResult {
                        tool_use_id: "tool_1".into(),
                        content: "ok".into(),
                    }],
                ),
                Message::new_text(Role::Assistant, "it passes"),
            ],
        );

        let messages = history(&workspace, session).expect("history");

        assert_eq!(messages.len(), 4, "{messages:?}");
        assert_eq!(messages[0].role, HistoryRole::User);
        assert_eq!(
            messages[0].blocks,
            vec![HistoryBlock::Text("check the build".into())]
        );
        // Block order survives, which is what a front end redraws from.
        assert_eq!(
            messages[1].blocks,
            vec![
                HistoryBlock::Thinking("weighing".into()),
                HistoryBlock::ToolUse {
                    id: "tool_1".into(),
                    name: "bash".into(),
                    detail: "cargo test".into(),
                },
            ]
        );
        assert_eq!(messages[2].role, HistoryRole::User);
        assert_eq!(
            messages[2].blocks,
            vec![HistoryBlock::ToolResult {
                tool_use_id: "tool_1".into(),
                output: "ok".into(),
            }],
            "a tool result is its own user-role message in the store"
        );
        assert_eq!(
            messages[3].blocks,
            vec![HistoryBlock::Text("it passes".into())]
        );

        let _ = std::fs::remove_dir_all(&workspace);
    }

    #[test]
    fn a_session_with_no_messages_is_empty_rather_than_missing() {
        let workspace = temp_workspace();
        let session = "44444444-dddd";
        write(&workspace, session, vec![]);

        let messages = history(&workspace, session).expect("history");

        assert!(
            messages.is_empty(),
            "a fresh session redraws as a blank page"
        );

        let _ = std::fs::remove_dir_all(&workspace);
    }

    #[test]
    fn tool_details_come_from_the_input_and_stay_on_one_line() {
        assert_eq!(
            "cargo test",
            tool_detail(&serde_json::json!({"command": "cargo test"}))
        );
        // A nested or unknown shape falls back to compact JSON rather than
        // dropping the argument entirely.
        assert_eq!(
            r#"{"a":{"b":1}}"#,
            tool_detail(&serde_json::json!({"a": {"b": 1}}))
        );
        assert_eq!("", tool_detail(&Value::Null));
        let long = tool_detail(&serde_json::json!({"command": "x".repeat(400)}));
        assert_eq!(long.chars().count(), DETAIL_LIMIT + 1, "cut plus ellipsis");
        assert!(long.ends_with('…'));
    }

    #[test]
    fn blocks_a_transcript_cannot_draw_are_skipped() {
        let message = convert(Message::new_blocks(
            Role::User,
            vec![
                ContentBlock::Image {
                    source: tact_llm::ImageSource {
                        type_: "base64".into(),
                        media_type: "image/png".into(),
                        data: "AAAA".into(),
                    },
                },
                ContentBlock::RedactedThinking {
                    data: "opaque".into(),
                },
                ContentBlock::Text { text: "hi".into() },
            ],
        ));

        assert_eq!(message.blocks, vec![HistoryBlock::Text("hi".into())]);
        assert!(!message.is_empty());
        assert!(convert(Message::new_blocks(Role::Assistant, vec![])).is_empty());
    }
}
