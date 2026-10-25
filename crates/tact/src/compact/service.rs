//! Compaction as a replaceable service.
//!
//! The pieces here are the ones that do not need the agent itself: micro-
//! compaction over a message slice, the token estimates that decide when to
//! compact, building the post-compaction history, and writing the transcript.
//! The *orchestration* — deciding to compact, calling the model for a summary,
//! swapping the context — stays on the agent, because it rewrites the agent's own
//! state and cannot be lifted out without moving the loop with it.
//!
//! That split is deliberate and worth stating: a host that wants different
//! compaction behaviour replaces this service and gets different micro-compaction,
//! different history shaping and a different transcript format, while the trigger
//! policy remains the agent's.

use std::path::{Path, PathBuf};

use tact_llm::Message;

use crate::compact::{self, CompactTrigger};

/// The compaction policy a host can replace.
pub trait CompactionService: Send + Sync {
    /// Mark stale tool output as omitted, in place.
    ///
    /// `enabled` is a parameter rather than a field read: the agent decides per
    /// session whether micro-compaction runs, and a service that read a global
    /// could disagree with the session it is compacting.
    fn micro_compact(&self, messages: &mut [Message], enabled: bool);

    /// Estimated tokens the context occupies.
    fn estimate_tokens(&self, messages: &[Message]) -> usize;

    /// The user-authored messages a summary should be built from.
    ///
    /// Assistant turns and injected context are excluded: a summary of a summary
    /// drifts, and injected context is not something the user said.
    fn user_messages(&self, messages: &[Message]) -> Vec<Message>;

    /// Build the history that replaces the compacted one.
    ///
    /// `max_tokens` of `0` means "no user messages" rather than "unlimited": the
    /// caller has already decided the budget, and a zero here is the honest way to
    /// say the summary stands alone.
    fn build_history(
        &self,
        user_messages: &[Message],
        summary: String,
        max_tokens: usize,
        transcript: Option<&Path>,
    ) -> Vec<Message>;

    /// The context a freshly compacted session starts with.
    fn compacted_context(&self, summary: String) -> Vec<Message>;

    /// Write the pre-compaction transcript, returning where it went.
    ///
    /// # Errors
    ///
    /// Returns an error when the transcript directory cannot be created or
    /// written. A compaction that cannot preserve the transcript must fail rather
    /// than silently discard what it was about to replace.
    fn write_transcript<'a>(
        &'a self,
        tact_path: &'a crate::consts::TactPath,
        messages: &'a [Message],
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<PathBuf>> + Send + 'a>>;
}

/// Why a compaction was requested.
///
/// Re-exported through the service so a host that replaces the policy does not
/// have to reach into the compaction module to name a trigger.
pub type Trigger = CompactTrigger;

/// The built-in policy: the crate's existing behaviour, behind the trait.
#[derive(Debug, Default, Clone, Copy)]
pub struct BuiltinCompaction;

impl CompactionService for BuiltinCompaction {
    fn micro_compact(&self, messages: &mut [Message], enabled: bool) {
        compact::micro_compact(messages, enabled);
    }

    fn estimate_tokens(&self, messages: &[Message]) -> usize {
        compact::estimate_context_tokens(messages)
    }

    fn user_messages(&self, messages: &[Message]) -> Vec<Message> {
        compact::collect_user_messages(messages)
    }

    fn build_history(
        &self,
        user_messages: &[Message],
        summary: String,
        max_tokens: usize,
        transcript: Option<&Path>,
    ) -> Vec<Message> {
        compact::build_compacted_history(user_messages, summary, max_tokens, transcript)
    }

    fn compacted_context(&self, summary: String) -> Vec<Message> {
        compact::compacted_context(summary)
    }

    fn write_transcript<'a>(
        &'a self,
        tact_path: &'a crate::consts::TactPath,
        messages: &'a [Message],
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<PathBuf>> + Send + 'a>>
    {
        Box::pin(compact::write_transcript(tact_path, messages))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tact_llm::{ContentBlock, MessageContent, Role};

    fn user(text: &str) -> Message {
        Message::new_text(Role::User, text)
    }

    /// The tool-result text at `index`, if that message is a single result block.
    ///
    /// `extract_text` only reads `Text` blocks, so a test that wants to see what
    /// happened to a tool result has to look at the block itself.
    fn tool_result_text(message: &Message) -> Option<&str> {
        let MessageContent::Blocks { content } = &message.content else {
            return None;
        };
        content.iter().find_map(|block| match block {
            ContentBlock::ToolResult { content, .. } => Some(content.as_str()),
            _ => None,
        })
    }

    /// Micro-compaction honours the caller's flag rather than a global.
    ///
    /// This is the property that lets a session disable it without changing what
    /// any other session does. `Message` has no `PartialEq`, so the check is on
    /// the rendered text: the long tool output either survives verbatim or is
    /// replaced by the omission marker.
    #[test]
    fn micro_compaction_is_off_when_the_caller_says_so() {
        let service = BuiltinCompaction;
        let long = "x".repeat(4000);
        // One more than the keep-recent window, so the oldest entry is eligible.
        // A shorter history is left alone by design: compaction only touches what
        // is no longer recent.
        let build = || {
            let mut messages = vec![user("hi")];
            for index in 0..13 {
                messages.push(Message::new_blocks(
                    Role::User,
                    vec![ContentBlock::ToolResult {
                        tool_use_id: format!("t{index}"),
                        content: long.clone(),
                    }],
                ));
            }
            messages
        };

        let mut untouched = build();
        service.micro_compact(&mut untouched, false);
        assert_eq!(
            tool_result_text(&untouched[1]),
            Some(long.as_str()),
            "disabled must not rewrite anything"
        );

        let mut compacted = build();
        service.micro_compact(&mut compacted, true);
        assert_ne!(
            tool_result_text(&compacted[1]),
            Some(long.as_str()),
            "the oldest result is omitted"
        );
        assert_eq!(
            tool_result_text(&compacted[13]),
            Some(long.as_str()),
            "the most recent result is kept intact"
        );
    }

    /// The estimate is monotone in the number of messages.
    ///
    /// A compaction trigger compares this against a budget, so an estimate that
    /// could shrink as history grows would make the trigger fire less the more
    /// there is to compact.
    #[test]
    fn the_estimate_grows_with_history() {
        let service = BuiltinCompaction;
        let short = service.estimate_tokens(&[user("a")]);
        let long = service.estimate_tokens(&[user("a"), user("b"), user("c")]);
        assert!(long > short, "{long} should exceed {short}");
    }

    /// Only user-authored turns feed a summary.
    ///
    /// Summarising assistant output would let the model's own guesses become
    /// facts in the next summary.
    #[test]
    fn only_user_turns_are_collected() {
        let service = BuiltinCompaction;
        let messages = vec![
            user("mine"),
            Message::new_text(Role::Assistant, "theirs"),
            user("also mine"),
        ];
        let collected = service.user_messages(&messages);
        assert_eq!(collected.len(), 2, "{collected:?}");
        assert!(
            collected.iter().all(|message| message.role == Role::User),
            "no assistant turn may be collected"
        );
    }

    /// A zero budget means the summary stands alone, not "unlimited".
    #[test]
    fn a_zero_budget_keeps_only_the_summary() {
        let service = BuiltinCompaction;
        let history = service.build_history(&[user("one"), user("two")], "sum".into(), 0, None);
        assert_eq!(history.len(), 1, "{history:?}");
        assert!(
            matches!(&history[0].content, MessageContent::Text { content } if content.contains("sum")),
            "the summary is what survives: {:?}",
            history[0].content
        );
    }

    /// A compacted context is the summary alone.
    #[test]
    fn a_compacted_context_holds_the_summary() {
        let service = BuiltinCompaction;
        let context = service.compacted_context("the summary".into());
        assert_eq!(context.len(), 1);
    }
}
