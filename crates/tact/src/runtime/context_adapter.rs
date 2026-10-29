//! Product context policy over Tact's prompts, hooks, and queued context.

use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use super::{
    activation_view::{ModelIdentity, SessionIdentity, TactActivationView},
    compaction_adapter::Compactor,
    prompt_adapter::{SystemPromptAssembler, estimate_agent_context_tokens},
    services::TactSharedServices,
};
use crate::{
    compact::should_auto_compact,
    hook::{
        Hook, HookContextChunk, HookControl, SessionStartContext, SessionStartFn,
        SessionStartSource, UserPromptSubmitFn, frame_hook_context,
    },
    subagent::SubagentResult,
};
use async_trait::async_trait;
use tact_agent_api::{
    AgentMessage, AgentRole, CancelToken, CompactionOutcome, CompactionRequest, ContextError,
    ContextPolicy, ContextPreparation, ContextView, MessageOrigin, ModelCallOptions, ModelMessage,
    ModelPort, PreparationDecision, PreparedRequest, RequestPlan, RunInput, RunPhase, RunStop,
};

type SessionStartHooks = Vec<Arc<dyn SessionStartFn>>;
type UserPromptSubmitHooks = Vec<Arc<dyn UserPromptSubmitFn>>;

fn hooks_from(hooks: &[Hook]) -> (SessionStartHooks, UserPromptSubmitHooks) {
    let mut session_start = Vec::new();
    let mut user_prompt = Vec::new();
    for hook in hooks {
        match hook {
            Hook::SessionStart(run) => session_start.push(run.clone()),
            Hook::UserPromptSubmit(run) => user_prompt.push(run.clone()),
            _ => {}
        }
    }
    (session_start, user_prompt)
}

/// Context policy for a kernel activation.
///
/// It owns prompt assembly, hook execution, queue draining, and compaction
/// policy. The kernel remains the sole writer of every message, cursor, usage
/// row, and compaction replacement.
pub(crate) struct TactContextPolicy {
    services: Arc<TactSharedServices>,
    session_id: String,
    prompt: Arc<SystemPromptAssembler>,
    compactor: Arc<dyn Compactor>,
    model: ModelIdentity,
    initial_session_context: Mutex<VecDeque<HookContextChunk>>,
    session_start_hooks: Vec<Arc<dyn SessionStartFn>>,
    user_prompt_hooks: Vec<Arc<dyn UserPromptSubmitFn>>,
    model_options: ModelCallOptions,
    goal_active: bool,
    prepared_run: Mutex<Option<ContextPreparation>>,
    pending_appended: Mutex<Option<Vec<AgentMessage>>>,
    suppress_auto_once: AtomicBool,
}

impl TactContextPolicy {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        services: Arc<TactSharedServices>,
        session_id: impl Into<String>,
        prompt: Arc<SystemPromptAssembler>,
        compactor: Arc<dyn Compactor>,
        model: ModelIdentity,
        initial_session_context: Vec<HookContextChunk>,
        hooks: &[Hook],
        model_options: ModelCallOptions,
        goal_active: bool,
    ) -> Self {
        let (session_start_hooks, user_prompt_hooks) = hooks_from(hooks);
        Self {
            services,
            session_id: session_id.into(),
            prompt,
            compactor,
            model,
            initial_session_context: Mutex::new(initial_session_context.into()),
            session_start_hooks,
            user_prompt_hooks,
            model_options,
            goal_active,
            prepared_run: Mutex::new(None),
            pending_appended: Mutex::new(None),
            suppress_auto_once: AtomicBool::new(false),
        }
    }

    pub(crate) fn session_start_pending(&self) -> bool {
        self.compactor.session_start_pending()
    }

    pub(crate) fn session_start_source(&self) -> SessionStartSource {
        self.compactor.session_start_source()
    }

    pub(crate) fn pending_session_context(&self) -> Vec<HookContextChunk> {
        self.initial_session_context
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .cloned()
            .collect()
    }

    fn activation_view<'a>(&'a self, state: &'a ContextView<'_>) -> TactActivationView<'a> {
        TactActivationView::new_with_agent_messages(
            self.services.as_ref(),
            Some(state.transcript),
            SessionIdentity {
                id: Some(state.session.as_str().to_string()),
            },
            self.model.clone(),
            state.total_turns,
            state.remaining_turns,
            self.compactor.session_start_source(),
        )
    }

    async fn run_session_start(
        &self,
        state: &ContextView<'_>,
        cancel: &CancelToken,
    ) -> Result<Result<Vec<HookContextChunk>, RunStop>, ContextError> {
        if !self.compactor.take_session_start_pending() {
            return Ok(Ok(Vec::new()));
        }
        let view = self.activation_view(state);
        let runs = self.session_start_hooks.iter().map(|run| {
            let view = &view;
            async move {
                let mut context = SessionStartContext::default();
                let control = run(view, &mut context)
                    .await
                    .map_err(|error| ContextError::Preparation(error.to_string()))?;
                Ok::<_, ContextError>((control, context.additional_contexts))
            }
        });
        let collected = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(ContextError::Cancelled),
            collected = futures_util::future::join_all(runs) => collected,
        };

        let mut contexts = Vec::new();
        let mut blocked = None;
        for result in collected {
            let (control, additional) =
                result.map_err(|error| ContextError::Preparation(error.to_string()))?;
            contexts.extend(additional);
            if let HookControl::Block(reason) = control
                && blocked.is_none()
            {
                blocked = Some(reason);
            }
        }
        match blocked {
            Some(reason) => Ok(Err(RunStop::Blocked {
                phase: RunPhase::PrepareRun,
                reason,
            })),
            None => Ok(Ok(contexts)),
        }
    }

    async fn apply_user_prompt_hooks(
        &self,
        state: &ContextView<'_>,
        inputs: &mut [AgentMessage],
        cancel: &CancelToken,
    ) -> Result<Result<(), RunStop>, ContextError> {
        let Some(message) = inputs
            .iter_mut()
            .rev()
            .find(|message| message.role == AgentRole::User)
        else {
            return Ok(Ok(()));
        };
        let Some(text) = first_text_target(message) else {
            return Ok(Ok(()));
        };
        for hook in &self.user_prompt_hooks {
            let view = self.activation_view(state);
            let control = tokio::select! {
                biased;
                _ = cancel.cancelled() => return Err(ContextError::Cancelled),
                control = hook(&view, text) => {
                    control.map_err(|error| ContextError::Preparation(error.to_string()))?
                }
            };
            match control {
                HookControl::Continue | HookControl::Allow => {}
                HookControl::Block(reason) => {
                    return Ok(Err(RunStop::Blocked {
                        phase: RunPhase::PrepareRun,
                        reason,
                    }));
                }
            }
        }
        Ok(Ok(()))
    }

    fn queued_messages(&self) -> Vec<AgentMessage> {
        let subagents: Vec<SubagentResult> = {
            let mut queue = self
                .services
                .pending_subagent_results
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            queue.drain(..).collect()
        };
        let hook_context: Vec<HookContextChunk> = {
            let mut queue = self
                .services
                .pending_hook_context
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            queue.drain(..).collect()
        };

        subagents
            .into_iter()
            .map(|result| {
                AgentMessage::text(
                    AgentRole::User,
                    MessageOrigin::SubagentResult,
                    format!(
                        "<subagent-finished id=\"{}\" success=\"{}\">\n{}\n</subagent-finished>",
                        result.child_id, result.success, result.summary
                    ),
                )
            })
            .chain(
                hook_context
                    .into_iter()
                    .map(|chunk| self.hook_message(chunk)),
            )
            .collect()
    }

    fn hook_message(&self, chunk: HookContextChunk) -> AgentMessage {
        self.services
            .emit_update(tact_protocol::AgentUpdate::HookContext {
                source: chunk.source.clone(),
                text: chunk.text.clone(),
            });
        let body = spill_hook_context(&chunk.text, &self.session_id);
        let source = chunk.source.unwrap_or_else(|| "hook".to_string());
        AgentMessage::text(
            AgentRole::User,
            MessageOrigin::InjectedContext {
                source: source.clone(),
            },
            frame_hook_context(Some(&source), &body),
        )
    }

    fn request_plan(
        &self,
        state: &ContextView<'_>,
        appended: Vec<AgentMessage>,
    ) -> Result<RequestPlan, ContextError> {
        let mut messages = state
            .transcript
            .iter()
            .chain(&appended)
            .map(|message| ModelMessage {
                role: message.role,
                content: message.content.clone(),
            })
            .collect::<Vec<_>>();
        if messages.is_empty() && !state.pending_inputs.is_empty() {
            messages.extend(state.pending_inputs.iter().map(|message| ModelMessage {
                role: message.role,
                content: message.content.clone(),
            }));
        }
        let mut system = self
            .prompt
            .build(state)
            .map_err(|error| ContextError::Preparation(error.to_string()))?;
        if self.goal_active && state.remaining_turns == Some(1) {
            system.push_str("\n\n");
            system.push_str(&crate::agent::goal_final_turn_reminder(state.total_turns));
        }
        Ok(RequestPlan {
            request: PreparedRequest {
                system,
                messages,
                cursor: state.cursor.cloned(),
                options: self.model_options.clone(),
            },
            appended_messages: appended,
        })
    }

    fn auto_compact_due(&self, state: &ContextView<'_>, incoming_tokens: usize) -> bool {
        let last_token_total = state.last_token_total.unwrap_or(0);
        let Some(context_window) = state.capabilities.known_context_window() else {
            return false;
        };
        if state.capabilities.supports_native_compaction {
            if last_token_total == 0 {
                return false;
            }
            let threshold = context_window.saturating_mul(80).div_ceil(100);
            let projected = (last_token_total as usize)
                .saturating_add(incoming_tokens)
                .saturating_add(state.capabilities.max_output_tokens as usize);
            return projected >= threshold;
        }
        should_auto_compact(
            last_token_total,
            context_window,
            estimate_agent_context_tokens(state.transcript),
            incoming_tokens,
            state.capabilities.max_output_tokens as usize,
        )
    }

    fn micro_compact_due(&self, state: &ContextView<'_>) -> bool {
        if state.capabilities.supports_native_compaction
            || !self.compactor.micro_compaction_enabled()
        {
            return false;
        }
        let positions = state
            .transcript
            .iter()
            .flat_map(|message| &message.content)
            .filter(|block| matches!(block, tact_agent_api::AgentContentBlock::ToolResult { .. }))
            .collect::<Vec<_>>();
        positions.len() > 12
            && positions[..positions.len() - 12].iter().any(|block| {
                matches!(
                    block,
                    tact_agent_api::AgentContentBlock::ToolResult { content, .. }
                        if content.chars().count() > 120
                )
            })
    }
}

#[async_trait]
impl ContextPolicy for TactContextPolicy {
    async fn prepare_run(
        &self,
        input: &RunInput,
        state: &ContextView<'_>,
        _model: &dyn ModelPort,
        cancel: CancelToken,
    ) -> Result<PreparationDecision<ContextPreparation>, ContextError> {
        if let Some(prepared) = self
            .prepared_run
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            return Ok(PreparationDecision::Continue(prepared));
        }
        let session_start_context = match self.run_session_start(state, &cancel).await? {
            Ok(contexts) => contexts,
            Err(stop) => return Ok(PreparationDecision::Block(stop)),
        };
        let mut session_context: Vec<HookContextChunk> = {
            let mut queued = self
                .initial_session_context
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            queued.drain(..).collect()
        };
        session_context.extend(session_start_context);
        let session_messages = session_context
            .into_iter()
            .map(|chunk| self.hook_message(chunk))
            .collect::<Vec<_>>();

        let mut inputs = input.inputs.clone();
        match self
            .apply_user_prompt_hooks(state, &mut inputs, &cancel)
            .await?
        {
            Ok(()) => {}
            Err(stop) => return Ok(PreparationDecision::Block(stop)),
        }

        let mut prepared = session_messages;
        prepared.extend(inputs);
        let preparation = ContextPreparation {
            inputs: prepared,
            appended_messages: Vec::new(),
        };
        let incoming_tokens = preparation
            .inputs
            .iter()
            .map(estimate_agent_message_tokens)
            .fold(0usize, usize::saturating_add);
        if self.auto_compact_due(state, incoming_tokens) {
            *self
                .prepared_run
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(preparation.clone());
            return Ok(PreparationDecision::Compact {
                request: CompactionRequest::Auto { focus: None },
                if_skipped: preparation,
            });
        }
        Ok(PreparationDecision::Continue(preparation))
    }

    async fn prepare_request(
        &self,
        state: &ContextView<'_>,
        _model: &dyn ModelPort,
        cancel: CancelToken,
    ) -> Result<PreparationDecision<RequestPlan>, ContextError> {
        if cancel.is_cancelled() {
            return Ok(PreparationDecision::Block(RunStop::Cancelled));
        }
        let suppress_auto = self.suppress_auto_once.swap(false, Ordering::AcqRel);
        let pending_appended = self
            .pending_appended
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if !suppress_auto {
            let request = if self.micro_compact_due(state) {
                Some(CompactionRequest::Micro)
            } else if self.auto_compact_due(state, 0) {
                Some(CompactionRequest::Auto { focus: None })
            } else {
                None
            };
            if let Some(request) = request {
                let appended = pending_appended.unwrap_or_else(|| {
                    let appended = self.queued_messages();
                    *self
                        .pending_appended
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) =
                        Some(appended.clone());
                    appended
                });
                let fallback = self.request_plan(state, appended)?;
                return Ok(PreparationDecision::Compact {
                    request,
                    if_skipped: fallback,
                });
            }
        }
        let appended = self
            .pending_appended
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
            .unwrap_or_else(|| self.queued_messages());
        Ok(PreparationDecision::Continue(
            self.request_plan(state, appended)?,
        ))
    }

    async fn compact(
        &self,
        state: &ContextView<'_>,
        request: CompactionRequest,
        model: &dyn ModelPort,
        cancel: CancelToken,
    ) -> Result<CompactionOutcome, ContextError> {
        let kind = request.kind();
        let outcome = self
            .compactor
            .compact(state, request, model, cancel)
            .await?;
        match &outcome {
            CompactionOutcome::Replaced(_) => {
                if kind != tact_agent_api::CompactionKind::Micro {
                    self.suppress_auto_once.store(true, Ordering::Release);
                }
            }
            CompactionOutcome::Skipped { .. } => {
                self.prepared_run
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .take();
                self.pending_appended
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .take();
            }
        }
        Ok(outcome)
    }

    async fn compaction_committed(
        &self,
        state: &ContextView<'_>,
        request: &CompactionRequest,
    ) -> Result<(), ContextError> {
        self.compactor.commit_pending(state, request).await
    }
}

fn estimate_agent_message_tokens(message: &AgentMessage) -> usize {
    serde_json::to_string(message)
        .map(|serialized| crate::compact::approx_text_tokens(&serialized))
        .unwrap_or(usize::MAX / 2)
}

fn first_text_target(message: &mut AgentMessage) -> Option<&mut String> {
    message.content.iter_mut().find_map(|block| match block {
        tact_agent_api::AgentContentBlock::Text { text } => Some(text),
        _ => None,
    })
}

/// Keeps one hook-context chunk inside the model-visible budget.
pub(crate) fn spill_hook_context(text: &str, session_id: &str) -> String {
    const HOOK_CONTEXT_TOKEN_LIMIT: usize = 2_500;

    if crate::compact::approx_text_tokens(text) <= HOOK_CONTEXT_TOKEN_LIMIT {
        return text.to_string();
    }
    let path = hook_output_path(session_id);
    let footer = format!("\n\nFull hook output saved to: {}", path.display());
    let budget =
        HOOK_CONTEXT_TOKEN_LIMIT.saturating_sub(crate::compact::approx_text_tokens(&footer));
    let saved = path
        .parent()
        .map(std::fs::create_dir_all)
        .unwrap_or(Ok(()))
        .and_then(|()| std::fs::write(&path, text));
    if let Err(error) = saved {
        tracing::warn!(
            path = %path.display(),
            "failed to save an oversized hook output: {error}"
        );
        return crate::utils::truncate::truncate_middle_with_token_budget(
            text,
            HOOK_CONTEXT_TOKEN_LIMIT,
        )
        .0;
    }
    let (preview, _) = crate::utils::truncate::truncate_middle_with_token_budget(text, budget);
    format!("{preview}{footer}")
}

fn hook_output_path(session_id: &str) -> std::path::PathBuf {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir()
        .join("hook_outputs")
        .join(session_id)
        .join(format!("{}-{stamp}.txt", std::process::id()))
}

#[cfg(test)]
mod tests;
