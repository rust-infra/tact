//! Recovery and context-compaction scenarios for the agent harness.

mod harness;

use harness::{
    bash_tool_use, read_file_tool_use, run_single_task_with_config, task_completed_with, text_block,
};
use tact::permission::PermissionMode;
use tact::tool::test_support::write_workspace_file;
use tact_llm::StopReason;
use tact_llm::{LlmError, MockClient, ProviderKind};
use tact_protocol::AgentUpdate;

fn tiny_context_config() -> tact::config::ResolvedConfig {
    tact::config::ResolvedConfig {
        llm: tact::config::LlmSettings {
            provider: ProviderKind::OpenAi,
            api_key: String::new(),
            base_url: String::new(),
            model: "mock-model".to_string(),
        },
        agent: tact::config::AgentSettings {
            context_limit_chars: 500,
            max_tokens: 8192,
            thinking_budget: 0,
            snapshot_max_items: 80,
            notifications_enabled: false,
            micro_compact_enabled: true,
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
        },
        permission_mode: None,
        tokio_console: false,
    }
}

#[tokio::test]
async fn context_limit_triggers_auto_compact() {
    let big_content = "x".repeat(3000);

    let mock = MockClient::new(vec![
        (
            vec![read_file_tool_use("read1", "big.txt")],
            Some(StopReason::ToolUse),
        ),
        (
            // This turn is consumed by compact_history's create_message call.
            vec![text_block("Summary of previous conversation.")],
            Some(StopReason::EndTurn),
        ),
        (
            vec![text_block("Done after compact.")],
            Some(StopReason::EndTurn),
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
    let mock = MockClient::with_responder(move |_request, idx| {
        match idx {
            0 => Err(LlmError::Other("prompt is too long".to_string())),
            // compact_history's create_message consumes this turn.
            1 => Ok((
                vec![text_block("Compacted summary.")],
                Some(StopReason::EndTurn),
                None,
            )),
            // Retry after compaction.
            _ => Ok((
                vec![text_block("Recovered from long prompt.")],
                Some(StopReason::EndTurn),
                None,
            )),
        }
    });

    let (updates, _work_dir) = run_single_task_with_config(
        mock,
        "recover",
        PermissionMode::Auto,
        tiny_context_config(),
        |_| {},
    )
    .await;

    assert!(
        updates.iter().any(|u| matches!(u, AgentUpdate::Info(msg) if msg.contains("[Recovery]") && msg.contains("compact"))),
        "expected compact recovery info, got: {updates:?}"
    );
    assert!(task_completed_with(&updates, "Recovered from long prompt"));
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
