//! Built-in conversational application extension.
//!
//! Chat owns the **conversational turn**: what the user's turn says, how it is
//! run, and how the turn ends.
//!
//! Before this existed, Chat was a manifest that aliased `runs.start` (the
//! Agent extension serves `chat.start_run` today, unchanged) and the turn logic
//! lived in the TUI host (`tact_ui::driver`): `build_user_message`, the Stop-hook
//! continuation loop, the cancelled-vs-completed split, `task_complete` and the
//! TaskCompleted hooks. The consequence was an asymmetry no one could see from
//! the code: the headless host started its run through `runs.start` directly, so
//! it silently ignored a `Stop` hook that asked for a continuation and never
//! fired a `TaskCompleted` hook. Moving the turn here fixes that: both hosts
//! submit the raw prompt through `chat.submit` and get the same turn semantics.
//!
//! The turn still *executes* through `runs.start`: Chat is the turn's owner, not
//! a second run entry. `run_chat_turn` invokes the Agent extension's capability
//! through the session's serving router, so an invocation of a chat turn stays
//! inspectable, permission-checked and trajectory-recorded exactly as a direct
//! `runs.start` call is.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use tact::{
    CapabilityHandler, CapabilityRegistration, InvocationContext, KernelError, RuntimeContext,
};
use tact_protocol::{
    CapabilityDeclaration, CapabilityKind, CapabilityRisk, ErrorCategory, PluginId,
    ProtocolVersion, RunId,
};
use tact_view::AgentErrorKind;
use tokio::sync::Mutex;

use crate::{Agent, extract_text, hook::HookControl, plugin::RuntimePluginManifest, runtime_event};

use super::chat_input::build_user_message;

/// How a chat turn reaches the Agent for one iteration.
///
/// The turn is the same in all three cases — the same per-turn reset, the same
/// Stop-hook continuation loop, the same cancelled-vs-completed classification,
/// the same `task_complete` and TaskCompleted hooks. Only "start one run and say
/// how it ended" differs, so naming that difference as a value is what keeps one
/// implementation instead of three.
///
/// - [`TurnEntry::Routed`] — the shipping path: the shared Agent plus the
///   session's serving context, so the run goes through `runs.start` on the
///   router (and is therefore permission-checked, evented and recorded).
/// - [`TurnEntry::Shared`] — the same shared handle with no serving context
///   (the integration tests, a degraded setup): the run is started on the Agent
///   directly, which is the only difference.
/// - [`TurnEntry::Exclusive`] — an Agent a direct caller still owns exclusively
///   (`tact_ui::driver::handle_user_command`), which has no shared handle to
///   give a routed invocation. Same turn, `agent_loop` directly.
pub enum TurnEntry<'a> {
    Routed {
        agent: &'a Arc<Mutex<Agent>>,
        serving: &'a RuntimeContext,
        /// The caller's invocation, reused for the `runs.start` call so the run
        /// carries the same request identity, plugin and actor the host's
        /// submit did — and so cancelling the submit cancels the run.
        invocation: &'a InvocationContext,
    },
    Shared(&'a Arc<Mutex<Agent>>),
    Exclusive(&'a mut Agent),
}

/// How one iteration's run ended, from the turn's point of view.
enum TurnOutcome {
    /// The run returned and was not cancelled.
    Completed,
    /// The run returned `Err`; the error is what the host shows and returns.
    Failed(KernelError),
}

/// What the turn does after one iteration.
enum TurnStep {
    /// A Stop hook asked for one more turn, with this prompt.
    Continue(tact_llm::Message),
    /// The turn is over.
    Stop,
    /// The run failed; the turn ends and the error travels back to the caller.
    Failed(KernelError),
}

/// Runs one conversational turn: the message the user's prompt says, the run,
/// the Stop-hook continuation loop and the turn's end.
///
/// This is the turn, once, for every entry point: the `chat.submit` capability
/// handler and the hosts' direct (no serving context) path both call it, so a
/// degraded setup cannot drift from the routed one.
///
/// The Agent lock, and why the turn is still one critical section:
/// `TurnEntry::Exclusive` holds its Agent outright. The shared variants take the
/// lock for the per-turn reset and for every post-run bookkeeping step, so no
/// other turn can reset the counter or the cancel flag *inside* this turn. One
/// region is the exception, and it has to be: the `runs.start` invocation, which
/// the Agent extension's handler answers by locking this same Agent for the
/// duration of the run (`InProcessAgentExecutor::run`) — and
/// `tokio::sync::Mutex` is not reentrant, so holding the guard across that call
/// deadlocks rather than protecting anything. A caller that needs "one chat turn
/// at a time" therefore gets it by owning the Agent for the turn (which is what
/// both hosts do: the driver awaits its task's `JoinHandle`, headless submits
/// once), not by nesting the lock.
pub async fn run_chat_turn(
    mut entry: TurnEntry<'_>,
    prompt: &str,
    requested_run_id: Option<RunId>,
) -> Result<(), KernelError> {
    // Per-turn reset: the tool-use counter, the cooperative cancel flag cleared
    // for the new turn, and the run identity the caller asked for. The flag is
    // cloned out here so the loop can tell a cancelled run from a completed one
    // without taking the lock.
    let cancel_flag = match &mut entry {
        TurnEntry::Routed { agent, .. } | TurnEntry::Shared(agent) => {
            let mut guard = agent.lock().await;
            reset_turn(&mut guard, requested_run_id)
        }
        TurnEntry::Exclusive(agent) => reset_turn(agent, requested_run_id),
    };

    // The turn's message: the raw prompt, parsed for `@` file / `![]` image
    // references. The builder's work-dir parameter is unused today (references
    // are de-inlined to path text, not resolved against the workspace), so an
    // empty path is passed here rather than threading a host's workspace
    // through the capability boundary.
    let message = build_user_message(prompt, Path::new("")).await;

    // DeepSeek V4 and other text-only models reject `image_url` parts. Reject
    // images early rather than sending a broken request to the API.
    if message.has_images() && !crate::config::supports_vision() {
        let model = tact_llm::get_provider().model;
        let event = runtime_event::error(AgentErrorKind::Other(format!(
            "Image attachments are not supported by {model}. \
             The current model does not accept image input."
        )));
        match &mut entry {
            TurnEntry::Routed { agent, .. } | TurnEntry::Shared(agent) => {
                agent.lock().await.emit_update(event);
            }
            TurnEntry::Exclusive(agent) => agent.emit_update(event),
        }
        return Ok(());
    }

    let mut pending = Some(message);
    let mut stop_continuations = 0u32;
    loop {
        let message = pending.take().expect("a user message is pending");
        let outcome = match &mut entry {
            TurnEntry::Routed {
                serving,
                invocation,
                ..
            } => {
                // The built turn travels as full `content`, so image/file blocks
                // survive the boundary. `runs.start` is the per-turn executor.
                let input = json!({ "content": message.content });
                match serving
                    .router()
                    .invoke("runs.start", (*invocation).clone(), input)
                    .await
                {
                    Ok(_) => TurnOutcome::Completed,
                    Err(error) => TurnOutcome::Failed(error),
                }
            }
            TurnEntry::Shared(agent) => {
                let mut guard = agent.lock().await;
                run_on_agent(&mut guard, message).await
            }
            TurnEntry::Exclusive(agent) => run_on_agent(agent, message).await,
        };

        let step = match &mut entry {
            TurnEntry::Routed { agent, .. } | TurnEntry::Shared(agent) => {
                let mut guard = agent.lock().await;
                bookkeep(&mut guard, outcome, &cancel_flag, &mut stop_continuations).await
            }
            TurnEntry::Exclusive(agent) => {
                bookkeep(agent, outcome, &cancel_flag, &mut stop_continuations).await
            }
        };

        match step {
            TurnStep::Continue(next) => pending = Some(next),
            TurnStep::Stop => break,
            TurnStep::Failed(error) => return Err(error),
        }
    }
    Ok(())
}

/// The per-turn reset, under the caller's lock.
fn reset_turn(agent: &mut Agent, requested_run_id: Option<RunId>) -> Arc<AtomicBool> {
    agent.tool_use_counter = 0;
    agent.runtime.cancel_flag.store(false, Ordering::Relaxed);
    agent.runtime.next_run_id = requested_run_id;
    agent.runtime.cancel_flag.clone()
}

/// One run on an Agent the caller already owns.
///
/// The error is wrapped exactly as the Agent extension's own run handler wraps
/// it (`InternalError` / `"agent"` / not retryable) so a turn that could not use
/// the router reports the identical error a routed one would have.
async fn run_on_agent(agent: &mut Agent, message: tact_llm::Message) -> TurnOutcome {
    match agent.agent_loop(Some(message)).await {
        Ok(()) => TurnOutcome::Completed,
        Err(error) => TurnOutcome::Failed(KernelError::new(
            ErrorCategory::InternalError,
            error.to_string(),
            "agent",
            false,
        )),
    }
}

/// The post-run bookkeeping of one iteration: the cancelled-vs-completed split,
/// the Stop-hook continuation, `task_complete` and the TaskCompleted hooks.
async fn bookkeep(
    agent: &mut Agent,
    outcome: TurnOutcome,
    cancel_flag: &Arc<AtomicBool>,
    stop_continuations: &mut u32,
) -> TurnStep {
    match outcome {
        TurnOutcome::Completed if !cancel_flag.load(Ordering::Relaxed) => {
            match finish_completed_turn(agent, stop_continuations).await {
                Some(message) => TurnStep::Continue(message),
                None => TurnStep::Stop,
            }
        }
        // Cancelled: clear TUI busy state (Planning/Executing) so queued
        // (pending) messages are flushed rather than waiting on a stale busy
        // state.
        TurnOutcome::Completed => {
            agent.emit_update(runtime_event::task_cancelled());
            TurnStep::Stop
        }
        TurnOutcome::Failed(error) => {
            agent.emit_update(runtime_event::error(AgentErrorKind::Other(
                error.message().to_string(),
            )));
            TurnStep::Failed(error)
        }
    }
}

/// The post-turn bookkeeping a completed turn runs once per iteration.
///
/// Runs the Stop hooks (an `allow` means "nothing to add" like `continue`; a
/// `block` asks for one more turn, bounded by `MAX_STOP_CONTINUATIONS`), then
/// emits `task_complete` with the last message and fires the TaskCompleted
/// hooks once. Returns the continuation message when a Stop hook asked for one.
async fn finish_completed_turn(
    agent: &mut Agent,
    stop_continuations: &mut u32,
) -> Option<tact_llm::Message> {
    // A Stop hook may `block` to request one more turn (Codex
    // continuation-fragment semantics: the block reason becomes the next
    // prompt). Bound the loop so a misbehaving hook cannot spin the agent
    // forever.
    const MAX_STOP_CONTINUATIONS: u32 = 4;
    match agent.dispatch_stop_hooks().await {
        Ok(HookControl::Block(reason)) if *stop_continuations < MAX_STOP_CONTINUATIONS => {
            *stop_continuations += 1;
            agent.emit_update(runtime_event::info(format!(
                "[Stop hook] continuing: {reason}"
            )));
            return Some(tact_llm::Message::new_text(tact_llm::Role::User, reason));
        }
        Ok(HookControl::Block(reason)) => {
            agent.emit_update(runtime_event::info(format!(
                "[Stop hook] continuation limit reached; stopping: {reason}"
            )));
        }
        // An `allow` from a Stop hook means "nothing to add", the same as
        // `continue`.
        Ok(HookControl::Continue | HookControl::Allow) | Err(_) => {}
    }
    if let Some(last) = agent.runtime.context.last() {
        let text = extract_text(&last.content);
        agent.emit_update(runtime_event::task_complete(text));
    }
    // TaskCompleted hooks fire once per completed user task.
    if let Err(error) = agent.dispatch_task_completed_hooks().await {
        agent.emit_update(runtime_event::info(format!(
            "[TaskCompleted hook failed] {error}"
        )));
    }
    None
}

/// The Chat extension over the Kernel capability protocol.
///
/// Chat holds the *shared* Agent — the same `Arc<Mutex<Agent>>` the session's
/// `runs.start` capability and the host's teardown use — because a turn has to
/// run on the one conversation the session owns, not on a copy. It also holds
/// the session's serving [`RuntimeContext`], so a turn can start its run
/// through the router.
#[derive(Clone)]
pub struct ChatExtension {
    agent: Arc<Mutex<Agent>>,
    runtime: RuntimeContext,
}

impl ChatExtension {
    /// Wraps the session's shared Agent and serving context.
    ///
    /// `runtime` is the context a turn's `runs.start` is invoked on; it is the
    /// same session serving context [`Self::register`] registers on in both
    /// hosts. It is stored rather than passed per call because a registered
    /// capability handler is called with only an [`InvocationContext`], which
    /// carries the *request*, not the session's router.
    #[must_use]
    pub fn new(agent: Arc<Mutex<Agent>>, runtime: RuntimeContext) -> Self {
        Self { agent, runtime }
    }

    /// Registers `chat.submit` on the given context.
    ///
    /// `chat.start_run` stays the Agent extension's: it is the run's
    /// *implementation*, served by [`crate::extensions::agent::AgentExtension`],
    /// and is neither removed nor re-pointed here.
    pub fn register(&self, runtime: &RuntimeContext) -> Result<(), KernelError> {
        let handler: Arc<dyn CapabilityHandler> = Arc::new(ChatSubmitHandler {
            agent: Arc::clone(&self.agent),
            runtime: self.runtime.clone(),
        });
        runtime
            .router()
            .register(CapabilityRegistration::new(submit_capability(), handler))
    }
}

/// The `chat.submit` request body.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ChatSubmitInput {
    /// The raw prompt, exactly as the user submitted it. The user message
    /// (`@` file / `![]` image references, text) is assembled by this
    /// extension — a host is not expected to know what a turn says.
    prompt: String,
    /// The run identity the caller's command asked for, when it carried one.
    #[serde(default)]
    run_id: Option<RunId>,
}

struct ChatSubmitHandler {
    agent: Arc<Mutex<Agent>>,
    runtime: RuntimeContext,
}

#[async_trait]
impl CapabilityHandler for ChatSubmitHandler {
    async fn invoke(&self, context: InvocationContext, input: Value) -> Result<Value, KernelError> {
        let request: ChatSubmitInput = serde_json::from_value(input).map_err(|error| {
            KernelError::new(
                ErrorCategory::InvalidRequest,
                format!("invalid chat.submit request: {error}"),
                "chat",
                false,
            )
        })?;
        run_chat_turn(
            TurnEntry::Routed {
                agent: &self.agent,
                serving: &self.runtime,
                invocation: &context,
            },
            &request.prompt,
            request.run_id,
        )
        .await?;
        Ok(json!({ "accepted": true }))
    }
}

pub fn manifest() -> RuntimePluginManifest {
    RuntimePluginManifest {
        id: PluginId::from("tact.chat"),
        version: env!("CARGO_PKG_VERSION").into(),
        protocol: ProtocolVersion::CURRENT,
        capabilities: vec![start_run_capability(), submit_capability()],
        dependencies: Vec::new(),
    }
}

/// `chat.start_run` — the run entry, served today by the Agent extension.
///
/// It is declared here (Chat is the extension whose turn a run belongs to) and
/// registered there (the Agent extension is what runs it), exactly as before.
pub fn start_run_capability() -> CapabilityDeclaration {
    CapabilityDeclaration {
        name: "chat.start_run".into(),
        kind: CapabilityKind::App,
        version: "1".into(),
        description: Some("Start and follow a conversational run".into()),
        input_schema: None,
        output_schema: None,
        risk: CapabilityRisk::Medium,
    }
}

fn submit_capability() -> CapabilityDeclaration {
    CapabilityDeclaration {
        name: "chat.submit".into(),
        kind: CapabilityKind::App,
        version: "1".into(),
        description: Some(
            "Submit one conversational turn: assemble the user message, run it, \
             and drive the Stop-hook continuation loop"
                .into(),
        ),
        input_schema: None,
        output_schema: None,
        risk: CapabilityRisk::Medium,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use tact::{CapabilityRouter, PermissionService, RuntimeServices};
    use tact_llm::{ContentBlock, MockClient, Role, StopReason};
    use tact_protocol::{PluginId, RequestId, RuntimeEvent};

    use super::*;
    use crate::extensions::agent::AgentExtension;
    use crate::permission::{PermissionManager, PermissionMode};
    use crate::tool::test_support::test_context;

    struct AllowAll;

    #[async_trait]
    impl PermissionService for AllowAll {
        async fn check(
            &self,
            _declaration: &CapabilityDeclaration,
            _context: &InvocationContext,
            _input: &Value,
        ) -> Result<(), KernelError> {
            Ok(())
        }
    }

    fn runtime() -> RuntimeContext {
        RuntimeContext::with_services(
            CapabilityRouter::new(),
            RuntimeServices::with_permission(Arc::new(AllowAll)),
        )
    }

    fn test_agent(mock: MockClient) -> Agent {
        crate::config::test_support::install_default();
        Agent::new(
            tact_llm::LlmProvider::Mock(mock),
            test_context("chat-extension"),
            crate::tool::toolset(),
            crate::mcp::MCPToolRouter::new(),
            PermissionManager::try_new(PermissionMode::Auto).expect("permission mode"),
            crate::AgentSystemPrompt::Static("chat test".into()),
        )
    }

    fn text_block(text: &str) -> ContentBlock {
        ContentBlock::Text { text: text.into() }
    }

    /// The Chat extension registers `chat.submit` — and leaves `chat.start_run`
    /// to the Agent extension — on the session's serving router.
    #[test]
    fn chat_extension_registers_its_submit_capability() {
        let runtime = runtime();
        let agent = Arc::new(Mutex::new(test_agent(MockClient::new(vec![]))));
        ChatExtension::new(agent, runtime.clone())
            .register(&runtime)
            .expect("chat.submit registers");
        assert!(runtime.router().describe("chat.submit").is_some());
        assert!(
            manifest()
                .capabilities
                .iter()
                .any(|capability| capability.name == "chat.start_run"),
            "the run entry stays declared by Chat; the Agent extension serves it"
        );
    }

    /// A turn submitted through `chat.submit` runs through `runs.start` on the
    /// router (the per-turn executor) and reports completion.
    #[tokio::test]
    async fn chat_submit_runs_the_turn_through_runs_start() {
        let mock = MockClient::new(vec![(
            vec![text_block("answered")],
            Some(StopReason::EndTurn),
        )]);
        let agent = Arc::new(Mutex::new(test_agent(mock).with_ui_channel({
            let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
            tx
        })));
        let runtime = runtime();
        AgentExtension::from_shared(Arc::clone(&agent), Arc::new(AtomicBool::new(false)))
            .register(&runtime)
            .expect("runs.start registers");
        ChatExtension::new(Arc::clone(&agent), runtime.clone())
            .register(&runtime)
            .expect("chat.submit registers");

        let output = runtime
            .router()
            .invoke(
                "chat.submit",
                runtime.invocation(
                    RequestId::from("chat-submit-test"),
                    PluginId::from("tact.chat"),
                    "test",
                ),
                json!({"prompt": "hello"}),
            )
            .await
            .expect("chat.submit answers");
        assert_eq!(output, json!({"accepted": true}));

        let agent = agent.lock().await;
        let saw_user_turn =
            agent.runtime.context.iter().any(|message| {
                message.role == Role::User && extract_text(&message.content) == "hello"
            });
        assert!(saw_user_turn, "the user turn reached the shared Agent");
    }

    /// A Stop hook that blocks keeps the turn going: the block reason becomes
    /// the next prompt and the model is called again.
    #[tokio::test]
    async fn stop_hook_block_continues_the_chat_turn() {
        let mock = MockClient::new(vec![
            (vec![text_block("first")], Some(StopReason::EndTurn)),
            (vec![text_block("second")], Some(StopReason::EndTurn)),
        ]);
        let calls = Arc::new(AtomicUsize::new(0));
        let hook_calls = Arc::clone(&calls);
        let (events_tx, mut events_rx) = tokio::sync::mpsc::unbounded_channel();
        let agent = test_agent(mock)
            .with_stop(move |_agent| {
                let remaining = hook_calls.fetch_add(1, Ordering::Relaxed);
                Box::pin(async move {
                    if remaining == 0 {
                        Ok(HookControl::Block("keep going".into()))
                    } else {
                        Ok(HookControl::Continue)
                    }
                })
            })
            .with_ui_channel(events_tx);
        let agent = Arc::new(Mutex::new(agent));

        run_chat_turn(TurnEntry::Shared(&agent), "start", None)
            .await
            .expect("the turn completes");

        assert_eq!(calls.load(Ordering::Relaxed), 2, "the Stop hook ran again");
        let mut continuations = 0;
        while let Ok(event) = events_rx.try_recv() {
            if let RuntimeEvent::Info { content, .. } = event
                && content.contains("[Stop hook] continuing: keep going")
            {
                continuations += 1;
            }
        }
        assert_eq!(continuations, 1, "the continuation was announced once");
    }

    /// TaskCompleted hooks fire once for a completed chat turn — the half of
    /// the turn the headless host used to miss.
    #[tokio::test]
    async fn task_completed_hooks_fire_in_the_chat_turn() {
        let mock = MockClient::new(vec![(vec![text_block("done")], Some(StopReason::EndTurn))]);
        let completed = Arc::new(AtomicUsize::new(0));
        let hook_calls = Arc::clone(&completed);
        let agent = Arc::new(Mutex::new(test_agent(mock).with_task_completed(
            move |_agent| {
                hook_calls.fetch_add(1, Ordering::Relaxed);
                Box::pin(async { Ok(HookControl::Continue) })
            },
        )));

        run_chat_turn(TurnEntry::Shared(&agent), "finish", None)
            .await
            .expect("the turn completes");

        assert_eq!(
            completed.load(Ordering::Relaxed),
            1,
            "TaskCompleted fires once per completed turn"
        );
    }
}
