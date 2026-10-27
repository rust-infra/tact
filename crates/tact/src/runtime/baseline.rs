//! Executable F7 baseline fixture and normalized run comparator.

use std::collections::HashMap;

use serde::Deserialize;
use tact_agent_api::{
    AgentContentBlock, AgentMessage, AgentState, MessageOrigin, ModelCallKind, RunStop, UsageRecord,
};
use tact_protocol::AgentUpdate;

pub(crate) const REQUIRED_SCENARIOS: &[&str] = &[
    "simple_text",
    "tool_use_image_companion",
    "permission_denial",
    "transient_retry",
    "cancellation_during_stream",
    "cancellation_during_tool_wave",
    "injected_context",
    "hosted_web_search",
    "resumed_sqlite_history",
    "tui_full_area_blank_background",
];

#[derive(Debug, Deserialize)]
pub(crate) struct LegacyBaseline {
    pub(crate) schema_version: u32,
    pub(crate) captured_from: CapturedFrom,
    pub(crate) normalization: Normalization,
    pub(crate) scenarios: Vec<Scenario>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct CapturedFrom {
    pub(crate) path: String,
    pub(crate) commit: String,
    pub(crate) date: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Normalization {
    pub(crate) ids: String,
    pub(crate) durations: String,
    pub(crate) timestamps: String,
    pub(crate) paths: String,
    pub(crate) image_data: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Scenario {
    pub(crate) id: String,
    pub(crate) legacy_tests: Vec<String>,
    pub(crate) updates: Vec<String>,
    pub(crate) transcript: Vec<NormalizedMessage>,
    pub(crate) cursor: String,
    pub(crate) usage: Vec<String>,
    pub(crate) terminal: String,
    pub(crate) invariants: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub(crate) struct NormalizedMessage {
    pub(crate) role: String,
    pub(crate) origin: String,
    pub(crate) blocks: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NormalizedRun {
    pub(crate) updates: Vec<String>,
    pub(crate) transcript: Vec<NormalizedMessage>,
    pub(crate) cursor: String,
    pub(crate) usage: Vec<String>,
    pub(crate) terminal: String,
}

pub(crate) fn load() -> LegacyBaseline {
    serde_json::from_str(include_str!(
        "../../testdata/agent_kernel/legacy_baseline.json"
    ))
    .expect("legacy baseline must remain valid JSON")
}

/// Normalize one kernel-backed activation into the frozen fixture vocabulary.
///
/// `cursor` is supplied by the caller because the same observable state can be
/// retained or replaced depending on the activation's starting point; that
/// provenance is not recoverable from the final `AgentState` alone.
pub(crate) fn normalize_run(
    state: &AgentState,
    usage: &[UsageRecord],
    updates: &[AgentUpdate],
    stop: &RunStop,
    cursor: &str,
) -> NormalizedRun {
    NormalizedRun {
        updates: normalize_updates(updates),
        transcript: normalize_transcript(&state.transcript),
        cursor: cursor.to_string(),
        usage: usage.iter().map(normalize_usage).collect(),
        terminal: normalize_terminal(stop),
    }
}

pub(crate) fn assert_matches_fixture(scenario: &str, actual: &NormalizedRun) {
    let baseline = load();
    let expected = baseline
        .scenarios
        .iter()
        .find(|candidate| candidate.id == scenario)
        .unwrap_or_else(|| panic!("missing baseline scenario {scenario}"));

    assert_eq!(
        actual.updates, expected.updates,
        "{scenario}: host-visible update order or multiplicity changed"
    );
    assert_eq!(
        actual.transcript, expected.transcript,
        "{scenario}: transcript role/origin/block order changed"
    );
    assert_eq!(
        actual.cursor, expected.cursor,
        "{scenario}: provider cursor outcome changed"
    );
    assert_eq!(
        actual.usage, expected.usage,
        "{scenario}: usage record order or total changed"
    );
    assert_eq!(
        actual.terminal, expected.terminal,
        "{scenario}: terminal outcome changed"
    );
}

fn normalize_transcript(messages: &[AgentMessage]) -> Vec<NormalizedMessage> {
    let mut tool_names = HashMap::new();
    messages
        .iter()
        .map(|message| NormalizedMessage {
            role: match message.role {
                tact_agent_api::AgentRole::User => "user".into(),
                tact_agent_api::AgentRole::Assistant => "assistant".into(),
            },
            origin: normalize_origin(&message.origin),
            blocks: message
                .content
                .iter()
                .map(|block| normalize_block(block, &mut tool_names))
                .collect(),
        })
        .collect()
}

fn normalize_origin(origin: &MessageOrigin) -> String {
    match origin {
        MessageOrigin::User => "user".into(),
        MessageOrigin::Assistant => "assistant".into(),
        MessageOrigin::ToolResult => "tool_result".into(),
        MessageOrigin::InjectedContext { source } => format!("injected_context:{source}"),
        MessageOrigin::CompactionSummary => "compaction_summary".into(),
        MessageOrigin::GoalContext => "goal_context".into(),
        MessageOrigin::SubagentResult => "subagent_result".into(),
        MessageOrigin::Continuation => "continuation".into(),
    }
}

fn normalize_block(block: &AgentContentBlock, tool_names: &mut HashMap<String, String>) -> String {
    match block {
        AgentContentBlock::Text { text } => format!("text:{text}"),
        AgentContentBlock::Image { source } => {
            format!("image:{}:<redacted-image-data>", source.media_type)
        }
        AgentContentBlock::ToolUse { id, name, .. } => {
            tool_names.insert(id.clone(), name.clone());
            format!("tool_use:{name}")
        }
        AgentContentBlock::ToolResult {
            tool_use_id,
            content,
        } => {
            let name = tool_names
                .get(tool_use_id)
                .map(String::as_str)
                .unwrap_or("<unknown-tool>");
            format!("tool_result:{name}:{content}")
        }
        AgentContentBlock::Thinking { thinking, .. } => format!("thinking:{thinking}"),
        AgentContentBlock::RedactedThinking { .. } => "redacted_thinking:<redacted>".into(),
    }
}

fn normalize_usage(record: &UsageRecord) -> String {
    let kind = match record.kind {
        ModelCallKind::Stream => "stream",
        ModelCallKind::Complete => "complete",
        ModelCallKind::LocalCompact => "local_compact",
        ModelCallKind::NativeCompact => "native_compact",
    };
    let total = record
        .usage
        .map(|usage| usage.total_tokens.to_string())
        .unwrap_or_else(|| "none".into());
    format!("{kind}:total-{total}")
}

fn normalize_terminal(stop: &RunStop) -> String {
    match stop {
        RunStop::Completed => "completed".into(),
        RunStop::YieldToHost { .. } => "yield_to_host".into(),
        RunStop::MaxTurns => "max_turns".into(),
        RunStop::MaxTokens => "max_tokens".into(),
        RunStop::Cancelled => "cancelled".into(),
        RunStop::Blocked { phase, .. } => format!("blocked:{phase:?}"),
    }
}

#[derive(Default)]
struct UpdateNormalizer {
    tool_names: HashMap<String, String>,
}

impl UpdateNormalizer {
    fn normalize(&mut self, update: &AgentUpdate) -> String {
        match update {
            AgentUpdate::StepAdded(step) => {
                self.tool_names
                    .insert(step.tool_id.clone(), step.tool.clone());
                format!("kernel:tool_added:{}", step.tool)
            }
            AgentUpdate::StepStarted {
                tool_id, tool_name, ..
            } => {
                self.tool_names.insert(tool_id.clone(), tool_name.clone());
                if tool_name == "web_search" {
                    format!("kernel:web_search_started:{tool_id}")
                } else {
                    format!("kernel:tool_started:{tool_name}")
                }
            }
            AgentUpdate::StepFinished {
                tool_id, result, ..
            } => {
                if result.tool == "web_search" {
                    format!("kernel:web_search_finished:{tool_id}")
                } else {
                    let status = match result.status {
                        tact_protocol::StepStatus::Success => "succeeded",
                        tact_protocol::StepStatus::Failed => "failed",
                    };
                    format!("kernel:tool_finished:{}:{status}", result.tool)
                }
            }
            AgentUpdate::StepFailed { tool_id, error, .. } => {
                let tool = self
                    .tool_names
                    .get(tool_id)
                    .map(String::as_str)
                    .unwrap_or("<unknown-tool>");
                if error == "Cancelled by user" {
                    format!("kernel:tool_finished:{tool}:cancelled")
                } else {
                    format!("kernel:tool_failed:{tool}:{error}")
                }
            }
            AgentUpdate::ToolProgress { tool_id, chunks } => {
                format!("kernel:tool_progress:{tool_id}:{}", chunks.len())
            }
            AgentUpdate::TokenUsage(usage) => {
                format!("kernel:token_usage:total-{}", usage.total)
            }
            AgentUpdate::TurnStats { turns_taken, .. } => {
                format!("kernel:turn_stats:turn-{turns_taken}")
            }
            AgentUpdate::ModelInfo(_) => "kernel:model_info:stream".into(),
            AgentUpdate::StreamChunk(text) => format!("kernel:stream_chunk:{text}"),
            AgentUpdate::ThinkingChunk(chunk) => match chunk {
                tact_protocol::ThinkingChunk::Started => "kernel:thinking_started".into(),
                tact_protocol::ThinkingChunk::Delta(delta) => {
                    format!("kernel:thinking_delta:{delta}")
                }
                tact_protocol::ThinkingChunk::Finished => "kernel:thinking_finished".into(),
            },
            AgentUpdate::Info(text) => normalize_info(text),
            AgentUpdate::HookStatus {
                elapsed_ms: None, ..
            } => "adapter:hook_status:running".into(),
            AgentUpdate::HookStatus {
                elapsed_ms: Some(_),
                ..
            } => "adapter:hook_status:finished".into(),
            AgentUpdate::RequestSelect { .. } => "adapter:request_select".into(),
            AgentUpdate::RequestMultiSelect { .. } => "adapter:request_multi_select".into(),
            AgentUpdate::HookContext { source, .. } => {
                format!(
                    "adapter:hook_context:{}",
                    source.as_deref().unwrap_or("<unknown>")
                )
            }
            AgentUpdate::TaskComplete(message) => {
                format!("driver:task_complete:{message}")
            }
            AgentUpdate::TaskCancelled => "driver:task_cancelled".into(),
            AgentUpdate::TaskIdle => "driver:task_idle".into(),
            AgentUpdate::Error(error) => format!("driver:error:{error:?}"),
            _ => format!("unclassified:{update:?}"),
        }
    }
}

fn normalize_info(text: &str) -> String {
    match text {
        "Cancelled by user" => "kernel:info:cancelling".into(),
        "[auto compact]" => "kernel:info:auto_compact".into(),
        "[manual compact]" => "kernel:info:manual_compact".into(),
        _ if text.starts_with("[Recovery] backoff") => {
            let attempt = text
                .split_once('(')
                .and_then(|(_, rest)| rest.split_once('/'))
                .map(|(attempt, _)| attempt)
                .unwrap_or("?");
            format!("kernel:info:retry-{attempt}")
        }
        _ if text.starts_with("[Recovery] continue") => "kernel:info:recovery_continue".into(),
        _ => format!("kernel:info:{text}"),
    }
}

fn normalize_updates(updates: &[AgentUpdate]) -> Vec<String> {
    let mut normalizer = UpdateNormalizer::default();
    updates
        .iter()
        .map(|update| normalizer.normalize(update))
        .collect()
}

#[cfg(test)]
mod tests {
    use std::{collections::HashSet, sync::Arc, time::Duration};

    use tact_agent_api::test_support::{InMemoryTranscriptStore, RecordingUsageSink};
    use tact_agent_api::{
        AgentContentBlock, AgentImageSource, AgentMessage, AgentRole, AgentState, CancelToken,
        CursorUpdate, MessageDelta, MessageId, MessageOrigin, ModelError, ModelErrorKind,
        ModelEvent, ProviderActivity, ProviderActivityKind, ProviderActivityPhase, ResourceAccess,
        RunId, RunInput, RunOutcome, SessionRef, StopReason, TokenUsage, ToolExecutionError,
        ToolExecutionErrorKind, ToolExecutionResult, ToolOutcome, TranscriptStore,
    };
    use tact_agent_core::test_support::{
        ModelScript, ScriptedContext, ScriptedGate, ScriptedModel, ScriptedTools, ToolBehavior,
        ToolDefinition, events, events_with_deltas, text_response, tool_response,
    };
    use tact_agent_core::{AgentKernel, KernelDeps};
    use tact_llm::{ProviderConversationState, ResponsesConversationState, neutral::model_cursor};
    use tact_protocol::{AgentUpdate, UpdateSink};
    use tokio::sync::mpsc;

    use super::*;
    use crate::runtime::event_adapter::TactEventBridge;
    use crate::runtime::store_adapter::SqliteTranscriptStore;

    struct KernelHarness {
        kernel: AgentKernel,
        updates: mpsc::UnboundedReceiver<AgentUpdate>,
        usage: Arc<RecordingUsageSink>,
    }

    impl KernelHarness {
        async fn run(
            mut self,
            state: &mut AgentState,
            input: RunInput,
            cancel: CancelToken,
        ) -> (RunOutcome, Vec<AgentUpdate>, Arc<RecordingUsageSink>) {
            let outcome = self
                .kernel
                .run(state, input, cancel)
                .await
                .expect("kernel run");
            let mut updates = Vec::new();
            while let Ok(update) = self.updates.try_recv() {
                updates.push(update);
            }
            (outcome, updates, self.usage)
        }
    }

    fn harness(
        model: Arc<ScriptedModel>,
        tools: Arc<ScriptedTools>,
        gate: Arc<ScriptedGate>,
        context: Arc<ScriptedContext>,
        transcript: Arc<dyn TranscriptStore>,
        prelude: impl FnOnce(&UpdateSink),
    ) -> KernelHarness {
        let (tx, updates) = mpsc::unbounded_channel();
        let sink = UpdateSink::for_ui(tx);
        prelude(&sink);
        let usage = Arc::new(RecordingUsageSink::new());
        let kernel = AgentKernel::new(KernelDeps {
            model,
            tools,
            gate,
            context,
            transcript,
            usage: usage.clone(),
            events: Arc::new(TactEventBridge::new(sink)),
        });
        KernelHarness {
            kernel,
            updates,
            usage,
        }
    }

    fn fresh_state() -> AgentState {
        AgentState {
            initialized: true,
            ..AgentState::default()
        }
    }

    fn input(session: &str, text: &str) -> RunInput {
        RunInput {
            run_id: RunId::new(format!("{session}:fixture")),
            session: SessionRef::new(session),
            inputs: vec![AgentMessage::text(
                AgentRole::User,
                MessageOrigin::User,
                text,
            )],
            max_total_turns: None,
        }
    }

    fn total_usage(total_tokens: u32) -> TokenUsage {
        TokenUsage {
            input_tokens: total_tokens.saturating_sub(1),
            output_tokens: 1,
            total_tokens,
            ..TokenUsage::default()
        }
    }

    fn with_usage(
        mut response: tact_agent_api::ModelResponse,
        total: u32,
    ) -> tact_agent_api::ModelResponse {
        response.usage = Some(total_usage(total));
        response
    }

    fn text_script(id: &str, text: &str, total: u32) -> ModelScript {
        events_with_deltas(
            with_usage(text_response(id, text, StopReason::EndTurn), total),
            vec![MessageDelta::Text { delta: text.into() }],
        )
    }

    fn tool_success(content: Vec<AgentContentBlock>) -> ToolExecutionResult {
        ToolExecutionResult::success(ToolOutcome {
            content,
            directives: Vec::new(),
            metadata: None,
        })
    }

    fn tool_failure(message: &str) -> ToolExecutionResult {
        ToolExecutionResult::failure(ToolExecutionError {
            kind: ToolExecutionErrorKind::Tool,
            message: message.into(),
        })
    }

    fn append_driver_update(
        mut updates: Vec<AgentUpdate>,
        update: AgentUpdate,
    ) -> Vec<AgentUpdate> {
        updates.push(update);
        updates
    }

    #[test]
    fn fixture_freezes_every_required_legacy_scenario() {
        let baseline = load();
        assert_eq!(baseline.schema_version, 1);
        assert_eq!(baseline.captured_from.path, "legacy");
        assert_eq!(baseline.captured_from.commit.len(), 40);
        assert_eq!(baseline.captured_from.date, "2026-10-26");
        assert_eq!(baseline.normalization.ids, "<scenario>:<kind>:<ordinal>");
        assert_eq!(baseline.normalization.durations, "<duration>");
        assert_eq!(baseline.normalization.timestamps, "<timestamp>");
        assert_eq!(baseline.normalization.paths, "<path>");
        assert_eq!(baseline.normalization.image_data, "<redacted-image-data>");

        let ids = baseline
            .scenarios
            .iter()
            .map(|scenario| scenario.id.as_str())
            .collect::<HashSet<_>>();
        assert_eq!(
            ids.len(),
            baseline.scenarios.len(),
            "scenario ids are unique"
        );
        for required in REQUIRED_SCENARIOS {
            assert!(ids.contains(required), "missing scenario {required}");
        }
        for scenario in &baseline.scenarios {
            assert!(
                !scenario.legacy_tests.is_empty(),
                "{} must cite an existing assertion",
                scenario.id
            );
            assert!(!scenario.cursor.is_empty(), "{} cursor", scenario.id);
            assert!(!scenario.terminal.is_empty(), "{} terminal", scenario.id);
        }
    }

    #[test]
    fn fixture_keeps_image_and_usage_baseline_explicit() {
        let baseline = load();
        let image = baseline
            .scenarios
            .iter()
            .find(|scenario| scenario.id == "tool_use_image_companion")
            .expect("image scenario");
        let grouped = image
            .transcript
            .iter()
            .find(|message| message.origin == "tool_result")
            .expect("grouped tool result");
        assert_eq!(grouped.role, "user");
        assert_eq!(
            grouped.blocks,
            [
                "tool_result:read_image:image preview",
                "image:image/png:<redacted-image-data>"
            ]
        );
        assert!(
            image
                .invariants
                .iter()
                .any(|invariant| invariant.contains("immediately after"))
        );

        let simple = baseline
            .scenarios
            .iter()
            .find(|scenario| scenario.id == "simple_text")
            .expect("simple text scenario");
        assert_eq!(simple.usage, ["stream:total-7"]);
        assert!(
            simple
                .invariants
                .iter()
                .any(|invariant| invariant.contains("usage.total"))
        );
    }

    #[test]
    fn fixture_preserves_non_user_injection_and_hosted_activity_boundaries() {
        let baseline = load();
        let injected = baseline
            .scenarios
            .iter()
            .find(|scenario| scenario.id == "injected_context")
            .expect("injected context scenario");
        assert!(
            injected
                .transcript
                .iter()
                .any(|message| message.origin == "injected_context:hook")
        );

        let hosted = baseline
            .scenarios
            .iter()
            .find(|scenario| scenario.id == "hosted_web_search")
            .expect("hosted web search scenario");
        assert!(
            hosted
                .transcript
                .iter()
                .all(|message| message.origin != "tool_result")
        );
        assert!(
            hosted
                .updates
                .iter()
                .any(|update| update.starts_with("kernel:web_search_started:"))
        );
    }

    #[tokio::test]
    async fn simple_text_kernel_run_matches_frozen_legacy_baseline() {
        let harness = harness(
            ScriptedModel::new(vec![text_script("assistant-1", "hello", 7)]),
            ScriptedTools::new(Vec::new()),
            ScriptedGate::new(),
            ScriptedContext::new(),
            Arc::new(InMemoryTranscriptStore::new()),
            |_| {},
        );
        let mut state = fresh_state();
        let (outcome, updates, usage) = harness
            .run(
                &mut state,
                input("baseline-simple-text", "hello"),
                CancelToken::new(),
            )
            .await;
        let updates = append_driver_update(updates, AgentUpdate::TaskComplete("hello".into()));

        let actual = normalize_run(
            &state,
            &usage.records(),
            &updates,
            &outcome.stop,
            "unchanged",
        );
        assert_matches_fixture("simple_text", &actual);
    }

    #[tokio::test]
    async fn tool_use_image_companion_kernel_run_matches_frozen_legacy_baseline() {
        let first = with_usage(
            tool_response(
                "assistant-1",
                &[("call-image", "read_image", serde_json::json!({}))],
                StopReason::ToolUse,
            ),
            8,
        );
        let tools = ScriptedTools::new(vec![ToolDefinition::new(
            "read_image",
            ResourceAccess::independent(),
            ToolBehavior::Immediate(tool_success(vec![
                AgentContentBlock::Text {
                    text: "image preview".into(),
                },
                AgentContentBlock::Image {
                    source: AgentImageSource {
                        type_: "base64".into(),
                        media_type: "image/png".into(),
                        data: "AAAA".into(),
                    },
                },
            ])),
        )]);
        let harness = harness(
            ScriptedModel::new(vec![
                events(first),
                text_script("assistant-2", "captured", 21),
            ]),
            tools.clone(),
            ScriptedGate::new(),
            ScriptedContext::new(),
            Arc::new(InMemoryTranscriptStore::new()),
            |_| {},
        );
        let mut state = fresh_state();
        let (outcome, updates, usage) = harness
            .run(
                &mut state,
                input("baseline-image", "capture the image"),
                CancelToken::new(),
            )
            .await;
        assert_eq!(tools.executions(), ["read_image"]);
        let updates = append_driver_update(updates, AgentUpdate::TaskComplete("captured".into()));
        let actual = normalize_run(
            &state,
            &usage.records(),
            &updates,
            &outcome.stop,
            "unchanged",
        );
        assert_matches_fixture("tool_use_image_companion", &actual);
    }

    #[tokio::test]
    async fn permission_denial_kernel_run_matches_frozen_legacy_baseline() {
        let gate = ScriptedGate::new();
        gate.push_before(tact_agent_core::test_support::BeforeAction::Deny(
            "denied".into(),
        ));
        let first = with_usage(
            tool_response(
                "assistant-1",
                &[("call-write", "write_file", serde_json::json!({}))],
                StopReason::ToolUse,
            ),
            9,
        );
        let tools = ScriptedTools::new(Vec::new());
        let harness = harness(
            ScriptedModel::new(vec![
                events(first),
                text_script("assistant-2", "Edit denied", 12),
            ]),
            tools.clone(),
            gate,
            ScriptedContext::new(),
            Arc::new(InMemoryTranscriptStore::new()),
            |_| {},
        );
        let mut state = fresh_state();
        let (outcome, updates, usage) = harness
            .run(
                &mut state,
                input("baseline-denial", "edit the file"),
                CancelToken::new(),
            )
            .await;
        assert!(tools.executions().is_empty());
        let updates =
            append_driver_update(updates, AgentUpdate::TaskComplete("Edit denied".into()));
        let actual = normalize_run(
            &state,
            &usage.records(),
            &updates,
            &outcome.stop,
            "unchanged",
        );
        assert_matches_fixture("permission_denial", &actual);
    }

    #[tokio::test]
    async fn transient_retry_kernel_run_matches_frozen_legacy_baseline() {
        let harness = harness(
            ScriptedModel::new(vec![
                ModelScript::Error(ModelError {
                    kind: ModelErrorKind::Transient,
                    message: "timeout".into(),
                    retry_after: Some(Duration::ZERO),
                }),
                text_script("assistant-1", "recovered", 5),
            ]),
            ScriptedTools::new(Vec::new()),
            ScriptedGate::new(),
            ScriptedContext::new(),
            Arc::new(InMemoryTranscriptStore::new()),
            |_| {},
        );
        let mut state = fresh_state();
        let (outcome, updates, usage) = harness
            .run(
                &mut state,
                input("baseline-retry", "retry the request"),
                CancelToken::new(),
            )
            .await;
        let updates = append_driver_update(updates, AgentUpdate::TaskComplete("recovered".into()));
        let actual = normalize_run(
            &state,
            &usage.records(),
            &updates,
            &outcome.stop,
            "unchanged",
        );
        assert_matches_fixture("transient_retry", &actual);
    }

    #[tokio::test]
    async fn cancellation_during_stream_matches_frozen_legacy_baseline() {
        let model = ScriptedModel::new(vec![ModelScript::WaitForCancel(vec![
            Ok(ModelEvent::MessageStarted {
                message_id: MessageId::new("assistant-1"),
                role: AgentRole::Assistant,
            }),
            Ok(ModelEvent::MessageDelta {
                message_id: MessageId::new("assistant-1"),
                delta: MessageDelta::Text {
                    delta: "partial".into(),
                },
            }),
        ])]);
        let harness = harness(
            model,
            ScriptedTools::new(Vec::new()),
            ScriptedGate::new(),
            ScriptedContext::new(),
            Arc::new(InMemoryTranscriptStore::new()),
            |_| {},
        );
        let mut state = fresh_state();
        let cancel = CancelToken::new();
        let cancel_task = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            cancel_task.cancel();
        });
        let (outcome, updates, usage) = harness
            .run(
                &mut state,
                input("baseline-cancel-stream", "long task"),
                cancel,
            )
            .await;
        let updates = append_driver_update(updates, AgentUpdate::TaskCancelled);
        let actual = normalize_run(
            &state,
            &usage.records(),
            &updates,
            &outcome.stop,
            "unchanged",
        );
        assert_matches_fixture("cancellation_during_stream", &actual);
    }

    #[tokio::test]
    async fn cancellation_during_tool_wave_matches_frozen_legacy_baseline() {
        let first = with_usage(
            tool_response(
                "assistant-1",
                &[("call-bash", "bash", serde_json::json!({}))],
                StopReason::ToolUse,
            ),
            6,
        );
        let tools = ScriptedTools::new(vec![ToolDefinition::new(
            "bash",
            ResourceAccess::barrier(),
            ToolBehavior::WaitForCancel(tool_failure("Cancelled by user")),
        )]);
        let harness = harness(
            ScriptedModel::new(vec![events(first)]),
            tools.clone(),
            ScriptedGate::new(),
            ScriptedContext::new(),
            Arc::new(InMemoryTranscriptStore::new()),
            |_| {},
        );
        let mut state = fresh_state();
        let cancel = CancelToken::new();
        let cancel_task = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            cancel_task.cancel();
        });
        let (outcome, updates, usage) = harness
            .run(
                &mut state,
                input("baseline-cancel-wave", "run a command"),
                cancel,
            )
            .await;
        assert_eq!(tools.executions(), ["bash"]);
        let updates = append_driver_update(updates, AgentUpdate::TaskCancelled);
        let actual = normalize_run(
            &state,
            &usage.records(),
            &updates,
            &outcome.stop,
            "unchanged",
        );
        assert_matches_fixture("cancellation_during_tool_wave", &actual);
    }

    #[tokio::test]
    async fn injected_context_kernel_run_matches_frozen_legacy_baseline() {
        let harness = harness(
            ScriptedModel::new(vec![text_script("assistant-1", "ready", 4)]),
            ScriptedTools::new(Vec::new()),
            ScriptedGate::new(),
            ScriptedContext::new(),
            Arc::new(InMemoryTranscriptStore::new()),
            |sink| {
                sink.send(AgentUpdate::HookStatus {
                    id: 1,
                    source: Some("plugin".into()),
                    message: "plugin hook".into(),
                    elapsed_ms: None,
                });
                sink.send(AgentUpdate::HookContext {
                    source: Some("plugin".into()),
                    text: "hook briefing".into(),
                });
            },
        );
        let injected = AgentMessage::text(
            AgentRole::User,
            MessageOrigin::InjectedContext {
                source: "hook".into(),
            },
            "hook briefing",
        );
        let mut run_input = input("baseline-injected", "continue");
        run_input.inputs.insert(0, injected);
        let mut state = fresh_state();
        let (outcome, updates, usage) =
            harness.run(&mut state, run_input, CancelToken::new()).await;
        let updates = append_driver_update(updates, AgentUpdate::TaskComplete("ready".into()));
        let actual = normalize_run(
            &state,
            &usage.records(),
            &updates,
            &outcome.stop,
            "unchanged",
        );
        assert_matches_fixture("injected_context", &actual);
    }

    #[tokio::test]
    async fn hosted_web_search_kernel_run_matches_frozen_legacy_baseline() {
        let provider_state =
            ProviderConversationState::OpenAiResponses(ResponsesConversationState {
                version: 1,
                provider: "openai_responses".into(),
                base_url: "https://example.invalid".into(),
                model: "test".into(),
                input_items: vec![serde_json::json!({"type": "compaction"})],
                compaction_id: Some("cmp-hosted".into()),
                is_compacted: true,
                logical_message_count: 1,
                logical_context_hash: String::new(),
            });
        let mut response = with_usage(
            text_response("assistant-1", "answer", StopReason::EndTurn),
            15,
        );
        response.cursor = CursorUpdate::Replace {
            cursor: Some(model_cursor(&provider_state)),
        };
        let model = ScriptedModel::new(vec![ModelScript::Events(vec![
            Ok(ModelEvent::MessageStarted {
                message_id: MessageId::new("assistant-1"),
                role: AgentRole::Assistant,
            }),
            Ok(ModelEvent::ProviderActivity {
                activity: ProviderActivity {
                    id: "search-1".into(),
                    step: 0,
                    kind: ProviderActivityKind::WebSearch {
                        phase: ProviderActivityPhase::Started,
                        query: Some("Rust".into()),
                        failure: None,
                        duration: None,
                    },
                },
            }),
            Ok(ModelEvent::ProviderActivity {
                activity: ProviderActivity {
                    id: "search-1".into(),
                    step: 0,
                    kind: ProviderActivityKind::WebSearch {
                        phase: ProviderActivityPhase::Completed,
                        query: Some("Rust".into()),
                        failure: None,
                        duration: Some(Duration::from_millis(1)),
                    },
                },
            }),
            Ok(ModelEvent::MessageDelta {
                message_id: MessageId::new("assistant-1"),
                delta: MessageDelta::Text {
                    delta: "answer".into(),
                },
            }),
            Ok(ModelEvent::Finished(response)),
        ])]);
        let harness = harness(
            model,
            ScriptedTools::new(Vec::new()),
            ScriptedGate::new(),
            ScriptedContext::new(),
            Arc::new(InMemoryTranscriptStore::new()),
            |_| {},
        );
        let mut state = fresh_state();
        let (outcome, updates, usage) = harness
            .run(
                &mut state,
                input("baseline-hosted", "search"),
                CancelToken::new(),
            )
            .await;
        let updates = append_driver_update(updates, AgentUpdate::TaskComplete("answer".into()));
        let actual = normalize_run(
            &state,
            &usage.records(),
            &updates,
            &outcome.stop,
            "replaced-provider-state",
        );
        assert_matches_fixture("hosted_web_search", &actual);
    }

    #[tokio::test]
    async fn resumed_sqlite_history_kernel_run_matches_frozen_legacy_baseline() {
        let dir = tempfile::tempdir().unwrap();
        let store =
            crate::store::session_store::open_sqlite_session_store(&dir.path().join("sessions.db"))
                .await
                .unwrap();
        store
            .ensure_session_row("baseline-resume", "/workspace", "")
            .await
            .unwrap();
        let transcript = Arc::new(SqliteTranscriptStore::new(store));
        let session = SessionRef::new("baseline-resume");
        transcript
            .append(
                &session,
                &AgentMessage::text(AgentRole::User, MessageOrigin::User, "earlier question"),
                CursorUpdate::Unchanged,
            )
            .await
            .unwrap();
        let provider_state =
            ProviderConversationState::OpenAiResponses(ResponsesConversationState {
                version: 1,
                provider: "openai_responses".into(),
                base_url: "https://example.invalid".into(),
                model: "test".into(),
                input_items: vec![serde_json::json!({"type": "message", "id": "earlier"})],
                compaction_id: None,
                is_compacted: false,
                logical_message_count: 2,
                logical_context_hash: "retained".into(),
            });
        transcript
            .append(
                &session,
                &AgentMessage::text(
                    AgentRole::Assistant,
                    MessageOrigin::Assistant,
                    "earlier answer",
                ),
                CursorUpdate::Replace {
                    cursor: Some(model_cursor(&provider_state)),
                },
            )
            .await
            .unwrap();
        let harness = harness(
            ScriptedModel::new(vec![text_script("assistant-2", "continued", 11)]),
            ScriptedTools::new(Vec::new()),
            ScriptedGate::new(),
            ScriptedContext::new(),
            transcript,
            |_| {},
        );
        let mut state = AgentState::default();
        let (outcome, updates, usage) = harness
            .run(
                &mut state,
                input("baseline-resume", "continue"),
                CancelToken::new(),
            )
            .await;
        let updates = append_driver_update(updates, AgentUpdate::TaskComplete("continued".into()));
        let actual = normalize_run(
            &state,
            &usage.records(),
            &updates,
            &outcome.stop,
            "retained-provider-state",
        );
        assert_matches_fixture("resumed_sqlite_history", &actual);
    }
}
