//! The agent loop behind the kernel's [`AgentService`] contract.
//!
//! `Agent::agent_loop_detailed` needs `&mut self`: the loop rewrites the
//! conversation, the recovery state and the tool schedule as it runs. The trait
//! is `&self` because a view holds the service behind an `Arc`. The bridge is a
//! mutex around the agent, which is honest about what it means: one turn at a
//! time, and a second caller waits rather than interleaving.
//!
//! That serialisation is a feature, not a compromise. Two concurrent turns on
//! one conversation would interleave their messages, and the compaction pass of
//! one would rewrite history the other is still reading.

use std::sync::Arc;

use tact_kernel::runtime::{AgentService, TurnOptions, TurnOutcome, TurnRequest, TurnStop};
use tact_llm::{Message, MessageContent};

use crate::agent::{Agent, AgentLoopStop};

/// The agent, as a host-facing service.
///
/// The agent cannot exist before it is assembled, and this service is registered
/// *during* assembly, so the slot starts empty and is filled the moment the agent
/// finishes building itself. Every method reports that state honestly rather than
/// blocking: a caller that reaches the service before the fill is a composition
/// bug, and hanging would hide it.
pub struct RuntimeAgentService {
    agent: Arc<tokio::sync::Mutex<Option<Agent>>>,
    cancel_flag: Arc<std::sync::atomic::AtomicBool>,
}

impl RuntimeAgentService {
    /// A service with no agent yet.
    ///
    /// The cancel flag is captured here, before any agent exists, because
    /// `cancel` must work while a turn holds the lock — a cancel that had to wait
    /// for the lock would arrive only after the turn it was meant to stop.
    pub fn pending(cancel_flag: Arc<std::sync::atomic::AtomicBool>) -> Self {
        Self {
            agent: Arc::new(tokio::sync::Mutex::new(None)),
            cancel_flag,
        }
    }

    /// Put the assembled agent in place.
    ///
    /// Called once by the composition root, right after the agent is built. A
    /// second call would mean two agents claim the same slot, which is why this
    /// is not a silent overwrite.
    pub fn install(&self, agent: Agent) -> anyhow::Result<()> {
        let mut slot = self.agent.try_lock().map_err(|_| {
            anyhow::anyhow!("the agent slot is busy; install must run before any turn")
        })?;
        if slot.is_some() {
            anyhow::bail!("an agent is already installed in this service");
        }
        *slot = Some(agent);
        Ok(())
    }

    /// The cancel flag, so a sibling service cancels the same run.
    pub fn cancel_flag(&self) -> Arc<std::sync::atomic::AtomicBool> {
        self.cancel_flag.clone()
    }

    /// The shared slot, for a sibling service that drives the same agent.
    ///
    /// `ChatService` and `AgentService` must serialise against each other: a turn
    /// and a compaction touching one conversation concurrently would interleave
    /// their messages. Sharing the slot is what makes that structural rather than
    /// a rule someone has to remember.
    pub fn shared_slot(&self) -> Arc<tokio::sync::Mutex<Option<Agent>>> {
        self.agent.clone()
    }

    /// Whether an agent is in place.
    pub fn is_installed(&self) -> bool {
        self.agent
            .try_lock()
            .map(|slot| slot.is_some())
            .unwrap_or(true)
    }
}

/// Map the loop's own stop reasons onto the host-facing ones.
///
/// A separate function rather than a `From` impl: the mapping is the whole
/// contract between the two enums, and putting it here keeps a new
/// `AgentLoopStop` variant from silently gaining a default.
fn to_turn_stop(stop: AgentLoopStop) -> TurnStop {
    match stop {
        AgentLoopStop::Completed => TurnStop::Completed,
        AgentLoopStop::GoalYield(reason) => TurnStop::Yielded(reason),
        AgentLoopStop::MaxTurns => TurnStop::MaxTurns,
        AgentLoopStop::MaxTokens => TurnStop::MaxTokens,
        AgentLoopStop::Cancelled => TurnStop::Cancelled,
        AgentLoopStop::BlockedByHook => TurnStop::BlockedByHook,
    }
}

/// The assistant's last message as plain text, when it has any.
///
/// `None` rather than an empty string when the model produced nothing: the two
/// are different, and a caller that prints them differently needs to see which
/// happened.
fn last_text(agent: &Agent) -> Option<String> {
    let message = agent.runtime.context.last()?;
    let MessageContent::Text { content } = &message.content else {
        // A block message (thinking, tool use, image) is not "no text"; its text
        // parts are what a user would have seen.
        let text = crate::extract_text(&message.content);
        return (!text.trim().is_empty()).then_some(text);
    };
    (!content.trim().is_empty()).then(|| content.clone())
}

#[async_trait::async_trait]
impl AgentService for RuntimeAgentService {
    async fn run(&self, request: TurnRequest, options: TurnOptions) -> anyhow::Result<TurnOutcome> {
        // An empty request would append an empty user message and spend a model
        // call on it; refusing it here means every caller gets the same answer.
        if request.is_empty() {
            anyhow::bail!("a turn needs text or an image");
        }

        let message = turn_message(&request);
        let mut guard = self.agent.lock().await;
        let agent = guard
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("the agent is not installed yet"))?;

        // The per-turn bindings are copied onto the agent for the duration of
        // this turn: the loop reads them from `self` rather than threading a
        // parameter through every dispatch site.
        let previous_max_turns = agent.max_turns;
        if options.max_turns.is_some() {
            agent.max_turns = options.max_turns;
        }
        let previous_goal = agent.tool_context.active_goal_id.clone();
        if options.active_goal_id.is_some() {
            agent.tool_context.active_goal_id = options.active_goal_id.clone();
        }

        let outcome = agent.agent_loop_detailed(Some(message)).await;

        // Restore what this turn overrode, so a caller that sets a limit for one
        // turn does not silently keep it for the next.
        agent.max_turns = previous_max_turns;
        agent.tool_context.active_goal_id = previous_goal;

        let outcome = outcome?;
        Ok(TurnOutcome {
            stop: to_turn_stop(outcome.stop),
            final_text: last_text(agent),
        })
    }

    fn cancel(&self) {
        // Never awaits: a turn holds the lock for its whole duration, so waiting
        // here would deliver the cancel only after the turn it was meant to
        // stop.
        self.cancel_flag
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }

    async fn compact(&self) -> anyhow::Result<()> {
        let mut guard = self.agent.lock().await;
        let agent = guard
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("the agent is not installed yet"))?;
        agent.compact_history(None).await
    }
}

/// The user message a request becomes.
fn turn_message(request: &TurnRequest) -> Message {
    if request.images.is_empty() {
        return Message::new_text(tact_llm::Role::User, request.input.clone());
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
    Message::new_blocks(tact_llm::Role::User, blocks)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tact_kernel::tool::result::YieldReason;

    /// Every loop stop has a host-facing counterpart.
    ///
    /// The mapping is exhaustive on purpose: a new `AgentLoopStop` must decide
    /// what a host sees, rather than falling into a catch-all.
    #[test]
    fn every_loop_stop_maps() {
        assert_eq!(to_turn_stop(AgentLoopStop::Completed), TurnStop::Completed);
        assert_eq!(
            to_turn_stop(AgentLoopStop::GoalYield(YieldReason::NeedsUser)),
            TurnStop::Yielded(YieldReason::NeedsUser)
        );
        assert_eq!(to_turn_stop(AgentLoopStop::MaxTurns), TurnStop::MaxTurns);
        assert_eq!(to_turn_stop(AgentLoopStop::MaxTokens), TurnStop::MaxTokens);
        assert_eq!(to_turn_stop(AgentLoopStop::Cancelled), TurnStop::Cancelled);
        assert_eq!(
            to_turn_stop(AgentLoopStop::BlockedByHook),
            TurnStop::BlockedByHook
        );
    }

    /// A text-only request becomes one user message with that text.
    #[test]
    fn a_text_request_becomes_a_user_message() {
        let message = turn_message(&TurnRequest::text("hello"));
        assert_eq!(message.role, tact_llm::Role::User);
        match &message.content {
            MessageContent::Text { content } => assert_eq!(content, "hello"),
            other => panic!("expected text, got {other:?}"),
        }
    }

    /// An image request carries the text and the image as blocks.
    ///
    /// Dropping either would silently lose what the user sent.
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
        let MessageContent::Blocks { content } = &message.content else {
            panic!("expected blocks");
        };
        assert_eq!(content.len(), 2, "text and image: {content:?}");
    }

    /// An image-only request has no empty text block in front of it.
    ///
    /// An empty text block makes some providers reject the message.
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
        let MessageContent::Blocks { content } = &message.content else {
            panic!("expected blocks");
        };
        assert_eq!(content.len(), 1, "the image alone: {content:?}");
    }
}
