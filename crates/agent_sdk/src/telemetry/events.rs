use tact_agent_api::{
    AgentEvent, AgentNotice, EventDelivery, EventObserver, RunStop, RuntimeErrorInfo,
};
use tracing::{debug, info, warn};

use super::{TelemetryConfig, debug_content};

/// Semantic tracing observer for the kernel event stream.
pub(crate) struct TelemetryEventObserver {
    config: TelemetryConfig,
}

impl TelemetryEventObserver {
    pub(crate) fn new(config: TelemetryConfig) -> Self {
        Self { config }
    }

    fn record(&self, event: &AgentEvent) {
        match event {
            AgentEvent::RunStarted { run_id } => {
                info!(target: "tact_agent_sdk", run_id = %run_id, "agent run started");
            }
            AgentEvent::TurnStarted { run_id, turn, .. } => {
                debug!(target: "tact_agent_sdk", run_id = %run_id, turn, "agent turn started");
            }
            AgentEvent::ModelCallStarted {
                call_id,
                kind,
                model,
                ..
            } => {
                debug!(
                    target: "tact_agent_sdk",
                    call_id = %call_id,
                    kind = ?kind,
                    model = %model,
                    "agent model call started"
                );
            }
            AgentEvent::ToolStarted { call, .. } => {
                debug!(target: "tact_agent_sdk", tool = %call.name, call_id = %call.id, "agent tool started");
                if self.config.record_content_enabled()
                    && let Ok(input) = serde_json::to_string(&call.input)
                {
                    debug_content("tool", "input", &input);
                }
            }
            AgentEvent::ToolFinished {
                call_id,
                result,
                duration,
                ..
            } => {
                debug!(
                    target: "tact_agent_sdk",
                    call_id = %call_id,
                    status = ?result.status,
                    duration_ms = duration.as_millis(),
                    "agent tool finished"
                );
                if self.config.record_content_enabled()
                    && let Ok(result) = serde_json::to_string(&result)
                {
                    debug_content("tool", "result", &result);
                }
            }
            AgentEvent::Usage {
                call_id,
                usage: Some(usage),
                ..
            } => {
                debug!(
                    target: "tact_agent_sdk",
                    call_id = %call_id,
                    input_tokens = usage.input_tokens,
                    output_tokens = usage.output_tokens,
                    total_tokens = usage.total_tokens,
                    "agent usage"
                );
            }
            AgentEvent::Usage { .. } => {}
            AgentEvent::ContextCompacted { kind } => {
                info!(target: "tact_agent_sdk", kind = ?kind, "agent context compacted");
            }
            AgentEvent::Notice { notice } => match notice {
                AgentNotice::RecoveryBackoff {
                    attempt,
                    max_attempts,
                    ..
                } => {
                    warn!(
                        target: "tact_agent_sdk",
                        attempt,
                        max_attempts,
                        "agent recovery backoff"
                    );
                }
                AgentNotice::CancellationRequested => {
                    info!(target: "tact_agent_sdk", "agent cancellation requested");
                }
                AgentNotice::TurnLimitReached { max } => {
                    warn!(target: "tact_agent_sdk", max, "agent turn limit reached");
                }
                AgentNotice::Continuation {
                    attempt,
                    max_attempts,
                } => {
                    debug!(
                        target: "tact_agent_sdk",
                        attempt,
                        max_attempts,
                        "agent continuation"
                    );
                }
                AgentNotice::CompactionStarted { kind } => {
                    debug!(target: "tact_agent_sdk", kind = ?kind, "agent compaction started");
                }
            },
            AgentEvent::TurnFinished { turn, .. } => {
                debug!(target: "tact_agent_sdk", turn, "agent turn finished");
            }
            AgentEvent::RunFinished { outcome } => {
                let status = match &outcome.stop {
                    RunStop::Completed => "completed",
                    RunStop::YieldToHost { .. } => "yielded",
                    RunStop::MaxTurns => "max_turns",
                    RunStop::MaxTokens => "max_tokens",
                    RunStop::Cancelled => "cancelled",
                    RunStop::Blocked { .. } => "blocked",
                };
                info!(
                    target: "tact_agent_sdk",
                    status,
                    turns = outcome.turns,
                    tool_calls = outcome.tool_calls,
                    "agent run finished"
                );
            }
            AgentEvent::RunFailed { error } => record_error(error),
            AgentEvent::MessageStarted { message_id, role } => {
                debug!(
                    target: "tact_agent_sdk",
                    message_id = %message_id,
                    role = ?role,
                    "agent message started"
                );
            }
            AgentEvent::MessageFinished {
                message_id,
                message,
            } => {
                debug!(
                    target: "tact_agent_sdk",
                    message_id = %message_id,
                    "agent message finished"
                );
                if self.config.record_content_enabled() {
                    debug_content("message", "transcript", &message.text_content());
                }
            }
            AgentEvent::ProviderActivity { .. } => {}
            AgentEvent::MessageDelta { .. } | AgentEvent::ToolProgress { .. } => {}
        }
    }
}

impl EventObserver for TelemetryEventObserver {
    fn observe(&self, event: &AgentEvent) {
        self.record(event);
    }

    fn observe_with_delivery(&self, event: &AgentEvent, delivery: EventDelivery) {
        if delivery == EventDelivery::Progress
            && matches!(
                event,
                AgentEvent::MessageDelta { .. } | AgentEvent::ToolProgress { .. }
            )
        {
            return;
        }
        self.record(event);
    }
}

fn record_error(error: &RuntimeErrorInfo) {
    warn!(
        target: "tact_agent_sdk",
        phase = ?error.phase,
        "agent run failed"
    );
}
