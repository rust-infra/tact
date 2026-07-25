//! Subagent simulation: end-to-end exercise of child session creation, tagged UI
//! forwarding, sticky-pane rendering, and cascade delete — without requiring a
//! real LLM for the child agent.

mod harness;

use tact::store::open_sqlite_session_store;
use tact_llm::{Message, MockClient, Role, StopReason};
use tact_protocol::{AgentUpdate, StepResult, StepStatus, TokenUsageInfo};
use tact_ui::test_support::install_test_config;
use tui::test_support::TestApp;

const SUBAGENT_SUMMARY: &str = "Fixed the test assertion; all tests pass now.";

// ---------------------------------------------------------------------------
// Test 1: Parent agent with a MockClient — basic tool execution works
// ---------------------------------------------------------------------------

#[tokio::test]
async fn parent_agent_with_session_and_ui() {
    install_test_config();

    let mock = MockClient::new(vec![
        (
            vec![harness::bash_tool_use("b1", "echo 'hello from parent'")],
            Some(StopReason::ToolUse),
        ),
        (
            vec![harness::text_block("Parent done.")],
            Some(StopReason::EndTurn),
        ),
    ]);

    let (ui_tx, mut ui_rx) = tokio::sync::mpsc::unbounded_channel();

    let (mut agent, _work_dir, _store, _parent_id) =
        tact_ui::test_support::build_test_agent_with_session(mock, Some(ui_tx)).await;

    agent
        .agent_loop(Some(Message::new_text(
            Role::User,
            "Run echo and report back.",
        )))
        .await
        .expect("parent agent_loop");

    let mut updates = Vec::new();
    while let Ok(u) = ui_rx.try_recv() {
        updates.push(u);
    }

    let bash_finished = updates
        .iter()
        .any(|u| matches!(u, AgentUpdate::StepFinished { result, .. } if result.tool == "bash"));
    assert!(bash_finished, "parent should have executed bash tool");

    eprintln!("═══ Parent agent simulation ═══");
    eprintln!("✓ Parent agent ran bash tool successfully");
    eprintln!("✓ UI channel received {} updates", updates.len());
    eprintln!();
}

// ---------------------------------------------------------------------------
// Test 2: Subagent session persistence (sessions.ref_id)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn subagent_session_persistence_simulation() {
    install_test_config();
    let tmp = tempfile::TempDir::new().unwrap();
    let db_path = tmp.path().join("tact.db");
    let store = open_sqlite_session_store(&db_path).await.unwrap();

    // ── Step 1: Create parent session ──────────────────────────────────
    let parent_id = "parent-session-001".to_string();
    let root_dir = tmp.path().display().to_string();
    store
        .ensure_session_row(&parent_id, &root_dir, "")
        .await
        .unwrap();
    eprintln!("[1/6] ✓ Parent session '{parent_id}' created (ref_id = '')");

    // ── Step 2: Create child (subagent) session with ref_id ────────────
    let child_id = uuid::Uuid::new_v4().to_string();
    store
        .ensure_session_row(&child_id, &root_dir, &parent_id)
        .await
        .unwrap();
    eprintln!(
        "[2/6] ✓ Child session '{}…' created (ref_id = '{parent_id}')",
        &child_id[..8]
    );

    // ── Step 3: Write messages + token usage to child ──────────────────
    store
        .append_message(
            &child_id,
            Role::User,
            &tact_llm::MessageContent::Text {
                content: "Fix the bug".to_string(),
            },
            0,
        )
        .await
        .unwrap();
    store
        .append_message(
            &child_id,
            Role::Assistant,
            &tact_llm::MessageContent::Text {
                content: SUBAGENT_SUMMARY.to_string(),
            },
            1,
        )
        .await
        .unwrap();
    // record_token_usage requires first_message_id and last_message_id
    // (the app assigns these from the assistant message rowid)
    store
        .record_token_usage(
            &child_id,
            "subagent_llm_call",
            Some(&TokenUsageInfo {
                prompt: 500,
                completion: 120,
                total: 620,
                prompt_cache_hit_tokens: 200,
                prompt_cache_miss_tokens: 300,
                reasoning_tokens: 10,
            }),
            1,         // first_message_id
            2,         // last_message_id
            Some(&[]), // request_body (column is NOT NULL)
        )
        .await
        .unwrap();
    eprintln!("[3/6] ✓ Child session: 2 messages + token_usage stored");

    // ── Step 4: list_sessions hides child ──────────────────────────────
    let listed = store.list_sessions(None).await.unwrap();
    assert_eq!(listed.len(), 1, "list_sessions should only show top-level");
    assert_eq!(listed[0].id, parent_id);
    eprintln!("[4/6] ✓ list_sessions hides child (only parent visible)");

    // ── Step 5: Child loads independently by id ────────────────────────
    let messages = store.load_session(&child_id).await.unwrap();
    assert_eq!(messages.len(), 2, "child session should have 2 messages");
    let text = tact::extract_text(&messages.last().unwrap().content);
    assert_eq!(text, SUBAGENT_SUMMARY);
    eprintln!("[5/6] ✓ Child session loads by id with correct summary");

    // ── Step 6: Delete parent cascades children ───────────────────────
    store.delete_session(&parent_id).await.unwrap();

    // All sessions should be gone (parent + child)
    let remaining = store.list_sessions(None).await.unwrap();
    assert!(
        remaining.is_empty(),
        "delete_session should cascade: no sessions left"
    );

    // Child messages should be gone too
    let child_msgs = store.load_session(&child_id).await.unwrap();
    assert!(child_msgs.is_empty(), "child messages deleted too");
    eprintln!("[6/6] ✓ delete_parent cascades: child + messages removed");

    eprintln!();
    eprintln!("═══ Subagent session persistence: ALL CHECKS PASSED ═══");
    eprintln!();
}

// ---------------------------------------------------------------------------
// Test 3: Tagged UI channel forwarding
// ---------------------------------------------------------------------------

#[tokio::test]
async fn subagent_tagged_ui_forwards_correctly() {
    install_test_config();

    let (parent_tx, mut parent_rx) = tokio::sync::mpsc::unbounded_channel();

    let tool_id = "task-abc123";
    let child_sess_id = uuid::Uuid::new_v4().to_string();
    let tagged_tx = tact::tool::subagent_ui::tagged_ui_channel(
        parent_tx,
        tool_id.to_string(),
        child_sess_id.clone(),
    );

    // Emit events a subagent would send
    tagged_tx
        .send(AgentUpdate::StreamChunk("Working on it\n".into()))
        .unwrap();
    tagged_tx
        .send(AgentUpdate::ThinkingChunk(
            tact_protocol::ThinkingChunk::Started,
        ))
        .unwrap();
    tagged_tx
        .send(AgentUpdate::ThinkingChunk(
            tact_protocol::ThinkingChunk::Delta("Let me check the code…".into()),
        ))
        .unwrap();
    tagged_tx
        .send(AgentUpdate::ThinkingChunk(
            tact_protocol::ThinkingChunk::Finished,
        ))
        .unwrap();
    tagged_tx
        .send(AgentUpdate::StepStarted {
            idx: 0,
            tool_id: "b1".into(),
            tool_name: "read_file".into(),
            arg_summary: "main.rs".into(),
            arg_full: "main.rs".into(),
        })
        .unwrap();
    tagged_tx
        .send(AgentUpdate::TokenUsage(TokenUsageInfo {
            prompt: 200,
            completion: 50,
            total: 250,
            ..Default::default()
        }))
        .unwrap();
    tagged_tx
        .send(AgentUpdate::StepFinished {
            idx: 0,
            tool_id: "b1".into(),
            result: StepResult {
                tool: "read_file".into(),
                arg_summary: "main.rs".into(),
                arg_full: None,
                status: StepStatus::Success,
                message: "Read 42 lines".into(),
                detail: None,
                duration_us: None,
                permission_label: None,
            },
        })
        .unwrap();

    // Permission popup passthrough
    let (respond, _) = tokio::sync::oneshot::channel();
    tagged_tx
        .send(AgentUpdate::RequestSelect {
            prompt: "Allow read_file?".into(),
            options: vec!["Allow once".into()],
            respond,
            log_confirm: false,
        })
        .unwrap();
    // TaskComplete → dropped
    tagged_tx
        .send(AgentUpdate::TaskComplete("sub done".into()))
        .unwrap();

    tokio::task::yield_now().await;

    let mut received = Vec::new();
    while let Ok(u) = parent_rx.try_recv() {
        received.push(u);
    }

    let total_tagged = received
        .iter()
        .filter(|u| matches!(u, AgentUpdate::Subagent { .. }))
        .count();
    let total_passthrough = received
        .iter()
        .filter(|u| matches!(u, AgentUpdate::RequestSelect { .. }))
        .count();
    let total_dropped = received
        .iter()
        .filter(|u| matches!(u, AgentUpdate::TaskComplete(_)))
        .count();

    eprintln!("═══ Tagged UI channel simulation ═══");
    eprintln!("  Tagged (Subagent wrapper)  : {total_tagged} updates");
    eprintln!("  Passthrough (RequestSelect): {total_passthrough} updates");
    eprintln!("  Dropped (TaskComplete)     : {total_dropped} updates");

    assert_eq!(
        total_tagged, 7,
        "all stream/thinking/step/token events should be tagged"
    );
    assert_eq!(
        total_passthrough, 1,
        "RequestSelect should pass through unchanged"
    );
    assert_eq!(total_dropped, 0, "TaskComplete should be dropped");

    let tagged_updates: Vec<&AgentUpdate> = received
        .iter()
        .filter(|u| matches!(u, AgentUpdate::Subagent { .. }))
        .collect();
    if let AgentUpdate::Subagent {
        parent_tool_id,
        session_id,
        update,
    } = tagged_updates.first().unwrap()
    {
        assert_eq!(parent_tool_id, tool_id);
        assert_eq!(session_id, &child_sess_id);
        assert!(matches!(update.as_ref(), AgentUpdate::StreamChunk(_)));
        eprintln!(
            "  ✓ Meta: parent_tool_id='{tool_id}', session_id='{}…'",
            &child_sess_id[..8]
        );
    }

    eprintln!();
    eprintln!("═══ Tagged UI simulation: ALL CHECKS PASSED ═══");
    eprintln!();
}

// ---------------------------------------------------------------------------
// Test 4: TUI sticky pane renders subagent mini-log
// ---------------------------------------------------------------------------

#[test]
fn subagent_sticky_pane_shows_mini_log() {
    let mut app = TestApp::new();

    // Feed Subagent updates: first run auto-switches to Subagent tab
    app.feed_all(vec![
        AgentUpdate::Subagent {
            parent_tool_id: "task-1".into(),
            session_id: "child-sess".into(),
            update: Box::new(AgentUpdate::StreamChunk("Analyzing module\n".into())),
        },
        AgentUpdate::Subagent {
            parent_tool_id: "task-1".into(),
            session_id: "child-sess".into(),
            update: Box::new(AgentUpdate::StepStarted {
                idx: 0,
                tool_id: "r1".into(),
                tool_name: "read_file".into(),
                arg_summary: "src/auth.rs".into(),
                arg_full: "src/auth.rs".into(),
            }),
        },
        AgentUpdate::Subagent {
            parent_tool_id: "task-1".into(),
            session_id: "child-sess".into(),
            update: Box::new(AgentUpdate::StepFinished {
                idx: 0,
                tool_id: "r1".into(),
                result: StepResult {
                    tool: "read_file".into(),
                    arg_summary: "src/auth.rs".into(),
                    arg_full: None,
                    status: StepStatus::Success,
                    message: "Read 120 lines".into(),
                    detail: None,
                    duration_us: None,
                    permission_label: None,
                },
            }),
        },
        AgentUpdate::Subagent {
            parent_tool_id: "task-1".into(),
            session_id: "child-sess".into(),
            update: Box::new(AgentUpdate::StreamChunk("Found the bug\n".into())),
        },
        AgentUpdate::Subagent {
            parent_tool_id: "task-1".into(),
            session_id: "child-sess".into(),
            update: Box::new(AgentUpdate::StepStarted {
                idx: 1,
                tool_id: "e1".into(),
                tool_name: "edit_file".into(),
                arg_summary: "src/auth.rs s/insecure/secure/".into(),
                arg_full: "src/auth.rs s/insecure/secure/".into(),
            }),
        },
        AgentUpdate::Subagent {
            parent_tool_id: "task-1".into(),
            session_id: "child-sess".into(),
            update: Box::new(AgentUpdate::StepFinished {
                idx: 1,
                tool_id: "e1".into(),
                result: StepResult {
                    tool: "edit_file".into(),
                    arg_summary: "src/auth.rs".into(),
                    arg_full: None,
                    status: StepStatus::Success,
                    message: "Applied edit".into(),
                    detail: None,
                    duration_us: None,
                    permission_label: None,
                },
            }),
        },
    ]);

    let output = app.render(100, 20);
    eprintln!("═══ Subagent sticky pane render ═══");
    eprintln!("{output}");
    eprintln!("════════════════════════════════════");

    // The sticky pane should be visible and show Subagent content
    assert!(
        output.contains("Subagent"),
        "Render should include Subagent tab label"
    );
    assert!(
        output.contains("Analyzing") || output.contains("module"),
        "Render should include stream chunks from subagent"
    );
    assert!(
        output.contains("read_file") || output.contains("edit_file"),
        "Render should include tool names"
    );
    assert!(
        output.contains("auth.rs"),
        "Render should include the file path from subagent steps"
    );
    eprintln!("✓ Subagent sticky pane renders correctly");
    eprintln!();
}
