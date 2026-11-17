//! Product-owned compaction behavior for kernel activations.
//!
//! The kernel owns the typed compaction loop, observed model scope, usage
//! records, and atomic transcript/cursor replacement. This adapter owns the
//! product policy: hooks, micro truncation, summary prompts, native Responses
//! compaction, and the replacement transcript it returns.

use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use async_trait::async_trait;
use tact_agent_api::{
    AgentContentBlock, AgentMessage, AgentRole, CancelToken, CompactionKind, CompactionOutcome,
    CompactionRequest, ContextError, ContextReplacement, ContextView, CursorUpdate,
    ModelCallOptions, ModelCursor, ModelMessage, ModelPort, ModelRequest, ModelResponse,
    StopReason,
};
use tact_llm::{
    Message, OpenAiReasoningEffort, ProviderKind,
    neutral::{agent_message, llm_message},
};
use tact_protocol::AgentUpdate;

use super::{
    activation_view::{ModelIdentity, SessionIdentity, TactActivationView},
    host_neutral_token_usage,
    prompt_adapter::SystemPromptAssembler,
    services::TactSharedServices,
};
use crate::{
    ToolSpec,
    agent::{
        COMPACT_SUMMARY_HEADROOM_PERCENT, COMPACT_SUMMARY_INSTRUCTIONS, COMPACT_SUMMARY_MAX_TOKENS,
        COMPACT_SUMMARY_OUTPUT_PERCENT, compact_effort_reserve_tokens, compact_summary_effort,
        compact_summary_envelope_ceiling, compact_summary_server_default_effort,
        effort_after_provider_fold, next_compaction_reserve,
    },
    compact::{
        CompactState, CompactTrigger, approx_text_tokens, build_compacted_history,
        collect_user_messages, compact_rebuild_headroom_tokens, estimate_context_tokens,
        recent_messages_for_summary, retained_user_message_token_budget, write_transcript,
    },
    config::AgentSettings,
    consts::TactPath,
    hook::{Hook, HookControl, PostCompactFn, PreCompactFn, SessionStartSource},
    recovery::{
        FailureKind, MAX_COMPACT_SUMMARY_ATTEMPTS, MAX_COMPACT_SUMMARY_RETRY_ATTEMPTS,
        backoff_delay, classify_model_error, continuation_message, error_summary,
    },
};

/// How many recent tool results survive micro compaction intact; every older
/// result longer than [`MICRO_COMPACT_STUB_CHARS`] is replaced with
/// [`COMPACTED_TOOL_RESULT`].
///
/// These three live in the code that runs the pass, and
/// [`crate::runtime::context_adapter`] reads the same constants for its
/// `micro_compact_due` check. A literal on either side lets "is it due?" and
/// "what does it do?" disagree — the due check then fires a pass that stubs
/// nothing, or stays silent while the transcript keeps growing.
pub(crate) const KEEP_RECENT_TOOL_RESULTS: usize = 12;
/// Character count above which an old tool result is worth stubbing.
///
/// Shorter results stay: they are cheap to carry, and the stub would cost more
/// tokens than the text it replaces.
pub(crate) const MICRO_COMPACT_STUB_CHARS: usize = 120;
pub(crate) const COMPACTED_TOOL_RESULT: &str =
    "[Earlier tool result compacted. Re-run the tool (e.g., read_file) for full content.]";
/// The read-out a native compaction's card shows in place of a prose summary.
///
/// The native path replaces history provider-side, so there is no local text;
/// what it does report is this count. Kept next to the `[responses compacted:
/// items=…]` log line that carries the same numbers so the two cannot drift.
pub(crate) const NATIVE_COMPACTION_FALLBACK_SUMMARY: &str =
    "Native Responses compaction replaced the conversation provider-side.";
/// The stub has to be shorter than the results it replaces, or micro compaction
/// grows the context it was meant to shrink.
const _: () = assert!(COMPACTED_TOOL_RESULT.len() < MICRO_COMPACT_STUB_CHARS);

type PreCompactHooks = Vec<Arc<dyn PreCompactFn>>;
type PostCompactHooks = Vec<Arc<dyn PostCompactFn>>;

fn hooks_from(hooks: &[Hook]) -> (PreCompactHooks, PostCompactHooks) {
    let mut pre = Vec::new();
    let mut post = Vec::new();
    for hook in hooks {
        match hook {
            Hook::PreCompact(run) => pre.push(run.clone()),
            Hook::PostCompact(run) => post.push(run.clone()),
            _ => {}
        }
    }
    (pre, post)
}

struct PendingCommit {
    summary: Option<String>,
    success_notice: Option<String>,
}

/// Product compaction service injected into the context policy.
pub(crate) struct TactCompactor {
    services: Arc<TactSharedServices>,
    prompt: Arc<SystemPromptAssembler>,
    session_id: String,
    model: ModelIdentity,
    work_dir: PathBuf,
    settings: AgentSettings,
    tool_specs: Vec<ToolSpec>,
    provider_kind: ProviderKind,
    native_compaction: bool,
    session_start_pending: Arc<AtomicBool>,
    session_start_source: Arc<Mutex<SessionStartSource>>,
    compact_state: Mutex<CompactState>,
    pre_compact_hooks: Vec<Arc<dyn PreCompactFn>>,
    post_compact_hooks: Vec<Arc<dyn PostCompactFn>>,
    pending_commit: Mutex<Option<PendingCommit>>,
}

impl TactCompactor {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        services: Arc<TactSharedServices>,
        prompt: Arc<SystemPromptAssembler>,
        session_id: impl Into<String>,
        model: ModelIdentity,
        work_dir: PathBuf,
        settings: AgentSettings,
        tool_specs: Vec<ToolSpec>,
        provider_kind: ProviderKind,
        native_compaction: bool,
        session_start_pending: bool,
        session_start_source: SessionStartSource,
        compact_state: CompactState,
        hooks: &[Hook],
    ) -> Self {
        let (pre_compact_hooks, post_compact_hooks) = hooks_from(hooks);
        Self {
            services,
            prompt,
            session_id: session_id.into(),
            model,
            work_dir,
            settings,
            tool_specs,
            provider_kind,
            native_compaction,
            session_start_pending: Arc::new(AtomicBool::new(session_start_pending)),
            session_start_source: Arc::new(Mutex::new(session_start_source)),
            compact_state: Mutex::new(compact_state),
            pre_compact_hooks,
            post_compact_hooks,
            pending_commit: Mutex::new(None),
        }
    }

    pub(crate) fn session_start_pending(&self) -> bool {
        self.session_start_pending.load(Ordering::Acquire)
    }

    pub(crate) fn take_session_start_pending(&self) -> bool {
        self.session_start_pending.swap(false, Ordering::AcqRel)
    }

    pub(crate) fn session_start_source(&self) -> SessionStartSource {
        *self
            .session_start_source
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(crate) fn compact_state(&self) -> CompactState {
        self.compact_state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub(crate) fn micro_compaction_enabled(&self) -> bool {
        self.settings.micro_compact_enabled
    }

    async fn compact(
        &self,
        state: &ContextView<'_>,
        request: CompactionRequest,
        model: &dyn ModelPort,
        cancel: CancelToken,
    ) -> Result<CompactionOutcome, ContextError> {
        if matches!(request, CompactionRequest::Micro) {
            return self.compact_micro(state);
        }

        let trigger = trigger_for(request.kind());
        if self.run_pre_hooks(state, trigger, &cancel).await? {
            return Ok(CompactionOutcome::Skipped {
                reason: "PreCompact hook vetoed compaction".into(),
            });
        }

        let mut native_readout = None;
        let (transcript, summary, cursor, success_notice) = if self.native_compaction {
            let (cursor, success_notice) = self.compact_native(state, model, &cancel).await?;
            // Native compaction always replaces something, so the host always
            // gets a read-out: the provider's own numbers when its cursor is
            // readable, a plain sentence when it is not (an external or fake
            // `ModelPort` may hand back an opaque cursor that carries no Tact
            // provider state).
            native_readout = Some(
                success_notice
                    .as_deref()
                    .map(native_compaction_readout)
                    .unwrap_or_else(|| NATIVE_COMPACTION_FALLBACK_SUMMARY.to_string()),
            );
            (None, None, cursor, success_notice)
        } else {
            let (transcript, summary) = self
                .compact_local(state, request.focus(), model, &cancel)
                .await?;
            (
                Some(transcript),
                Some(summary),
                CursorUpdate::Replace { cursor: None },
                None,
            )
        };

        // The summary goes two ways: the facade keeps it for `/compact`'s own
        // read-out, and the replacement carries it so a host that renders the
        // compaction (ACP's card) can show what replaced the history.
        //
        // Native compaction has no prose summary — history is replaced
        // provider-side — so its read-out stands in: a card with an empty body
        // reads as "nothing happened", while "items=2" is checkable. It stays
        // out of `summary`/`pending_commit` on purpose: the facade treats that
        // field as local summarization text (`last_summary`), and a count is not
        // prose.
        let summary_for_host = match (summary.clone(), native_readout) {
            (Some(summary), _) => Some(summary),
            (None, Some(readout)) => Some(readout),
            (None, None) => None,
        };
        *self
            .pending_commit
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(PendingCommit {
            summary,
            success_notice,
        });
        Ok(CompactionOutcome::Replaced(ContextReplacement {
            transcript,
            cursor,
            summary: summary_for_host,
        }))
    }

    async fn commit_pending(
        &self,
        state: &ContextView<'_>,
        request: &CompactionRequest,
    ) -> Result<(), ContextError> {
        if matches!(request, CompactionRequest::Micro) {
            return Ok(());
        }
        let Some(pending) = self
            .pending_commit
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        else {
            return Ok(());
        };
        self.finish_success(pending.summary);
        let trigger = trigger_for(request.kind());
        let view = self.activation_view(state);
        for hook in &self.post_compact_hooks {
            let control = hook(&view, trigger)
                .await
                .map_err(|error| ContextError::Compaction(error.to_string()))?;
            if let HookControl::Block(reason) = control {
                self.services.emit_update(AgentUpdate::Info(format!(
                    "[PostCompact hook blocked] {reason}"
                )));
            }
        }
        if let Some(notice) = pending.success_notice {
            self.services.emit_update(AgentUpdate::Info(notice));
        }
        Ok(())
    }

    async fn run_pre_hooks(
        &self,
        state: &ContextView<'_>,
        trigger: CompactTrigger,
        cancel: &CancelToken,
    ) -> Result<bool, ContextError> {
        let view = self.activation_view(state);
        for hook in &self.pre_compact_hooks {
            let control = tokio::select! {
                biased;
                _ = cancel.cancelled() => return Err(ContextError::Cancelled),
                control = hook(&view, trigger) => {
                    control.map_err(|error| ContextError::Compaction(error.to_string()))?
                }
            };
            if let HookControl::Block(reason) = control {
                self.services.emit_update(AgentUpdate::Info(format!(
                    "[PreCompact hook vetoed compaction] {reason}"
                )));
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn activation_view<'a>(&'a self, state: &'a ContextView<'_>) -> TactActivationView<'a> {
        TactActivationView::new_with_agent_messages(
            self.services.as_ref(),
            Some(state.transcript),
            SessionIdentity {
                id: Some(self.session_id.clone()),
            },
            self.model.clone(),
            state.total_turns,
            state.remaining_turns,
            self.session_start_source(),
        )
    }

    fn compact_micro(&self, state: &ContextView<'_>) -> Result<CompactionOutcome, ContextError> {
        if !self.settings.micro_compact_enabled {
            return Ok(CompactionOutcome::Skipped {
                reason: "micro compaction disabled".into(),
            });
        }
        let mut messages = state.transcript.to_vec();
        let positions: Vec<(usize, usize)> = messages
            .iter()
            .enumerate()
            .flat_map(|(message_index, message)| {
                message
                    .content
                    .iter()
                    .enumerate()
                    .filter_map(move |(block_index, block)| {
                        matches!(block, AgentContentBlock::ToolResult { .. })
                            .then_some((message_index, block_index))
                    })
            })
            .collect();
        if positions.len() <= KEEP_RECENT_TOOL_RESULTS {
            return Ok(CompactionOutcome::Skipped {
                reason: "no old tool results to compact".into(),
            });
        }

        let compact_until = positions.len().saturating_sub(KEEP_RECENT_TOOL_RESULTS);
        let mut changed = false;
        for (message_index, block_index) in positions.into_iter().take(compact_until) {
            let Some(message) = messages.get_mut(message_index) else {
                continue;
            };
            let Some(AgentContentBlock::ToolResult { content, .. }) =
                message.content.get_mut(block_index)
            else {
                continue;
            };
            if content.chars().count() > MICRO_COMPACT_STUB_CHARS {
                *content = COMPACTED_TOOL_RESULT.to_string();
                changed = true;
            }
        }
        if !changed {
            return Ok(CompactionOutcome::Skipped {
                reason: "no old tool result exceeded the compaction threshold".into(),
            });
        }
        // Micro compaction stubs old tool results in place; there is no
        // prose summary to show, and inventing one would misdescribe it as a
        // summarization pass.
        Ok(CompactionOutcome::Replaced(ContextReplacement {
            transcript: Some(messages),
            cursor: CursorUpdate::Unchanged,
            summary: None,
        }))
    }

    async fn compact_native(
        &self,
        state: &ContextView<'_>,
        model: &dyn ModelPort,
        cancel: &CancelToken,
    ) -> Result<(CursorUpdate, Option<String>), ContextError> {
        self.services
            .emit_update(AgentUpdate::Info("[native compact]".into()));
        let request = summary_request(
            self.model.name.clone(),
            state.transcript.iter().map(ModelMessage::from).collect(),
            self.settings
                .max_tokens
                .min(model.capabilities().max_output_tokens),
            ModelCallOptions::default(),
        );
        let response = self
            .native_with_retry(model, request, state.cursor, cancel)
            .await?;
        match &response.cursor {
            CursorUpdate::Replace {
                cursor: Some(cursor),
            } => {
                // The product adapter can only describe a Tact-owned cursor.
                // Fake/external ModelPorts may return an opaque cursor; that
                // must still satisfy the replacement contract, just without a
                // product-specific success notice.
                let notice =
                    tact_llm::neutral::provider_state(cursor)
                        .ok()
                        .map(|state| match state {
                            tact_llm::ProviderConversationState::OpenAiResponses(state) => {
                                let item_count = state.input_items.len();
                                let compaction_id = state.compaction_id.unwrap_or_default();
                                format!(
                                    "[responses compacted: items={item_count}, id={}]",
                                    compact_id_display(&compaction_id)
                                )
                            }
                        });
                Ok((response.cursor, notice))
            }
            CursorUpdate::Replace { cursor: None } => Err(ContextError::Compaction(
                "native Responses compaction returned no replacement cursor".into(),
            )),
            CursorUpdate::Unchanged => Err(ContextError::Compaction(
                "native Responses compaction returned no replacement cursor".into(),
            )),
        }
    }

    async fn compact_local(
        &self,
        state: &ContextView<'_>,
        focus: Option<&str>,
        model: &dyn ModelPort,
        cancel: &CancelToken,
    ) -> Result<(Vec<AgentMessage>, String), ContextError> {
        let legacy_transcript = state
            .transcript
            .iter()
            .map(llm_message)
            .collect::<Vec<Message>>();
        let transcript_path = write_transcript(&TactPath::new(&self.work_dir), &legacy_transcript)
            .await
            .map_err(compaction_error)?;
        self.services.emit_update(AgentUpdate::Info(format!(
            "[transcript saved: {}]",
            transcript_path.display()
        )));

        let model_context_window = self.settings.model_context_window;
        let session_effort = self.settings.reasoning_effort;
        let model_max_output = model.capabilities().max_output_tokens;
        let summary_text_max_tokens = if model_context_window == 0 {
            COMPACT_SUMMARY_MAX_TOKENS
        } else {
            u32::try_from(
                model_context_window
                    .saturating_mul(COMPACT_SUMMARY_OUTPUT_PERCENT)
                    .div_ceil(100)
                    .min(COMPACT_SUMMARY_MAX_TOKENS as usize)
                    .max(1),
            )
            .map_err(compaction_error)?
        };
        // A model whose output cap is below the computed text budget cannot
        // serve the request; the cap is the hard ceiling, not the window.
        let summary_text_max_tokens = summary_text_max_tokens.min(model_max_output);
        let effective_effort =
            session_effort.or_else(|| compact_summary_server_default_effort(&self.provider_kind));
        let reasoning_reserve = effective_effort
            .map(|effort| {
                compact_effort_reserve_tokens(effort_after_provider_fold(
                    &self.provider_kind,
                    effort,
                ))
            })
            .unwrap_or(0)
            .min(model_max_output.saturating_sub(summary_text_max_tokens));
        let envelope_floor = if effective_effort.is_some() {
            self.settings
                .max_tokens
                .min(compact_summary_envelope_ceiling(model_context_window))
                .min(model_max_output)
        } else {
            0
        };
        let summary_max_tokens = summary_text_max_tokens
            .saturating_add(reasoning_reserve)
            .max(envelope_floor)
            .min(model_max_output);
        let summary_input_limit = if model_context_window == 0 {
            crate::compact::KEEP_USER_MESSAGE_TOKENS
        } else {
            let headroom = model_context_window
                .saturating_mul(COMPACT_SUMMARY_HEADROOM_PERCENT)
                .div_ceil(100);
            model_context_window
                .saturating_sub(summary_max_tokens as usize)
                .saturating_sub(headroom)
        };

        let mut prompt = COMPACT_SUMMARY_INSTRUCTIONS.to_string();
        if approx_text_tokens(&prompt) > summary_input_limit {
            return Err(ContextError::Compaction(format!(
                "model context window {model_context_window} is too small for the compaction summary request"
            )));
        }
        if let Some(focus) = focus.filter(|value| !value.trim().is_empty()) {
            let addition = format!("\n\nFocus to preserve next: {focus}");
            if approx_text_tokens(&prompt).saturating_add(approx_text_tokens(&addition))
                <= summary_input_limit
            {
                prompt.push_str(&addition);
            }
        }
        let recent_files = self
            .compact_state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .recent_files
            .clone();
        let mut heading_added = false;
        for path in &recent_files {
            let addition = if heading_added {
                format!("\n- {path}")
            } else {
                format!("\n\nRecent files to reopen if needed:\n- {path}")
            };
            if approx_text_tokens(&prompt).saturating_add(approx_text_tokens(&addition))
                > summary_input_limit
            {
                break;
            }
            prompt.push_str(&addition);
            heading_added = true;
        }
        let history_budget = summary_input_limit
            .saturating_sub(approx_text_tokens(&prompt))
            .saturating_sub(1)
            .min(crate::compact::KEEP_USER_MESSAGE_TOKENS);
        let recent_messages = recent_messages_for_summary(&legacy_transcript, history_budget)
            .map_err(compaction_error)?;
        if recent_messages != "[]" {
            prompt.push_str("\n\n");
            prompt.push_str(&recent_messages);
        }

        let max_reserve_for_window = if model_context_window == 0 {
            u32::MAX
        } else {
            let headroom = model_context_window
                .saturating_mul(COMPACT_SUMMARY_HEADROOM_PERCENT)
                .div_ceil(100);
            u32::try_from(
                model_context_window
                    .saturating_sub(approx_text_tokens(&prompt))
                    .saturating_sub(headroom)
                    .saturating_sub(summary_text_max_tokens as usize),
            )
            .unwrap_or(u32::MAX)
        };

        let mut continuation_attempt = 0u32;
        let mut attempt_max_tokens = summary_max_tokens;
        let mut attempt_reserve = reasoning_reserve;
        let mut messages = vec![ModelMessage {
            role: AgentRole::User,
            content: vec![AgentContentBlock::Text { text: prompt }],
        }];
        let mut summary_parts = Vec::new();
        loop {
            let stage = continuation_attempt.saturating_add(1);
            let total_stages = MAX_COMPACT_SUMMARY_ATTEMPTS.saturating_add(1);
            let attempt_effort =
                compact_summary_effort(&self.provider_kind, session_effort, continuation_attempt);
            let request = summary_request(
                self.model.name.clone(),
                messages.clone(),
                attempt_max_tokens,
                ModelCallOptions {
                    reasoning_effort: attempt_effort.map(Into::into),
                    ..ModelCallOptions::default()
                },
            );
            let request_chars = serde_json::to_string(&request)
                .map(|body| body.chars().count())
                .unwrap_or(0);
            self.services
                .emit_update(AgentUpdate::ModelInfo(tact_protocol::ModelCallParams {
                    model: self.model.name.clone(),
                    max_tokens: attempt_max_tokens,
                    thinking_budget: None,
                    reasoning_effort: attempt_effort.map(Into::into),
                    extra_body: None,
                }));
            self.services.emit_update(AgentUpdate::Info(format!(
                "[compact summary {stage}/{total_stages}] request model={} max_tokens={attempt_max_tokens} (text {summary_text_max_tokens} + reasoning {attempt_reserve}), reasoning_effort={}, input {request_chars} chars",
                self.model.name,
                attempt_effort.map_or("provider-default", OpenAiReasoningEffort::as_str),
            )));
            let response = self
                .complete_with_retry(model, request, None, cancel)
                .await?;
            let host_usage = response.usage.as_ref().map(host_neutral_token_usage);
            self.services.emit_update(AgentUpdate::Info(format!(
                "[compact summary {stage}/{total_stages}] response {}",
                compact_response_note(Some(&response.stop_reason), host_usage.as_ref()),
            )));
            summary_parts.push(response.message.text_content());
            let truncated = response.stop_reason == StopReason::MaxTokens;
            if truncated && continuation_attempt < MAX_COMPACT_SUMMARY_ATTEMPTS {
                let think_block_bytes = response.message.content.iter().fold(0, |total, block| {
                    if let AgentContentBlock::Thinking {
                        thinking,
                        signature,
                    } = block
                    {
                        total + thinking.len() + signature.len()
                    } else {
                        total
                    }
                });
                let observed_think = response
                    .usage
                    .map_or(0usize, |usage| usage.reasoning_tokens as usize);
                continuation_attempt = continuation_attempt.saturating_add(1);
                messages.push(ModelMessage::from(&response.message));
                messages.push(ModelMessage {
                    role: AgentRole::User,
                    content: vec![AgentContentBlock::Text {
                        text: continuation_message(continuation_attempt).to_string(),
                    }],
                });
                attempt_reserve = if continuation_attempt <= 1 {
                    0
                } else {
                    next_compaction_reserve(
                        reasoning_reserve,
                        summary_text_max_tokens,
                        attempt_reserve,
                        observed_think,
                    )
                    .min(max_reserve_for_window)
                    .min(model_max_output.saturating_sub(summary_text_max_tokens))
                };
                attempt_max_tokens = summary_text_max_tokens
                    .saturating_add(attempt_reserve)
                    .min(model_max_output);
                self.services.emit_update(AgentUpdate::Info(format!(
                    "[compact continue {continuation_attempt}/{MAX_COMPACT_SUMMARY_ATTEMPTS}] summary truncated ({}), next attempt max_tokens={attempt_max_tokens}",
                    compact_truncation_note(
                        response.usage.map(|usage| usage.reasoning_tokens),
                        think_block_bytes,
                    ),
                )));
                continue;
            }
            if !matches!(
                response.stop_reason,
                StopReason::EndTurn | StopReason::MaxTokens
            ) {
                return Err(ContextError::Compaction(format!(
                    "compaction summary ended with invalid stop reason: {:?}",
                    response.stop_reason
                )));
            }
            if truncated {
                self.services.emit_update(AgentUpdate::Info(format!(
                    "[compact fallback] summary still truncated at stage {stage}/{total_stages}; using the best-effort partial summary"
                )));
            }
            break;
        }

        let summary = summary_parts.join("\n").trim().to_string();
        if summary.is_empty() {
            return Err(ContextError::Compaction(
                "compaction summary response contained no text".into(),
            ));
        }
        let mut full_summary = summary.clone();
        if !recent_files.is_empty() {
            full_summary
                .push_str("\n\nRecently accessed files (re-read if you need their contents):\n");
            for path in &recent_files {
                full_summary.push_str(&format!("- {path}\n"));
            }
        }

        let system_prompt = self
            .prompt
            .build(state)
            .map_err(|error| ContextError::Compaction(error.to_string()))?;
        let system_prompt_tokens = approx_text_tokens(&system_prompt);
        let tool_specs_tokens = approx_text_tokens(
            &serde_json::to_string(&self.tool_specs)
                .map_err(|error| ContextError::Compaction(error.to_string()))?,
        );
        let summary_only = vec![Message::new_text(
            tact_llm::Role::User,
            full_summary.clone(),
        )];
        let non_retained_input_tokens = system_prompt_tokens
            .saturating_add(tool_specs_tokens)
            .saturating_add(estimate_context_tokens(&summary_only));
        let mut retained_tokens = retained_user_message_token_budget(
            model_context_window,
            self.settings.max_tokens as usize,
            non_retained_input_tokens,
        );
        let retained = collect_user_messages(&legacy_transcript);
        let summary_text = full_summary.clone();
        let mut rebuilt = build_compacted_history(
            &retained,
            full_summary,
            retained_tokens,
            Some(transcript_path.as_path()),
        );
        if model_context_window > 0 {
            let headroom = compact_rebuild_headroom_tokens(model_context_window);
            loop {
                let total = system_prompt_tokens
                    .saturating_add(tool_specs_tokens)
                    .saturating_add(estimate_context_tokens(&rebuilt))
                    .saturating_add(self.settings.max_tokens as usize)
                    .saturating_add(headroom);
                if total <= model_context_window {
                    break;
                }
                if retained_tokens == 0 {
                    return Err(ContextError::Compaction(format!(
                        "compacted request cannot fit model context window {model_context_window}"
                    )));
                }
                retained_tokens = retained_tokens
                    .saturating_sub(total.saturating_sub(model_context_window).max(1));
                rebuilt = build_compacted_history(
                    &retained,
                    summary_text.clone(),
                    retained_tokens,
                    Some(transcript_path.as_path()),
                );
            }
        }

        Ok((rebuilt.iter().map(agent_message).collect(), summary))
    }

    async fn complete_with_retry(
        &self,
        model: &dyn ModelPort,
        request: ModelRequest,
        cursor: Option<&ModelCursor>,
        cancel: &CancelToken,
    ) -> Result<ModelResponse, ContextError> {
        let mut attempt = 0u32;
        loop {
            if cancel.is_cancelled() {
                return Err(ContextError::Cancelled);
            }
            match model
                .complete(request.clone(), cursor, cancel.clone())
                .await
            {
                Ok(response) => return Ok(response),
                Err(error) => {
                    let failure = classify_model_error(&error);
                    if attempt >= MAX_COMPACT_SUMMARY_RETRY_ATTEMPTS
                        || failure != FailureKind::Transient
                    {
                        return Err(summary_failure(&error, failure));
                    }
                    attempt = attempt.saturating_add(1);
                    let delay = backoff_delay(attempt.saturating_sub(1));
                    self.services.emit_update(AgentUpdate::Info(format!(
                        "[compact retry {attempt}/{MAX_COMPACT_SUMMARY_RETRY_ATTEMPTS}] retrying in {:.1}s — {}",
                        delay.as_secs_f64(),
                        error_summary(&error.to_string())
                    )));
                    tokio::select! {
                        _ = cancel.cancelled() => return Err(ContextError::Cancelled),
                        _ = tokio::time::sleep(delay) => {}
                    }
                }
            }
        }
    }

    async fn native_with_retry(
        &self,
        model: &dyn ModelPort,
        request: ModelRequest,
        cursor: Option<&ModelCursor>,
        cancel: &CancelToken,
    ) -> Result<ModelResponse, ContextError> {
        let mut attempt = 0u32;
        loop {
            if cancel.is_cancelled() {
                return Err(ContextError::Cancelled);
            }
            match model.compact(request.clone(), cursor, cancel.clone()).await {
                Ok(response) => return Ok(response),
                Err(error) => {
                    let failure = classify_model_error(&error);
                    if attempt >= MAX_COMPACT_SUMMARY_RETRY_ATTEMPTS
                        || failure != FailureKind::Transient
                    {
                        return Err(summary_failure(&error, failure));
                    }
                    attempt = attempt.saturating_add(1);
                    let delay = backoff_delay(attempt.saturating_sub(1));
                    self.services.emit_update(AgentUpdate::Info(format!(
                        "[compact retry {attempt}/{MAX_COMPACT_SUMMARY_RETRY_ATTEMPTS}] retrying in {:.1}s — {}",
                        delay.as_secs_f64(),
                        error_summary(&error.to_string())
                    )));
                    tokio::select! {
                        _ = cancel.cancelled() => return Err(ContextError::Cancelled),
                        _ = tokio::time::sleep(delay) => {}
                    }
                }
            }
        }
    }

    fn finish_success(&self, summary: Option<String>) {
        let mut compact_state = self
            .compact_state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        compact_state.has_compacted = true;
        if let Some(summary) = summary {
            compact_state.last_summary = Some(summary);
        }
        drop(compact_state);
        {
            let mut stats = self
                .services
                .stats
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            stats.compactions = stats.compactions.saturating_add(1);
        }
        self.session_start_pending.store(true, Ordering::Release);
        *self
            .session_start_source
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = SessionStartSource::Compact;
    }
}

#[async_trait]
impl Compactor for TactCompactor {
    async fn compact(
        &self,
        state: &ContextView<'_>,
        request: CompactionRequest,
        model: &dyn ModelPort,
        cancel: CancelToken,
    ) -> Result<CompactionOutcome, ContextError> {
        TactCompactor::compact(self, state, request, model, cancel).await
    }

    async fn commit_pending(
        &self,
        state: &ContextView<'_>,
        request: &CompactionRequest,
    ) -> Result<(), ContextError> {
        TactCompactor::commit_pending(self, state, request).await
    }

    fn session_start_pending(&self) -> bool {
        TactCompactor::session_start_pending(self)
    }

    fn take_session_start_pending(&self) -> bool {
        TactCompactor::take_session_start_pending(self)
    }

    fn session_start_source(&self) -> SessionStartSource {
        TactCompactor::session_start_source(self)
    }

    fn micro_compaction_enabled(&self) -> bool {
        TactCompactor::micro_compaction_enabled(self)
    }
}

/// Narrow context-policy view of the product compactor.
#[async_trait]
pub(crate) trait Compactor: Send + Sync {
    async fn compact(
        &self,
        state: &ContextView<'_>,
        request: CompactionRequest,
        model: &dyn ModelPort,
        cancel: CancelToken,
    ) -> Result<CompactionOutcome, ContextError>;

    async fn commit_pending(
        &self,
        state: &ContextView<'_>,
        request: &CompactionRequest,
    ) -> Result<(), ContextError>;

    fn session_start_pending(&self) -> bool;

    fn take_session_start_pending(&self) -> bool;

    fn session_start_source(&self) -> SessionStartSource;

    fn micro_compaction_enabled(&self) -> bool;
}

pub(crate) fn trigger_for(kind: CompactionKind) -> CompactTrigger {
    match kind {
        CompactionKind::Auto => CompactTrigger::Auto,
        CompactionKind::Manual => CompactTrigger::Manual,
        CompactionKind::Recovery => CompactTrigger::Recovery,
        CompactionKind::Command => CompactTrigger::Command,
        CompactionKind::Micro => CompactTrigger::Auto,
    }
}

fn summary_request(
    model: String,
    messages: Vec<ModelMessage>,
    max_output_tokens: u32,
    options: ModelCallOptions,
) -> ModelRequest {
    ModelRequest {
        model,
        system: String::new(),
        messages,
        tools: Vec::new(),
        max_output_tokens,
        options,
    }
}

fn compaction_error(error: impl std::fmt::Display) -> ContextError {
    ContextError::Compaction(error.to_string())
}

/// The `ContextError` one exhausted summarizer attempt reports.
///
/// A cancelled model call is the user or the host stopping the compaction, not
/// the summarizer breaking: it must reach the kernel as
/// [`ContextError::Cancelled`] so the card closes `Cancelled` and `/compact`
/// does not print a failure line. Every other kind is a real compaction
/// failure, carried with its provider message.
///
/// Both retry loops go through here so neither can forget the distinction —
/// forgetting it is exactly how a cancel used to be reported as `Failed`.
fn summary_failure(error: &tact_agent_api::ModelError, failure: FailureKind) -> ContextError {
    match failure {
        FailureKind::Cancelled => ContextError::Cancelled,
        FailureKind::PromptTooLong | FailureKind::Transient | FailureKind::Permanent => {
            ContextError::Compaction(error.to_string())
        }
    }
}

/// Turn the native path's diagnostic line into text a compaction card can show.
///
/// The line is `[responses compacted: items=N, id=X]`: a log row, brackets and
/// field names included, meant for the TUI. A host that draws a compaction
/// card needs the same fact as prose, and it must keep the bounded id prefix
/// that [`compact_id_display`] produced — the provider's full identifier never
/// leaves the protocol baseline.
///
/// A notice this function cannot read still produces a sentence rather than
/// `None`: the caller only reaches it when the compaction replaced something,
/// and dropping the text there would put an empty card in front of the user.
fn native_compaction_readout(notice: &str) -> String {
    let field = |name: &str| -> Option<&str> {
        let rest = notice.split_once(name)?.1;
        let value = rest.split([',', ']']).next()?.trim();
        (!value.is_empty()).then_some(value)
    };
    match (field("items="), field("id=")) {
        (Some(items), Some(id)) => {
            format!("Native Responses compaction kept {items} items (provider id {id}).")
        }
        (Some(items), None) => format!("Native Responses compaction kept {items} items."),
        _ => NATIVE_COMPACTION_FALLBACK_SUMMARY.to_string(),
    }
}

fn compact_id_display(id: &str) -> String {
    const MAX_ID_CHARS: usize = 12;
    id.chars().take(MAX_ID_CHARS).collect()
}

fn compact_response_note(
    stop_reason: Option<&StopReason>,
    usage: Option<&tact_protocol::TokenUsageInfo>,
) -> String {
    let stop = match stop_reason {
        None => "none".to_string(),
        Some(StopReason::EndTurn) => "end_turn".to_string(),
        Some(StopReason::MaxTokens) => "max_tokens".to_string(),
        Some(StopReason::StopSequence) => "stop_sequence".to_string(),
        Some(StopReason::ToolUse) => "tool_use".to_string(),
        Some(StopReason::Refusal) => "refusal".to_string(),
        Some(StopReason::PauseTurn) => "pause_turn".to_string(),
        Some(StopReason::Unknown { raw }) => format!("unknown({raw})"),
    };
    match usage {
        Some(usage) => format!(
            "stop={stop}, prompt {}, completion {} (reasoning {}), cache {}/{}",
            usage.prompt,
            usage.completion,
            usage.reasoning_tokens,
            usage.prompt_cache_hit_tokens,
            usage.prompt_cache_miss_tokens,
        ),
        None => format!("stop={stop}, no usage reported"),
    }
}

fn compact_truncation_note(reasoning_tokens: Option<u32>, think_block_bytes: usize) -> String {
    match reasoning_tokens {
        Some(tokens) => {
            format!("{tokens} reasoning tokens, thinking block {think_block_bytes} bytes")
        }
        None => format!("thinking block {think_block_bytes} bytes"),
    }
}

#[cfg(test)]
mod tests;
