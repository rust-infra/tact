//! `/compact` as a catalog command.
//!
//! It used to be a `UserCommand` variant, which meant the view had to know the
//! command existed and the driver had to know how to run it. As a catalog command
//! it is listed, completed and dispatched like every other one, and the transport
//! loses a variant.
//!
//! The command does not touch the agent directly: it asks the chat service, which
//! owns the turn lifecycle and holds the same slot the driver does. That is what
//! keeps the lock discipline in one place — a command that locked the slot itself
//! could deadlock against a turn the driver had already started.

use async_trait::async_trait;
use tact_kernel::capability::command::{
    Command, CommandInvocation, CommandMeta, CommandOutcome, CommandScope,
};

/// Compact the conversation now.
pub struct CompactCommand {
    meta: CommandMeta,
    chat: std::sync::Arc<dyn tact_kernel::runtime::ChatService>,
}

impl CompactCommand {
    /// A command over the given turn lifecycle.
    pub fn new(chat: std::sync::Arc<dyn tact_kernel::runtime::ChatService>) -> Self {
        Self {
            meta: CommandMeta {
                name: "compact".to_string(),
                aliases: Vec::new(),
                summary: "Compact the conversation now".to_string(),
                usage: "/compact".to_string(),
                scope: CommandScope::Runtime,
                // No arguments, so Enter runs it rather than completing.
                needs_args: false,
            },
            chat,
        }
    }
}

#[async_trait]
impl Command for CompactCommand {
    fn meta(&self) -> &CommandMeta {
        &self.meta
    }

    async fn run_async(&self, invocation: &CommandInvocation) -> CommandOutcome {
        // Refusing an argument is better than ignoring one: a user who typed
        // `/compact now` should be told the flag does not exist.
        if !invocation.args.trim().is_empty() {
            return CommandOutcome::Usage(format!(
                "`/compact` takes no arguments (got `{}`)",
                invocation.args.trim()
            ));
        }
        match self.chat.compact().await {
            Ok(()) => CommandOutcome::Message("Compaction complete.".to_string()),
            Err(error) => CommandOutcome::Failed(format!("Compaction failed: {error}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A chat service that records how many times it was asked to compact.
    struct CountingChat {
        calls: AtomicUsize,
        fail: bool,
    }

    #[async_trait]
    impl tact_kernel::runtime::ChatService for CountingChat {
        async fn submit(&self, _message: tact_kernel::runtime::TurnRequest) -> anyhow::Result<()> {
            Ok(())
        }

        fn cancel(&self) {}

        async fn compact(&self) -> anyhow::Result<()> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            if self.fail {
                anyhow::bail!("the summary call failed");
            }
            Ok(())
        }
    }

    /// Running the command asks the service to compact, once.
    #[tokio::test]
    async fn it_asks_the_service_to_compact() {
        let chat = std::sync::Arc::new(CountingChat {
            calls: AtomicUsize::new(0),
            fail: false,
        });
        let command = CompactCommand::new(chat.clone());
        let outcome = command
            .run_async(&CommandInvocation {
                name: "compact".to_string(),
                args: String::new(),
            })
            .await;
        assert!(
            matches!(&outcome, CommandOutcome::Message(text) if text.contains("complete")),
            "{outcome:?}"
        );
        assert_eq!(chat.calls.load(Ordering::Relaxed), 1);
    }

    /// An argument is refused by name rather than ignored.
    #[tokio::test]
    async fn an_argument_is_refused() {
        let chat = std::sync::Arc::new(CountingChat {
            calls: AtomicUsize::new(0),
            fail: false,
        });
        let command = CompactCommand::new(chat.clone());
        let outcome = command
            .run_async(&CommandInvocation {
                name: "compact".to_string(),
                args: "now".to_string(),
            })
            .await;
        assert!(
            matches!(&outcome, CommandOutcome::Usage(text) if text.contains("now")),
            "{outcome:?}"
        );
        assert_eq!(
            chat.calls.load(Ordering::Relaxed),
            0,
            "a refused invocation does not compact"
        );
    }

    /// A failed compaction reaches the user as a failure, not as success.
    #[tokio::test]
    async fn a_failure_is_reported_as_one() {
        let chat = std::sync::Arc::new(CountingChat {
            calls: AtomicUsize::new(0),
            fail: true,
        });
        let command = CompactCommand::new(chat);
        let outcome = command
            .run_async(&CommandInvocation {
                name: "compact".to_string(),
                args: String::new(),
            })
            .await;
        assert!(
            matches!(&outcome, CommandOutcome::Failed(text) if text.contains("failed")),
            "{outcome:?}"
        );
    }

    /// The metadata is what the palette shows.
    #[test]
    fn the_metadata_says_what_it_does() {
        let chat = std::sync::Arc::new(CountingChat {
            calls: AtomicUsize::new(0),
            fail: false,
        });
        let meta = CompactCommand::new(chat).meta().clone();
        assert_eq!(meta.name, "compact");
        assert!(!meta.needs_args, "no arguments, so Enter runs it");
        assert_eq!(meta.scope, CommandScope::Runtime);
    }
}
