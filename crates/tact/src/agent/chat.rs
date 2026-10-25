//! One user turn, as a host sees it.
//!
//! This is the lifecycle the command driver was doing inline: assemble the
//! message, run the loop, continue past a Stop hook that blocks, publish the
//! completion. Lifting it out is what lets a view drive a turn without holding
//! the agent's managers.
//!
//! It shares the agent slot with [`crate::agent::service::RuntimeAgentService`],
//! so a turn and a compaction serialise against each other. That is required, not
//! convenient: both rewrite the conversation.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use anyhow::Result;
use tact_kernel::runtime::{ChatService, TurnRequest};
use tact_llm::{Message, Role};
use tact_protocol::{AgentErrorKind, AgentUpdate};

use crate::agent::Agent;
use crate::hook::HookControl;

/// How many times a Stop hook may ask for another turn.
///
/// Bounded so a misbehaving hook cannot spin the agent forever. Codex's
/// continuation-fragment semantics let the block reason become the next prompt,
/// which is only safe with a ceiling.
const MAX_STOP_CONTINUATIONS: u32 = 4;

/// The chat service: the turn lifecycle over a shared agent slot.
pub struct RuntimeChatService {
    agent: Arc<tokio::sync::Mutex<Option<Agent>>>,
    cancel_flag: Arc<std::sync::atomic::AtomicBool>,
}

impl RuntimeChatService {
    /// A service over the same slot an [`crate::agent::service::RuntimeAgentService`]
    /// holds.
    pub fn new(
        agent: Arc<tokio::sync::Mutex<Option<Agent>>>,
        cancel_flag: Arc<std::sync::atomic::AtomicBool>,
    ) -> Self {
        Self { agent, cancel_flag }
    }

    /// A service that shares `agent`'s slot.
    pub fn from_agent_service(service: &crate::agent::service::RuntimeAgentService) -> Self {
        Self::new(service.shared_slot(), service.cancel_flag())
    }
}

/// The user message a request becomes.
fn turn_message(request: &TurnRequest) -> Message {
    if request.images.is_empty() {
        return Message::new_text(Role::User, request.input.clone());
    }
    let mut blocks: Vec<tact_llm::ContentBlock> = Vec::new();
    if !request.input.is_empty() {
        blocks.push(tact_llm::ContentBlock::Text {
            text: request.input.clone(),
        });
    }
    for image in &request.images {
        blocks.push(tact_llm::ContentBlock::Image {
            source: tact_llm::ImageSource {
                type_: "base64".to_string(),
                media_type: image.media_type.clone(),
                data: image.data.clone(),
            },
        });
    }
    Message::new_blocks(Role::User, blocks)
}

#[async_trait::async_trait]
impl ChatService for RuntimeChatService {
    async fn submit(&self, request: TurnRequest) -> Result<()> {
        // An empty request is not a turn; refusing it here means every caller
        // gets the same answer instead of each deciding for itself.
        if request.is_empty() {
            anyhow::bail!("a turn needs text or an image");
        }
        let message = turn_message(&request);

        let mut guard = self.agent.lock().await;
        let agent = guard
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("the agent is not installed yet"))?;

        let mut next = Some(message);
        let mut continuations = 0u32;
        loop {
            match agent.agent_loop(next.take()).await {
                Ok(()) if !self.cancel_flag.load(Ordering::Relaxed) => {
                    match agent.dispatch_stop_hooks().await {
                        Ok(HookControl::Block(reason))
                            if continuations < MAX_STOP_CONTINUATIONS =>
                        {
                            continuations += 1;
                            agent.emit_update(AgentUpdate::Info(format!(
                                "[Stop hook] continuing: {reason}"
                            )));
                            // The block reason becomes the next prompt, which is
                            // what makes a Stop hook able to steer one more turn.
                            next = Some(Message::new_text(Role::User, reason));
                            continue;
                        }
                        Ok(HookControl::Block(reason)) => {
                            agent.emit_update(AgentUpdate::Info(format!(
                                "[Stop hook] continuation limit reached; stopping: {reason}"
                            )));
                        }
                        // `allow` means "nothing to add", the same as `continue`.
                        Ok(HookControl::Continue | HookControl::Allow) | Err(_) => {}
                    }
                    if let Some(last) = agent.runtime.context.last() {
                        let text = crate::extract_text(&last.content);
                        agent.emit_update(AgentUpdate::TaskComplete(text));
                    }
                    if let Err(error) = agent.dispatch_task_completed_hooks().await {
                        agent.emit_update(AgentUpdate::Info(format!(
                            "[TaskCompleted hook failed] {error}"
                        )));
                    }
                }
                Ok(()) => {
                    // Cancelled: the view clears its busy state so queued
                    // messages are flushed rather than waiting on a stale one.
                    agent.emit_update(AgentUpdate::TaskCancelled);
                }
                Err(error) => {
                    agent.emit_update(AgentUpdate::Error(AgentErrorKind::Other(error.to_string())));
                }
            }
            break;
        }
        Ok(())
    }

    fn cancel(&self) {
        // Never awaits: a turn holds the lock for its whole duration.
        self.cancel_flag.store(true, Ordering::Relaxed);
    }

    async fn compact(&self) -> Result<()> {
        let mut guard = self.agent.lock().await;
        let agent = guard
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("the agent is not installed yet"))?;
        agent.compact_history(None).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A text-only request becomes one user message.
    #[test]
    fn a_text_request_becomes_a_user_message() {
        let message = turn_message(&TurnRequest::text("hello"));
        assert_eq!(message.role, Role::User);
        assert!(matches!(
            &message.content,
            tact_llm::MessageContent::Text { content } if content == "hello"
        ));
    }

    /// An image request carries both parts as blocks.
    #[test]
    fn an_image_request_carries_both_parts() {
        let request = TurnRequest {
            input: "what is this".to_string(),
            images: vec![tact_kernel::runtime::TurnImage {
                media_type: "image/png".to_string(),
                data: "AAAA".to_string(),
            }],
        };
        let message = turn_message(&request);
        let tact_llm::MessageContent::Blocks { content } = &message.content else {
            panic!("expected blocks");
        };
        assert_eq!(content.len(), 2, "{content:?}");
    }

    /// An image-only request has no empty text block.
    ///
    /// Some providers reject a message whose only text block is empty.
    #[test]
    fn an_image_only_request_has_no_empty_text_block() {
        let request = TurnRequest {
            input: String::new(),
            images: vec![tact_kernel::runtime::TurnImage {
                media_type: "image/png".to_string(),
                data: "AAAA".to_string(),
            }],
        };
        let message = turn_message(&request);
        let tact_llm::MessageContent::Blocks { content } = &message.content else {
            panic!("expected blocks");
        };
        assert_eq!(content.len(), 1, "{content:?}");
    }

    /// Submitting before an agent is installed fails by name.
    #[tokio::test]
    async fn a_turn_before_installation_names_the_problem() {
        let service = RuntimeChatService::new(
            Arc::new(tokio::sync::Mutex::new(None)),
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
        );
        let error = match service.submit(TurnRequest::text("hi")).await {
            Ok(()) => panic!("no agent is installed, so this must fail"),
            Err(error) => error,
        };
        assert!(format!("{error}").contains("not installed"), "{error}");
    }

    /// Cancelling sets the flag without waiting for the turn to finish.
    ///
    /// A cancel that waited for the lock would arrive only after the turn it was
    /// meant to stop.
    #[tokio::test]
    async fn cancel_does_not_wait_for_the_lock() {
        let flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let slot = Arc::new(tokio::sync::Mutex::new(None));
        let service = RuntimeChatService::new(slot.clone(), flag.clone());

        // Hold the lock, as a running turn would.
        let _held = slot.lock().await;
        service.cancel();
        assert!(flag.load(Ordering::Relaxed), "the flag is set regardless");
    }
}
