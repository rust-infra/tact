//! Recovery and context-compaction scenarios for the agent harness.

mod harness;

use harness::{
    bash_tool_use, read_file_tool_use, run_single_task_with_config, task_completed_with, text_block,
};
use tact::permission::PermissionMode;
use tact::tool::test_support::write_workspace_file;
use tact_llm::StopReason;
use tact_llm::{ContentBlock, LlmError, MessageContent, MockClient, ProviderKind};
use tact_protocol::{AgentUpdate, TokenUsageInfo};

fn error_contains(updates: &[AgentUpdate], needle: &str) -> bool {
    updates.iter().any(
        |update| matches!(update, AgentUpdate::Error(error) if error.to_string().contains(needle)),
    )
}

fn tiny_context_config() -> tact::config::ResolvedConfig {
    tact::config::ResolvedConfig {
        llm: tact::config::LlmSettings {
            provider: ProviderKind::OpenAi,
            api_key: String::new(),
            base_url: String::new(),
            model: "mock-model".to_string(),
            models: Vec::new(),
        },
        agent: tact::config::AgentSettings {
            model_context_window: 100_000,
            max_tokens: 8192,
            thinking_budget: 0,
            snapshot_max_items: 80,
            notifications_enabled: false,
            micro_compact_enabled: true,
            skill_body_auto_inject: false,
            instruction_sources: tact::config::InstructionSources::default(),
        },
        ui: tact::config::UiSettings {
            theme: "retro".to_string(),
            vision_image: tact::config::VisionImageSettings {
                compress: tact::config::VisionImageSettings::DEFAULT_COMPRESS,
                max_edge: tact::config::VisionImageSettings::DEFAULT_MAX_EDGE,
                jpeg_quality: tact::config::VisionImageSettings::DEFAULT_JPEG_QUALITY,
            },
        },
        tools: tact::config::ToolSettings {
            brave_search_api_key: None,
            bash_timeout_secs: tact::config::ToolSettings::DEFAULT_BASH_TIMEOUT_SECS,
        },
        permission_mode: None,
        tokio_console: false,
        config_path: None,
    }
}

#[tokio::test]
async fn context_limit_triggers_auto_compact() {
    let big_content = "x".repeat(3000);

    let mock = MockClient::with_usage(vec![
        (
            vec![read_file_tool_use("read1", "big.txt")],
            Some(StopReason::ToolUse),
            TokenUsageInfo {
                total: 85_000,
                ..TokenUsageInfo::default()
            },
        ),
        (
            // This turn is consumed by compact_history's create_message call.
            vec![text_block("Summary of previous conversation.")],
            Some(StopReason::EndTurn),
            TokenUsageInfo::default(),
        ),
        (
            vec![text_block("Done after compact.")],
            Some(StopReason::EndTurn),
            TokenUsageInfo::default(),
        ),
    ]);

    let (updates, work_dir) = run_single_task_with_config(
        mock,
        "read big file",
        PermissionMode::Auto,
        tiny_context_config(),
        |dir| write_workspace_file(dir, "big.txt", &big_content),
    )
    .await;

    assert!(
        updates
            .iter()
            .any(|u| matches!(u, AgentUpdate::Info(msg) if msg.contains("[auto compact]"))),
        "expected auto compact info, got: {updates:?}"
    );
    assert!(
        updates
            .iter()
            .any(|u| matches!(u, AgentUpdate::Info(msg) if msg.contains("[transcript saved"))),
        "expected transcript saved info, got: {updates:?}"
    );
    assert!(task_completed_with(&updates, "Done after compact"));

    // Transcript should have been written under .claude/transcripts.
    let transcript_dir = work_dir.join(".claude").join("transcripts");
    assert!(
        transcript_dir
            .read_dir()
            .map(|mut d| d.next().is_some())
            .unwrap_or(false),
        "transcript file should be persisted"
    );
}

#[tokio::test]
async fn prompt_too_long_recovery_compacts_and_retries() {
    let mock = MockClient::with_responder(move |request, idx| {
        match idx {
            0 => Err(LlmError::Other("prompt is too long".to_string())),
            // compact_history's create_message consumes this turn.
            1 => Ok((
                vec![text_block("Compacted summary.")],
                Some(StopReason::EndTurn),
                None,
            )),
            // Retry after compaction.
            _ => {
                assert!(
                    request.messages.iter().any(|message| matches!(
                        &message.content,
                        MessageContent::Blocks { content }
                            if content.iter().any(|block| matches!(
                                block,
                                ContentBlock::Text { text } if text == "recover"
                            ))
                    )),
                    "compacted request should retain the UI block prompt: {:?}",
                    request.messages
                );
                Ok((
                    vec![text_block("Recovered from long prompt.")],
                    Some(StopReason::EndTurn),
                    None,
                ))
            }
        }
    });

    let mut config = tiny_context_config();
    config.agent.model_context_window = 200_000;
    let (updates, _work_dir) =
        run_single_task_with_config(mock, "recover", PermissionMode::Auto, config, |_| {}).await;

    assert!(
        updates.iter().any(|u| matches!(u, AgentUpdate::Info(msg) if msg.contains("[Recovery]") && msg.contains("compact"))),
        "expected compact recovery info, got: {updates:?}"
    );
    assert!(task_completed_with(&updates, "Recovered from long prompt"));
}

#[tokio::test]
async fn compact_summary_retries_transient_transport_error() {
    let mock = MockClient::with_responder(|_request, idx| match idx {
        0 => Err(LlmError::Other("prompt is too long".to_string())),
        1 => Err(LlmError::Other(
            "service temporarily unavailable".to_string(),
        )),
        2 => Ok((
            vec![text_block("Summary after retry.")],
            Some(StopReason::EndTurn),
            None,
        )),
        _ => Ok((
            vec![text_block("Recovered after summary retry.")],
            Some(StopReason::EndTurn),
            None,
        )),
    });
    let mut config = tiny_context_config();
    config.agent.model_context_window = 200_000;
    let (updates, _) =
        run_single_task_with_config(mock, "recover", PermissionMode::Auto, config, |_| {}).await;

    assert!(updates.iter().any(
        |update| matches!(update, AgentUpdate::Info(message) if message.contains("compact retry"))
    ));
    assert!(task_completed_with(
        &updates,
        "Recovered after summary retry"
    ));
}

#[tokio::test]
async fn compact_summary_rejects_empty_text_response() {
    let mock = MockClient::with_responder(|_request, idx| match idx {
        0 => Err(LlmError::Other("prompt is too long".to_string())),
        _ => Ok((Vec::new(), Some(StopReason::EndTurn), None)),
    });
    let mut config = tiny_context_config();
    config.agent.model_context_window = 200_000;
    let (updates, _) =
        run_single_task_with_config(mock, "recover", PermissionMode::Auto, config, |_| {}).await;

    assert!(error_contains(
        &updates,
        "summary response contained no text"
    ));
    assert!(
        !updates
            .iter()
            .any(|update| matches!(update, AgentUpdate::TaskComplete(_)))
    );
}

#[tokio::test]
async fn compact_summary_rejects_truncated_response() {
    let mock = MockClient::with_responder(|_request, idx| match idx {
        0 => Err(LlmError::Other("prompt is too long".to_string())),
        _ => Ok((
            vec![text_block("partial summary")],
            Some(StopReason::MaxTokens),
            None,
        )),
    });
    let mut config = tiny_context_config();
    config.agent.model_context_window = 200_000;
    let (updates, _) =
        run_single_task_with_config(mock, "recover", PermissionMode::Auto, config, |_| {}).await;

    assert!(error_contains(&updates, "invalid stop reason: MaxTokens"));
}

#[tokio::test]
async fn compact_summary_request_is_window_aware_for_oversized_turn() {
    let task = "x".repeat(100_000);
    let mock = MockClient::with_responder(|request, idx| match idx {
        0 => Err(LlmError::Other("prompt is too long".to_string())),
        1 => {
            let prompt = serde_json::to_string(&request.messages).unwrap();
            assert_eq!(request.max_tokens, 2_000);
            assert!(
                prompt.chars().count() < 100_000,
                "summary prompt was not bounded"
            );
            Ok((
                vec![text_block("Bounded summary.")],
                Some(StopReason::EndTurn),
                None,
            ))
        }
        _ => Ok((
            vec![text_block("Recovered from oversized turn.")],
            Some(StopReason::EndTurn),
            None,
        )),
    });
    let mut config = tiny_context_config();
    config.agent.model_context_window = 35_000;
    config.agent.max_tokens = 2_000;
    let (updates, _) =
        run_single_task_with_config(mock, &task, PermissionMode::Auto, config, |_| {}).await;

    assert!(task_completed_with(
        &updates,
        "Recovered from oversized turn"
    ));
}

#[tokio::test]
async fn max_tokens_with_pending_tools_executes_then_continues() {
    let mock = MockClient::new(vec![
        (
            // Simulate truncation mid-tool-call: the model emitted a tool use but hit max_tokens.
            vec![bash_tool_use("bash1", "echo ok")],
            Some(StopReason::MaxTokens),
        ),
        (
            vec![text_block("Continued after max_tokens.")],
            Some(StopReason::EndTurn),
        ),
    ]);

    let (updates, _work_dir) = run_single_task_with_config(
        mock,
        "truncated tool",
        PermissionMode::Auto,
        tiny_context_config(),
        |_| {},
    )
    .await;

    assert!(
        updates
            .iter()
            .any(|u| matches!(u, AgentUpdate::StepFinished { tool_id: id, .. } if id == "bash1")),
        "pending bash tool should still execute, got: {updates:?}"
    );
    assert!(
        updates.iter().any(|u| matches!(u, AgentUpdate::Info(msg) if msg.contains("[Recovery]") && msg.contains("continue"))),
        "expected continuation recovery info, got: {updates:?}"
    );
    assert!(task_completed_with(&updates, "Continued after max_tokens"));
}
