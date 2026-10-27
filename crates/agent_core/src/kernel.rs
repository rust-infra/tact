use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use futures_util::future::BoxFuture;
use futures_util::stream::FuturesUnordered;
use tact_agent_api::{
    AgentContentBlock, AgentEvent, AgentMessage, AgentNotice, AgentRole, AgentState, CancelToken,
    CompactionOutcome, CompactionRequest, ContextError, ContextPreparation, ContextReplacement,
    ContextView, CursorUpdate, GateDecision, HostYieldReason, MessageDelta, MessageOrigin,
    MessageRange, ModelCallKind, ModelCapabilities, ModelCursor, ModelError, ModelErrorKind,
    ModelEvent, ModelPort, ModelRequest, ModelResponse, PreparationDecision, RecoveryStats,
    RequestPlan, ResolvedToolCall, RunInput, RunOutcome, RunStop, SessionRef, StopReason, ToolCall,
    ToolDirective, ToolError, ToolExecutionContext, ToolExecutionError, ToolExecutionErrorKind,
    ToolExecutionResult, ToolExecutionStatus, ToolGateContext, ToolGateError, ToolOutputChunk,
    ToolProgressSink, ToolSnapshot, UsageRecord,
};

use crate::deps::{AgentKernel, KernelError};
use crate::model_scope::{ObservedCall, ObservedModelPort};
use crate::recovery::{
    MAX_CONTINUATION_ATTEMPTS, MAX_TRANSPORT_ATTEMPTS, backoff_delay, continuation_message,
};
use crate::scheduling::waves_grouped;
use crate::state::apply_cursor;

const CANCELLED_RESULT: &str = "Cancelled by user";
const REFUSED_RESULT: &str = "Not executed: the model refused this request";
const TRUNCATED_RESULT: &str = "Not executed: output was truncated before this tool ran";
const UNKNOWN_STOP_RESULT: &str =
    "Not executed: the model turn ended with an unsupported stop reason";
const TURN_ENDED_RESULT: &str = "Not executed: the model ended the turn before this tool ran";
const INTERRUPTED_RESULT: &str = "Activation interrupted before result commit";

enum ModelAttempt {
    Complete {
        response: ModelResponse,
        call: Box<ObservedCall>,
    },
    Overflow(ModelError),
    Error(ModelError),
    Cancelled,
}

#[derive(Debug, Clone, Copy)]
enum DispatchTerminal {
    Cancelled,
    CompleteRun,
    YieldToHost,
}

struct ToolDispatchOutcome {
    terminal: Option<DispatchTerminal>,
    compaction: Option<CompactionRequest>,
    tool_calls: u64,
}

struct ToolSlot {
    raw: ToolCall,
    resolved: Option<ResolvedToolCall>,
    permission_label: Option<String>,
    result: Option<ToolExecutionResult>,
}

struct KernelProgressSink {
    call_id: String,
    events: crate::deps::EventBus,
}

struct RunPreparationContext<'a> {
    session: &'a SessionRef,
    raw_inputs: &'a [AgentMessage],
    max_total_turns: Option<u32>,
    run_id: &'a tact_agent_api::RunId,
    model: &'a ObservedModelPort,
    cancel: &'a CancelToken,
}

impl ToolProgressSink for KernelProgressSink {
    fn emit(&self, chunks: Vec<ToolOutputChunk>) {
        if !chunks.is_empty() {
            self.events.emit(AgentEvent::ToolProgress {
                call_id: self.call_id.clone(),
                chunks,
            });
        }
    }
}

impl AgentKernel {
    pub async fn run(
        &self,
        state: &mut AgentState,
        input: RunInput,
        cancel: CancelToken,
    ) -> Result<RunOutcome, KernelError> {
        let run_id = input.run_id.clone();
        self.events.emit(AgentEvent::RunStarted {
            run_id: run_id.clone(),
        });
        let result = self.run_inner(state, input, cancel, run_id).await;
        match result {
            Ok(outcome) => {
                if let Err(error) = self.deps.usage.flush().await {
                    let error = KernelError::Usage(error.to_string());
                    self.events.emit(AgentEvent::RunFailed {
                        error: error.info(),
                    });
                    Err(error)
                } else {
                    self.events.emit(AgentEvent::RunFinished {
                        outcome: outcome.clone(),
                    });
                    Ok(outcome)
                }
            }
            Err(error) => {
                let _ = self.deps.usage.flush().await;
                self.events.emit(AgentEvent::RunFailed {
                    error: error.info(),
                });
                Err(error)
            }
        }
    }

    pub async fn compact(
        &self,
        state: &mut AgentState,
        session: &SessionRef,
        run_id: tact_agent_api::RunId,
        request: CompactionRequest,
        cancel: CancelToken,
    ) -> Result<CompactionOutcome, KernelError> {
        let observed = ObservedModelPort::new(self.deps.model.clone(), self.events.clone(), run_id);
        let outcome = self
            .compact_state(state, session, request, cancel, &observed)
            .await?;
        self.deps
            .usage
            .flush()
            .await
            .map_err(|error| KernelError::Usage(error.to_string()))?;
        Ok(outcome)
    }

    async fn run_inner(
        &self,
        state: &mut AgentState,
        input: RunInput,
        cancel: CancelToken,
        run_id: tact_agent_api::RunId,
    ) -> Result<RunOutcome, KernelError> {
        if !state.initialized {
            let snapshot = self
                .deps
                .transcript
                .load(&input.session)
                .await
                .map_err(|error| KernelError::Load(error.to_string()))?;
            state.transcript = snapshot.transcript;
            state.cursor = snapshot.cursor;
            state.initialized = true;
        }

        self.reconcile_interrupted_tools(state, &input.session)
            .await?;

        if cancel.is_cancelled() {
            return Ok(self.outcome(state, 0, 0, RunStop::Cancelled, &RecoveryStats::default()));
        }

        let model =
            ObservedModelPort::new(self.deps.model.clone(), self.events.clone(), run_id.clone());
        let mut activation_turns = 0;
        let mut tool_calls = 0;
        let mut recovery = RecoveryStats::default();
        let mut raw_inputs = input.inputs;

        loop {
            if cancel.is_cancelled() {
                return Ok(self.outcome(
                    state,
                    activation_turns,
                    tool_calls,
                    RunStop::Cancelled,
                    &recovery,
                ));
            }

            let preparation = {
                let context = RunPreparationContext {
                    session: &input.session,
                    raw_inputs: &raw_inputs,
                    max_total_turns: input.max_total_turns,
                    run_id: &run_id,
                    model: &model,
                    cancel: &cancel,
                };
                let decision = self.prepare_run_loop(state, &context).await?;
                self.record_context_model_calls(&model).await?;
                decision
            };

            match preparation {
                PreparationDecision::Block(stop) => {
                    return Ok(self.outcome(state, activation_turns, tool_calls, stop, &recovery));
                }
                PreparationDecision::Compact { .. } => {
                    return Err(KernelError::Contract(
                        "prepare_run_loop returned a compact decision".into(),
                    ));
                }
                PreparationDecision::Continue(preparation) => {
                    self.apply_context_preparation(state, &input.session, preparation)
                        .await?;
                }
            }
            raw_inputs.clear();

            if let Some(max) = input.max_total_turns
                && state.total_turns >= max
            {
                self.events.emit(AgentEvent::Notice {
                    notice: AgentNotice::TurnLimitReached { max },
                });
                return Ok(self.outcome(
                    state,
                    activation_turns,
                    tool_calls,
                    RunStop::MaxTurns,
                    &recovery,
                ));
            }

            let request_plan = {
                let capabilities = self.deps.model.capabilities();
                let decision = self
                    .prepare_request_loop(
                        state,
                        &input.session,
                        input.max_total_turns,
                        &capabilities,
                        &model,
                        &cancel,
                    )
                    .await?;
                self.record_context_model_calls(&model).await?;
                decision
            };

            let request_plan = match request_plan {
                PreparationDecision::Block(stop) => {
                    return Ok(self.outcome(state, activation_turns, tool_calls, stop, &recovery));
                }
                PreparationDecision::Compact { .. } => {
                    return Err(KernelError::Contract(
                        "prepare_request_loop returned a compact decision".into(),
                    ));
                }
                PreparationDecision::Continue(plan) => plan,
            };

            self.append_messages(state, &input.session, &request_plan.appended_messages)
                .await?;

            if cancel.is_cancelled() {
                return Ok(self.outcome(
                    state,
                    activation_turns,
                    tool_calls,
                    RunStop::Cancelled,
                    &recovery,
                ));
            }

            let tools = self
                .deps
                .tools
                .refresh(cancel.clone())
                .await
                .map_err(|error| KernelError::Tool(error.to_string()))?;
            let remaining_turns = input
                .max_total_turns
                .map(|max| max.saturating_sub(state.total_turns));

            let model_request = self.build_model_request(&request_plan, &tools);
            let cursor = request_plan
                .request
                .cursor
                .clone()
                .or_else(|| state.cursor.clone());

            if cancel.is_cancelled() {
                return Ok(self.outcome(
                    state,
                    activation_turns,
                    tool_calls,
                    RunStop::Cancelled,
                    &recovery,
                ));
            }

            state.total_turns = state.total_turns.saturating_add(1);
            activation_turns = activation_turns.saturating_add(1);
            self.events.emit(AgentEvent::TurnStarted {
                run_id: run_id.clone(),
                turn: state.total_turns,
                activation_turn: activation_turns,
                max_total_turns: input.max_total_turns,
            });

            let attempt = self
                .run_model_attempt(&model, model_request, cursor, &cancel, &mut recovery)
                .await;
            let (response, observed_call) = match attempt {
                ModelAttempt::Cancelled => {
                    return Ok(self.outcome(
                        state,
                        activation_turns,
                        tool_calls,
                        RunStop::Cancelled,
                        &recovery,
                    ));
                }
                ModelAttempt::Overflow(error) => {
                    recovery.compact_attempts = recovery.compact_attempts.saturating_add(1);
                    let outcome = self
                        .compact_state(
                            state,
                            &input.session,
                            CompactionRequest::Recovery { focus: None },
                            cancel.clone(),
                            &model,
                        )
                        .await?;
                    if matches!(outcome, CompactionOutcome::Skipped { .. }) {
                        return Err(KernelError::Model(error.to_string()));
                    }
                    self.events.emit(AgentEvent::TurnFinished {
                        turn: state.total_turns,
                        activation_turn: activation_turns,
                    });
                    continue;
                }
                ModelAttempt::Error(error) => {
                    return Err(KernelError::Model(error.to_string()));
                }
                ModelAttempt::Complete { response, call } => (response, *call),
            };

            let receipt = self
                .deps
                .transcript
                .append(&input.session, &response.message, response.cursor.clone())
                .await
                .map_err(|error| KernelError::Store(error.to_string()))?;
            apply_cursor(&mut state.cursor, response.cursor.clone());
            state.transcript.push(response.message.clone());
            self.events.emit(AgentEvent::MessageFinished {
                message_id: response.message_id.clone(),
                message: response.message.clone(),
            });
            self.record_usage(observed_call, Some(receipt.range), &response)
                .await?;
            state.last_token_total = response
                .usage
                .map(|usage| usage.total_tokens)
                .or(state.last_token_total);

            if cancel.is_cancelled() {
                let calls = tool_calls_from_message(&response.message);
                self.persist_synthetic_results(state, &input.session, &calls, |_| {
                    CANCELLED_RESULT.to_owned()
                })
                .await?;
                self.events.emit(AgentEvent::TurnFinished {
                    turn: state.total_turns,
                    activation_turn: activation_turns,
                });
                return Ok(self.outcome(
                    state,
                    activation_turns,
                    tool_calls,
                    RunStop::Cancelled,
                    &recovery,
                ));
            }

            let calls = tool_calls_from_message(&response.message);
            let max_tokens = matches!(response.stop_reason, StopReason::MaxTokens);
            let continuation_exhausted =
                recovery.continuation_attempts >= MAX_CONTINUATION_ATTEMPTS;
            let execute_calls = matches!(response.stop_reason, StopReason::ToolUse)
                || (max_tokens && !continuation_exhausted);

            if execute_calls && !calls.is_empty() {
                let dispatch = self
                    .execute_tool_calls(
                        state,
                        calls.clone(),
                        &input.session,
                        remaining_turns,
                        cancel.clone(),
                        !max_tokens,
                    )
                    .await?;
                tool_calls = tool_calls.saturating_add(dispatch.tool_calls);
                self.events.emit(AgentEvent::TurnFinished {
                    turn: state.total_turns,
                    activation_turn: activation_turns,
                });

                if let Some(terminal) = dispatch.terminal {
                    let stop = match terminal {
                        DispatchTerminal::Cancelled => RunStop::Cancelled,
                        DispatchTerminal::CompleteRun => RunStop::Completed,
                        DispatchTerminal::YieldToHost => RunStop::YieldToHost {
                            reason: HostYieldReason::NeedsUser,
                        },
                    };
                    return Ok(self.outcome(state, activation_turns, tool_calls, stop, &recovery));
                }

                if let Some(request) = dispatch.compaction {
                    let _ = self
                        .compact_state(state, &input.session, request, cancel.clone(), &model)
                        .await?;
                }

                if max_tokens {
                    recovery.continuation_attempts =
                        recovery.continuation_attempts.saturating_add(1);
                    self.events.emit(AgentEvent::Notice {
                        notice: AgentNotice::Continuation {
                            attempt: recovery.continuation_attempts,
                            max_attempts: MAX_CONTINUATION_ATTEMPTS,
                        },
                    });
                    self.append_continuation(state, &input.session, recovery.continuation_attempts)
                        .await?;
                    continue;
                }

                continue;
            }

            if !calls.is_empty() {
                let reason = match response.stop_reason {
                    StopReason::Refusal => REFUSED_RESULT,
                    StopReason::MaxTokens => TRUNCATED_RESULT,
                    StopReason::Unknown { .. } => UNKNOWN_STOP_RESULT,
                    _ => TURN_ENDED_RESULT,
                };
                self.persist_synthetic_results(state, &input.session, &calls, |_| {
                    reason.to_owned()
                })
                .await?;
            }

            self.events.emit(AgentEvent::TurnFinished {
                turn: state.total_turns,
                activation_turn: activation_turns,
            });

            match response.stop_reason {
                StopReason::MaxTokens => {
                    if continuation_exhausted {
                        return Ok(self.outcome(
                            state,
                            activation_turns,
                            tool_calls,
                            RunStop::MaxTokens,
                            &recovery,
                        ));
                    }
                    recovery.continuation_attempts =
                        recovery.continuation_attempts.saturating_add(1);
                    self.events.emit(AgentEvent::Notice {
                        notice: AgentNotice::Continuation {
                            attempt: recovery.continuation_attempts,
                            max_attempts: MAX_CONTINUATION_ATTEMPTS,
                        },
                    });
                    self.append_continuation(state, &input.session, recovery.continuation_attempts)
                        .await?;
                }
                StopReason::Refusal => {
                    return Err(KernelError::Model(
                        "model refused to process this request (stop_reason=refusal)".into(),
                    ));
                }
                StopReason::Unknown { raw } => {
                    return Err(KernelError::Model(format!(
                        "model returned unknown stop reason {raw:?}"
                    )));
                }
                StopReason::ToolUse if calls.is_empty() => {
                    return Err(KernelError::Contract(
                        "model reported tool_use without any tool calls".into(),
                    ));
                }
                StopReason::EndTurn
                | StopReason::StopSequence
                | StopReason::PauseTurn
                | StopReason::ToolUse => {
                    return Ok(self.outcome(
                        state,
                        activation_turns,
                        tool_calls,
                        RunStop::Completed,
                        &recovery,
                    ));
                }
            }
        }
    }

    async fn prepare_run_loop(
        &self,
        state: &mut AgentState,
        context: &RunPreparationContext<'_>,
    ) -> Result<PreparationDecision<ContextPreparation>, KernelError> {
        let capabilities = self.deps.model.capabilities();
        loop {
            if context.cancel.is_cancelled() {
                return Ok(PreparationDecision::Block(RunStop::Cancelled));
            }
            let remaining_turns = context
                .max_total_turns
                .map(|max| max.saturating_sub(state.total_turns));
            let view = ContextView {
                session: context.session,
                transcript: &state.transcript,
                cursor: state.cursor.as_ref(),
                total_turns: state.total_turns,
                max_total_turns: context.max_total_turns,
                last_token_total: state.last_token_total,
                pending_inputs: context.raw_inputs,
                capabilities: &capabilities,
                remaining_turns,
            };
            let decision = self
                .deps
                .context
                .prepare_run(
                    &RunInput {
                        run_id: context.run_id.clone(),
                        session: context.session.clone(),
                        inputs: context.raw_inputs.to_vec(),
                        max_total_turns: context.max_total_turns,
                    },
                    &view,
                    context.model,
                    context.cancel.clone(),
                )
                .await
                .map_err(context_error)?;
            match decision {
                PreparationDecision::Compact {
                    request,
                    if_skipped,
                } => match self
                    .compact_state(
                        state,
                        context.session,
                        request,
                        context.cancel.clone(),
                        context.model,
                    )
                    .await?
                {
                    CompactionOutcome::Replaced(_) => continue,
                    CompactionOutcome::Skipped { .. } => {
                        return Ok(PreparationDecision::Continue(if_skipped));
                    }
                },
                other => return Ok(other),
            }
        }
    }

    async fn prepare_request_loop(
        &self,
        state: &mut AgentState,
        session: &SessionRef,
        max_total_turns: Option<u32>,
        capabilities: &ModelCapabilities,
        model: &ObservedModelPort,
        cancel: &CancelToken,
    ) -> Result<PreparationDecision<RequestPlan>, KernelError> {
        loop {
            if cancel.is_cancelled() {
                return Ok(PreparationDecision::Block(RunStop::Cancelled));
            }
            let remaining_turns = max_total_turns.map(|max| max.saturating_sub(state.total_turns));
            let view = ContextView {
                session,
                transcript: &state.transcript,
                cursor: state.cursor.as_ref(),
                total_turns: state.total_turns,
                max_total_turns,
                last_token_total: state.last_token_total,
                pending_inputs: &[],
                capabilities,
                remaining_turns,
            };
            let decision = self
                .deps
                .context
                .prepare_request(&view, model, cancel.clone())
                .await
                .map_err(context_error)?;
            match decision {
                PreparationDecision::Compact {
                    request,
                    if_skipped,
                } => match self
                    .compact_state(state, session, request, cancel.clone(), model)
                    .await?
                {
                    CompactionOutcome::Replaced(_) => continue,
                    CompactionOutcome::Skipped { .. } => {
                        return Ok(PreparationDecision::Continue(if_skipped));
                    }
                },
                other => return Ok(other),
            }
        }
    }

    async fn apply_context_preparation(
        &self,
        state: &mut AgentState,
        session: &SessionRef,
        preparation: ContextPreparation,
    ) -> Result<(), KernelError> {
        self.append_messages(state, session, &preparation.inputs)
            .await?;
        self.append_messages(state, session, &preparation.appended_messages)
            .await
    }

    async fn append_messages(
        &self,
        state: &mut AgentState,
        session: &SessionRef,
        messages: &[AgentMessage],
    ) -> Result<(), KernelError> {
        for message in messages {
            self.deps
                .transcript
                .append(session, message, CursorUpdate::Unchanged)
                .await
                .map_err(|error| KernelError::Store(error.to_string()))?;
            state.transcript.push(message.clone());
        }
        Ok(())
    }

    async fn append_continuation(
        &self,
        state: &mut AgentState,
        session: &SessionRef,
        attempt: u32,
    ) -> Result<(), KernelError> {
        let message = AgentMessage::text(
            AgentRole::User,
            MessageOrigin::Continuation,
            continuation_message(attempt),
        );
        self.append_messages(state, session, &[message]).await
    }

    fn build_model_request(&self, request: &RequestPlan, tools: &ToolSnapshot) -> ModelRequest {
        let capabilities = self.deps.model.capabilities();
        ModelRequest {
            model: capabilities.model,
            system: request.request.system.clone(),
            messages: request.request.messages.clone(),
            tools: tools.specs.clone(),
            max_output_tokens: capabilities.max_output_tokens,
            options: request.request.options.clone(),
        }
    }

    async fn run_model_attempt(
        &self,
        model: &ObservedModelPort,
        request: ModelRequest,
        cursor: Option<ModelCursor>,
        cancel: &CancelToken,
        recovery: &mut RecoveryStats,
    ) -> ModelAttempt {
        let mut transport_attempts = 0;
        loop {
            if cancel.is_cancelled() {
                return ModelAttempt::Cancelled;
            }
            match model
                .stream(request.clone(), cursor.as_ref(), cancel.clone())
                .await
            {
                Ok(mut stream) => {
                    let aggregated = self.read_model_stream(&mut stream, cancel).await;
                    match aggregated {
                        Ok(response) => {
                            return match model.take_latest() {
                                Some(call) => ModelAttempt::Complete {
                                    response,
                                    call: Box::new(call),
                                },
                                None => ModelAttempt::Error(ModelError::new(
                                    ModelErrorKind::Permanent,
                                    "observed model call metadata was missing",
                                )),
                            };
                        }
                        Err(error) if error.kind == ModelErrorKind::Transient => {
                            if transport_attempts >= MAX_TRANSPORT_ATTEMPTS {
                                return ModelAttempt::Error(ModelError::new(
                                    ModelErrorKind::Permanent,
                                    error.to_string(),
                                ));
                            }
                            let delay = error
                                .retry_after
                                .unwrap_or_else(|| backoff_delay(transport_attempts));
                            self.emit_recovery_backoff(
                                transport_attempts + 1,
                                MAX_TRANSPORT_ATTEMPTS,
                                delay,
                                &error,
                            );
                            recovery.transport_attempts =
                                recovery.transport_attempts.saturating_add(1);
                            if self.wait_or_cancel(delay, cancel).await {
                                return ModelAttempt::Cancelled;
                            }
                            transport_attempts += 1;
                        }
                        Err(error) if error.kind == ModelErrorKind::ContextOverflow => {
                            return ModelAttempt::Overflow(error);
                        }
                        Err(error) if error.kind == ModelErrorKind::Cancelled => {
                            return ModelAttempt::Cancelled;
                        }
                        Err(error) => {
                            return ModelAttempt::Error(ModelError::new(
                                ModelErrorKind::Permanent,
                                error.to_string(),
                            ));
                        }
                    }
                }
                Err(error) if error.kind == ModelErrorKind::Transient => {
                    if transport_attempts >= MAX_TRANSPORT_ATTEMPTS {
                        return ModelAttempt::Error(ModelError::new(
                            ModelErrorKind::Permanent,
                            error.to_string(),
                        ));
                    }
                    let delay = error
                        .retry_after
                        .unwrap_or_else(|| backoff_delay(transport_attempts));
                    self.emit_recovery_backoff(
                        transport_attempts + 1,
                        MAX_TRANSPORT_ATTEMPTS,
                        delay,
                        &error,
                    );
                    recovery.transport_attempts = recovery.transport_attempts.saturating_add(1);
                    if self.wait_or_cancel(delay, cancel).await {
                        return ModelAttempt::Cancelled;
                    }
                    transport_attempts += 1;
                }
                Err(error) if error.kind == ModelErrorKind::ContextOverflow => {
                    return ModelAttempt::Overflow(error);
                }
                Err(error) if error.kind == ModelErrorKind::Cancelled => {
                    return ModelAttempt::Cancelled;
                }
                Err(error) => {
                    return ModelAttempt::Error(ModelError::new(
                        ModelErrorKind::Permanent,
                        error.to_string(),
                    ));
                }
            }
        }
    }

    async fn read_model_stream(
        &self,
        stream: &mut tact_agent_api::ModelStream,
        cancel: &CancelToken,
    ) -> Result<ModelResponse, ModelError> {
        let mut started_id: Option<String> = None;
        let mut terminal: Option<ModelResponse> = None;
        let mut thinking_open = false;

        while let Some(item) = stream.next().await {
            if terminal.is_some() {
                return Err(ModelError::new(
                    ModelErrorKind::Permanent,
                    "model stream emitted an event after its terminal response",
                ));
            }
            let event = match item {
                Ok(event) => event,
                Err(error) => {
                    if cancel.is_cancelled() {
                        return Err(ModelError::new(
                            ModelErrorKind::Cancelled,
                            "model stream cancelled",
                        ));
                    }
                    return Err(error);
                }
            };
            match event {
                ModelEvent::MessageStarted { message_id, role } => {
                    if started_id.is_some() {
                        return Err(ModelError::new(
                            ModelErrorKind::Permanent,
                            "model stream emitted MessageStarted twice",
                        ));
                    }
                    started_id = Some(message_id.as_str().to_owned());
                    self.events
                        .emit(AgentEvent::MessageStarted { message_id, role });
                }
                ModelEvent::MessageDelta { message_id, delta } => {
                    if let Some(started) = &started_id {
                        if started != message_id.as_str() {
                            return Err(ModelError::new(
                                ModelErrorKind::Permanent,
                                "model delta message id did not match MessageStarted",
                            ));
                        }
                    } else {
                        started_id = Some(message_id.as_str().to_owned());
                    }
                    match &delta {
                        MessageDelta::Thinking {
                            phase: tact_agent_api::ThinkingDelta::Started,
                        } => {
                            if thinking_open {
                                return Err(ModelError::new(
                                    ModelErrorKind::Permanent,
                                    "thinking Started arrived while already open",
                                ));
                            }
                            thinking_open = true;
                        }
                        MessageDelta::Thinking {
                            phase: tact_agent_api::ThinkingDelta::Delta { .. },
                        } if !thinking_open => {
                            return Err(ModelError::new(
                                ModelErrorKind::Permanent,
                                "thinking Delta arrived before Started",
                            ));
                        }
                        MessageDelta::Thinking {
                            phase: tact_agent_api::ThinkingDelta::Finished,
                        } => {
                            if !thinking_open {
                                return Err(ModelError::new(
                                    ModelErrorKind::Permanent,
                                    "thinking Finished arrived before Started",
                                ));
                            }
                            thinking_open = false;
                        }
                        _ => {}
                    }
                    self.events
                        .emit(AgentEvent::MessageDelta { message_id, delta });
                }
                ModelEvent::ProviderActivity { activity } => {
                    self.events.emit(AgentEvent::ProviderActivity { activity });
                }
                ModelEvent::Usage { .. } => {}
                ModelEvent::Finished(response) => {
                    if thinking_open {
                        return Err(ModelError::new(
                            ModelErrorKind::Permanent,
                            "thinking block was still open at terminal response",
                        ));
                    }
                    if let Some(started) = &started_id
                        && started != response.message_id.as_str()
                    {
                        return Err(ModelError::new(
                            ModelErrorKind::Permanent,
                            "terminal message id did not match MessageStarted",
                        ));
                    }
                    terminal = Some(response);
                }
            }
        }

        if let Some(response) = terminal {
            Ok(response)
        } else if cancel.is_cancelled() {
            Err(ModelError::new(
                ModelErrorKind::Cancelled,
                "model stream cancelled before terminal response",
            ))
        } else {
            Err(ModelError::new(
                ModelErrorKind::Permanent,
                "model stream ended without a terminal response",
            ))
        }
    }

    fn emit_recovery_backoff(
        &self,
        attempt: u32,
        max_attempts: u32,
        delay: Duration,
        error: &ModelError,
    ) {
        self.events.emit(AgentEvent::Notice {
            notice: AgentNotice::RecoveryBackoff {
                attempt,
                max_attempts,
                delay,
                summary: error.to_string(),
            },
        });
    }

    async fn wait_or_cancel(&self, delay: Duration, cancel: &CancelToken) -> bool {
        tokio::select! {
            _ = tokio::time::sleep(delay) => false,
            _ = cancel.cancelled() => true,
        }
    }

    async fn record_usage(
        &self,
        call: ObservedCall,
        message_range: Option<MessageRange>,
        response: &ModelResponse,
    ) -> Result<(), KernelError> {
        self.deps
            .usage
            .record(UsageRecord {
                call_id: call.call_id.clone(),
                kind: call.kind,
                usage: response.usage,
                request_body: response.request_body.clone(),
                message_range,
            })
            .await
            .map_err(|error| KernelError::Usage(error.to_string()))?;
        self.events.emit(AgentEvent::Usage {
            call_id: call.call_id,
            kind: call.kind,
            usage: response.usage,
            message_range,
        });
        Ok(())
    }

    async fn record_context_model_calls(
        &self,
        model: &ObservedModelPort,
    ) -> Result<(), KernelError> {
        for call in model.drain_calls() {
            let response = call.response.clone();
            self.record_usage(call, None, &response).await?;
        }
        Ok(())
    }

    async fn compact_state(
        &self,
        state: &mut AgentState,
        session: &SessionRef,
        request: CompactionRequest,
        cancel: CancelToken,
        model: &ObservedModelPort,
    ) -> Result<CompactionOutcome, KernelError> {
        if cancel.is_cancelled() {
            return Ok(CompactionOutcome::Skipped {
                reason: "cancelled before compaction".into(),
            });
        }
        let kind = request.kind();
        self.events.emit(AgentEvent::Notice {
            notice: AgentNotice::CompactionStarted { kind },
        });
        let committed_request = request.clone();
        model.set_call_kind(ModelCallKind::LocalCompact);
        let capabilities = self.deps.model.capabilities();
        let view = ContextView {
            session,
            transcript: &state.transcript,
            cursor: state.cursor.as_ref(),
            total_turns: state.total_turns,
            max_total_turns: None,
            last_token_total: state.last_token_total,
            pending_inputs: &[],
            capabilities: &capabilities,
            remaining_turns: None,
        };
        let outcome = self
            .deps
            .context
            .compact(&view, request, model, cancel)
            .await
            .map_err(context_error)?;

        for call in model.drain_calls() {
            let response = call.response.clone();
            self.record_usage(call, None, &response).await?;
            state.last_token_total = response
                .usage
                .map(|usage| usage.total_tokens)
                .or(state.last_token_total);
        }

        if let CompactionOutcome::Replaced(ContextReplacement { transcript, cursor }) = &outcome {
            self.deps
                .transcript
                .replace(session, transcript.as_deref(), cursor.clone())
                .await
                .map_err(|error| KernelError::Store(error.to_string()))?;
            if let Some(transcript) = transcript {
                state.transcript = transcript.clone();
            }
            apply_cursor(&mut state.cursor, cursor.clone());
            self.events.emit(AgentEvent::ContextCompacted { kind });

            let view = ContextView {
                session,
                transcript: &state.transcript,
                cursor: state.cursor.as_ref(),
                total_turns: state.total_turns,
                max_total_turns: None,
                last_token_total: state.last_token_total,
                pending_inputs: &[],
                capabilities: &capabilities,
                remaining_turns: None,
            };
            self.deps
                .context
                .compaction_committed(&view, &committed_request)
                .await
                .map_err(context_error)?;
        }
        Ok(outcome)
    }

    async fn reconcile_interrupted_tools(
        &self,
        state: &mut AgentState,
        session: &SessionRef,
    ) -> Result<(), KernelError> {
        let mut transcript = state.transcript.clone();
        let mut index = 0;
        let mut changed = false;
        while index < transcript.len() {
            if transcript[index].role != AgentRole::Assistant {
                index += 1;
                continue;
            }
            let ids: Vec<String> = transcript[index]
                .content
                .iter()
                .filter_map(|block| match block {
                    AgentContentBlock::ToolUse { id, .. } => Some(id.clone()),
                    _ => None,
                })
                .collect();
            if ids.is_empty() {
                index += 1;
                continue;
            }

            let next_is_results = transcript
                .get(index + 1)
                .is_some_and(AgentMessage::is_tool_result_only);
            let existing: HashMap<String, String> = if next_is_results {
                transcript[index + 1]
                    .content
                    .iter()
                    .filter_map(|block| match block {
                        AgentContentBlock::ToolResult {
                            tool_use_id,
                            content,
                        } => Some((tool_use_id.clone(), content.clone())),
                        _ => None,
                    })
                    .collect()
            } else {
                HashMap::new()
            };

            if ids.iter().all(|id| existing.contains_key(id)) {
                index += 2;
                continue;
            }

            let message = AgentMessage::new(
                AgentRole::User,
                MessageOrigin::ToolResult,
                ids.iter()
                    .map(|id| AgentContentBlock::ToolResult {
                        tool_use_id: id.clone(),
                        content: existing
                            .get(id)
                            .cloned()
                            .unwrap_or_else(|| INTERRUPTED_RESULT.to_owned()),
                    })
                    .collect(),
            );
            if next_is_results {
                transcript[index + 1] = message;
                index += 2;
            } else {
                transcript.insert(index + 1, message);
                index += 2;
            }
            changed = true;
        }

        if changed {
            self.deps
                .transcript
                .replace(session, Some(&transcript), CursorUpdate::Unchanged)
                .await
                .map_err(|error| KernelError::Load(error.to_string()))?;
            state.transcript = transcript;
        }
        Ok(())
    }

    async fn execute_tool_calls(
        &self,
        state: &mut AgentState,
        calls: Vec<ToolCall>,
        session: &SessionRef,
        remaining_turns: Option<u32>,
        cancel: CancelToken,
        allow_complete: bool,
    ) -> Result<ToolDispatchOutcome, KernelError> {
        let mut seen = HashSet::new();
        for call in &calls {
            if !seen.insert(call.id.clone()) {
                return Err(KernelError::Contract(format!(
                    "duplicate tool call id {}",
                    call.id
                )));
            }
        }

        let mut slots = Vec::with_capacity(calls.len());
        for raw in calls.iter().cloned() {
            let descriptor = self
                .deps
                .tools
                .resolve(&raw)
                .ok()
                .map(|resolved| resolved.descriptor);
            self.events.emit(AgentEvent::ToolStarted {
                call: raw.clone(),
                descriptor,
            });
            slots.push(ToolSlot {
                raw,
                resolved: None,
                permission_label: None,
                result: None,
            });
        }

        let gate_context = ToolGateContext {
            session: session.clone(),
            total_turns: state.total_turns,
            remaining_turns,
            metadata: serde_json::json!({}),
        };

        for index in 0..slots.len() {
            if cancel.is_cancelled() {
                break;
            }
            let raw = slots[index].raw.clone();
            match self
                .deps
                .gate
                .before(raw, self.deps.tools.as_ref(), &gate_context, cancel.clone())
                .await
            {
                Ok(GateDecision::Allow { call, label }) => {
                    slots[index].resolved = Some(*call);
                    slots[index].permission_label = label;
                }
                Ok(GateDecision::Deny { reason }) => {
                    let result = failed_result(ToolExecutionErrorKind::Tool, reason);
                    self.emit_tool_finished(index, &slots, &result, Duration::ZERO);
                    slots[index].result = Some(result);
                }
                Err(ToolGateError::Cancelled) => {
                    cancel.cancel();
                    break;
                }
                Err(error) => {
                    let result = failed_result(ToolExecutionErrorKind::Adapter, error.to_string());
                    self.emit_tool_finished(index, &slots, &result, Duration::ZERO);
                    slots[index].result = Some(result);
                }
            }
        }

        if cancel.is_cancelled() {
            self.fill_unexecuted(&mut slots, CANCELLED_RESULT);
            let terminal = Some(DispatchTerminal::Cancelled);
            self.persist_tool_results(state, session, &slots).await?;
            return Ok(ToolDispatchOutcome {
                terminal,
                compaction: None,
                tool_calls: 0,
            });
        }

        let approved: Vec<usize> = slots
            .iter()
            .enumerate()
            .filter_map(|(index, slot)| slot.resolved.as_ref().map(|_| index))
            .collect();
        let resources: Vec<_> = approved
            .iter()
            .map(|index| {
                slots[*index]
                    .resolved
                    .as_ref()
                    .expect("approved slot")
                    .access
                    .clone()
            })
            .collect();
        let waves = waves_grouped(&resources);
        let mut executed = 0u64;
        let mut terminal = None;
        let mut compaction = None;

        for wave in waves {
            let mut futures: FuturesUnordered<
                BoxFuture<'static, (usize, Duration, ToolExecutionResult)>,
            > = FuturesUnordered::new();
            for resource_index in wave {
                let slot_index = approved[resource_index];
                let resolved = slots[slot_index]
                    .resolved
                    .as_ref()
                    .expect("approved slot")
                    .clone();
                let call_id = resolved.call_id.clone();
                let progress = Arc::new(KernelProgressSink {
                    call_id,
                    events: self.events.clone(),
                });
                let execution_context = ToolExecutionContext {
                    call_id: resolved.call_id.clone(),
                    progress,
                    cancel: cancel.clone(),
                    remaining_turns,
                };
                let tools = self.deps.tools.clone();
                let gate = self.deps.gate.clone();
                let after_context = gate_context.clone();
                let after_cancel = cancel.clone();
                futures.push(Box::pin(async move {
                    let started = Instant::now();
                    let mut result = match tools.execute(resolved.clone(), execution_context).await
                    {
                        Ok(result) => result,
                        Err(error) => adapter_error_result(error),
                    };
                    let original_error = result.error.clone();
                    match gate
                        .after(&resolved, result.clone(), &after_context, after_cancel)
                        .await
                    {
                        Ok(mut rewritten) => {
                            if result.status == ToolExecutionStatus::Failed {
                                rewritten.status = ToolExecutionStatus::Failed;
                                rewritten.error = original_error.clone();
                            }
                            result = rewritten;
                        }
                        Err(error) => {
                            result = failed_result(
                                ToolExecutionErrorKind::Adapter,
                                format!(
                                    "post-tool gate failed: {error}; original error: {}",
                                    original_error
                                        .as_ref()
                                        .map(|error| error.message.as_str())
                                        .unwrap_or("none")
                                ),
                            );
                        }
                    }
                    clear_failed_directives(&mut result);
                    (slot_index, started.elapsed(), result)
                }));
            }

            while let Some((slot_index, duration, result)) = futures.next().await {
                slots[slot_index].result = Some(result.clone());
                self.emit_tool_finished(slot_index, &slots, &result, duration);
                executed = executed.saturating_add(1);
            }

            if cancel.is_cancelled() {
                terminal = Some(DispatchTerminal::Cancelled);
                break;
            }

            let wave_terminal =
                wave_terminal(&slots, allow_complete, remaining_turns, &mut compaction);
            if wave_terminal.is_some() {
                terminal = wave_terminal;
                break;
            }
        }

        let unexecuted_reason = match terminal {
            Some(DispatchTerminal::Cancelled) => CANCELLED_RESULT,
            Some(DispatchTerminal::CompleteRun) => "Not executed: another tool completed the run",
            Some(DispatchTerminal::YieldToHost) => "Not executed: the run yielded to the host",
            None => TURN_ENDED_RESULT,
        };
        self.fill_unexecuted(&mut slots, unexecuted_reason);
        self.persist_tool_results(state, session, &slots).await?;

        Ok(ToolDispatchOutcome {
            terminal,
            compaction,
            tool_calls: executed,
        })
    }

    fn emit_tool_finished(
        &self,
        slot_index: usize,
        slots: &[ToolSlot],
        result: &ToolExecutionResult,
        duration: Duration,
    ) {
        self.events.emit(AgentEvent::ToolFinished {
            call_id: slots[slot_index].raw.id.clone(),
            result: result.clone(),
            permission_label: slots[slot_index].permission_label.clone(),
            duration,
        });
    }

    fn fill_unexecuted(&self, slots: &mut [ToolSlot], reason: &str) {
        for index in 0..slots.len() {
            if slots[index].result.is_none() {
                let result = failed_result(ToolExecutionErrorKind::Tool, reason);
                self.emit_tool_finished(index, slots, &result, Duration::ZERO);
                slots[index].result = Some(result);
            }
        }
    }

    async fn persist_tool_results(
        &self,
        state: &mut AgentState,
        session: &SessionRef,
        slots: &[ToolSlot],
    ) -> Result<(), KernelError> {
        if slots.is_empty() {
            return Ok(());
        }
        let message = AgentMessage::new(
            AgentRole::User,
            MessageOrigin::ToolResult,
            slots.iter().flat_map(tool_result_blocks).collect(),
        );
        self.deps
            .transcript
            .append(session, &message, CursorUpdate::Unchanged)
            .await
            .map_err(|error| KernelError::Store(error.to_string()))?;
        state.transcript.push(message);
        Ok(())
    }

    async fn persist_synthetic_results(
        &self,
        state: &mut AgentState,
        session: &SessionRef,
        calls: &[ToolCall],
        reason: impl Fn(&ToolCall) -> String,
    ) -> Result<(), KernelError> {
        if calls.is_empty() {
            return Ok(());
        }
        let message = AgentMessage::new(
            AgentRole::User,
            MessageOrigin::ToolResult,
            calls
                .iter()
                .map(|call| AgentContentBlock::ToolResult {
                    tool_use_id: call.id.clone(),
                    content: reason(call),
                })
                .collect(),
        );
        self.deps
            .transcript
            .append(session, &message, CursorUpdate::Unchanged)
            .await
            .map_err(|error| KernelError::Store(error.to_string()))?;
        state.transcript.push(message);
        Ok(())
    }

    fn outcome(
        &self,
        state: &AgentState,
        activation_turns: u32,
        tool_calls: u64,
        stop: RunStop,
        recovery: &RecoveryStats,
    ) -> RunOutcome {
        if matches!(stop, RunStop::Cancelled) {
            self.events.emit(AgentEvent::Notice {
                notice: AgentNotice::CancellationRequested,
            });
        }
        RunOutcome {
            stop,
            turns: activation_turns,
            total_turns: state.total_turns,
            tool_calls,
            recovery: *recovery,
        }
    }
}

fn context_error(error: ContextError) -> KernelError {
    KernelError::Context(error.to_string())
}

fn failed_result(kind: ToolExecutionErrorKind, message: impl Into<String>) -> ToolExecutionResult {
    ToolExecutionResult::failure(ToolExecutionError {
        kind,
        message: message.into(),
    })
}

fn adapter_error_result(error: ToolError) -> ToolExecutionResult {
    failed_result(ToolExecutionErrorKind::Adapter, error.to_string())
}

fn clear_failed_directives(result: &mut ToolExecutionResult) {
    if result.status == ToolExecutionStatus::Failed {
        result.outcome.directives.clear();
    }
}

fn wave_terminal(
    slots: &[ToolSlot],
    allow_complete: bool,
    remaining_turns: Option<u32>,
    compaction: &mut Option<CompactionRequest>,
) -> Option<DispatchTerminal> {
    let mut complete = false;
    let mut yield_to_host = false;
    let mut request_compaction = None;
    for slot in slots {
        let Some(result) = &slot.result else {
            continue;
        };
        if result.status == ToolExecutionStatus::Failed {
            continue;
        }
        for directive in &result.outcome.directives {
            match directive {
                ToolDirective::CompleteRun => complete = true,
                ToolDirective::YieldToHost { .. } => yield_to_host = true,
                ToolDirective::RequestCompaction { focus } => {
                    if request_compaction.is_none() {
                        request_compaction = Some(CompactionRequest::Manual {
                            focus: focus.clone(),
                        });
                    }
                }
            }
        }
    }
    if complete && allow_complete && remaining_turns == Some(1) {
        Some(DispatchTerminal::CompleteRun)
    } else if yield_to_host {
        Some(DispatchTerminal::YieldToHost)
    } else {
        *compaction = request_compaction;
        None
    }
}

fn render_tool_result(result: &ToolExecutionResult) -> String {
    let mut rendered = String::new();
    for block in &result.outcome.content {
        if let AgentContentBlock::Text { text } = block {
            rendered.push_str(text);
        }
    }
    if rendered.is_empty() {
        rendered = result
            .error
            .as_ref()
            .map(|error| error.message.clone())
            .unwrap_or_default();
    }
    rendered
}

fn tool_result_blocks(slot: &ToolSlot) -> Vec<AgentContentBlock> {
    let mut blocks = vec![AgentContentBlock::ToolResult {
        tool_use_id: slot.raw.id.clone(),
        content: slot
            .result
            .as_ref()
            .map(render_tool_result)
            .unwrap_or_else(|| TURN_ENDED_RESULT.to_owned()),
    }];
    if let Some(result) = &slot.result {
        blocks.extend(
            result
                .outcome
                .content
                .iter()
                .filter(|block| matches!(block, AgentContentBlock::Image { .. }))
                .cloned(),
        );
    }
    blocks
}

fn tool_calls_from_message(message: &AgentMessage) -> Vec<ToolCall> {
    message
        .content
        .iter()
        .filter_map(|block| match block {
            AgentContentBlock::ToolUse { id, name, input } => Some(ToolCall {
                id: id.clone(),
                name: name.clone(),
                input: input.clone(),
            }),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests;
