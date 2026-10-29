use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tact_agent_api::{
    AgentContentBlock, CancelToken, RunContext, RunId, SessionRef, ToolExecutionContext,
    ToolOutputChunk, ToolProgressSink, ToolResolver,
};
use tact_protocol::ToolOutputChunk as HostToolOutputChunk;

use super::*;
use crate::{
    mcp::MCPToolRouter,
    permission::{PermissionManager, PermissionMode},
    tool::test_support::test_context,
    tool::{Tool, ToolCallResult, ToolMetadata},
};

struct NoopProgress;

impl ToolProgressSink for NoopProgress {
    fn emit(&self, _chunks: Vec<ToolOutputChunk>) {}
}

const SPLIT_SECRET_METADATA: ToolMetadata = ToolMetadata::read_json(
    "split_secret",
    "Emit a deliberately split provider token.",
    "Split Secret",
);

struct SplitSecretTool;

#[async_trait]
impl Tool for SplitSecretTool {
    fn metadata(&self) -> &'static ToolMetadata {
        &SPLIT_SECRET_METADATA
    }

    fn input_schema(&self) -> Value {
        json!({"type": "object", "properties": {}})
    }

    async fn call(
        &self,
        context: crate::tool::ToolContext,
        _input: Value,
    ) -> anyhow::Result<ToolCallResult> {
        const SECRET: &str = "sk-7325c3231cef402d8481c32a49c4898a";
        context
            .progress_reporter
            .report(vec![HostToolOutputChunk::stdout("prefix\nsk-7325c32")]);
        context
            .progress_reporter
            .report(vec![HostToolOutputChunk::stdout(
                "31cef402d8481c32a49c4898a\n",
            )]);
        context.progress_reporter.flush();
        Ok(ToolCallResult::text(SECRET))
    }
}

#[derive(Default)]
struct RecordingProgress(Mutex<Vec<ToolOutputChunk>>);

impl ToolProgressSink for RecordingProgress {
    fn emit(&self, chunks: Vec<ToolOutputChunk>) {
        self.0.lock().unwrap().extend(chunks);
    }
}

impl RecordingProgress {
    fn text(&self) -> String {
        self.0
            .lock()
            .unwrap()
            .iter()
            .map(|chunk| chunk.text.as_str())
            .collect()
    }
}

#[tokio::test]
async fn resolving_a_write_does_not_execute_it() {
    let context = test_context("kernel_tool_resolution_is_read_only");
    let target = context.work_dir.join("resolution-only.txt");
    let permissions = PermissionManager::try_new(PermissionMode::Default).unwrap();
    let port = TactToolPort::new(
        crate::tool::toolset(),
        McpCatalogHandle::new(MCPToolRouter::new()),
        context,
        permissions.security_config(),
        permissions.scanner(),
        false,
    );
    let call = ToolCall {
        id: "write-1".into(),
        name: "write_file".into(),
        input: json!({
            "path": "resolution-only.txt",
            "content": "execution requires a separate ToolPort call",
        }),
    };

    let resolved = port.resolve(&call).expect("resolve write_file");
    assert!(
        !target.exists(),
        "ToolResolver::resolve must not execute the tool"
    );

    port.execute(
        resolved,
        ToolExecutionContext {
            call_id: call.id,
            run: RunContext {
                session: SessionRef::new("test-session"),
                run_id: RunId::new("test-run"),
            },
            progress: Arc::new(NoopProgress),
            cancel: CancelToken::new(),
            remaining_turns: None,
        },
    )
    .await
    .expect("execute write_file");
    assert_eq!(
        std::fs::read_to_string(target).unwrap(),
        "execution requires a separate ToolPort call"
    );
}

#[test]
fn port_classifies_native_mcp_and_subagent_resource_scopes() {
    let context = test_context("kernel_tool_resource_scopes");
    let permissions = PermissionManager::try_new(PermissionMode::Default).unwrap();
    let port = TactToolPort::new(
        crate::tool::toolset(),
        McpCatalogHandle::new(MCPToolRouter::new()),
        context,
        permissions.security_config(),
        permissions.scanner(),
        false,
    );
    let call = |name: &str, input: Value| ToolCall {
        id: format!("{name}-1"),
        name: name.into(),
        input,
    };

    let read = call("read_file", json!({"path": "notes.md"}));
    let read_access = port.access(
        &port.resolve_kind(&read).expect("resolve read_file"),
        &read.input,
    );
    assert_eq!(read_access.reads.len(), 1);
    assert_eq!(read_access.reads[0].key.namespace, "path");
    assert!(read_access.reads[0].covers_descendants);
    assert!(!read_access.barrier);

    let write = call("write_file", json!({"path": "notes.md", "content": "x"}));
    let write_access = port.access(
        &port.resolve_kind(&write).expect("resolve write_file"),
        &write.input,
    );
    assert_eq!(write_access.writes.len(), 1);
    assert_eq!(write_access.writes[0].key.namespace, "path");
    assert!(!write_access.writes[0].covers_descendants);

    let bash = call("bash", json!({"command": "pwd"}));
    let bash_access = port.access(
        &port.resolve_kind(&bash).expect("resolve bash"),
        &bash.input,
    );
    assert!(bash_access.barrier);

    let mcp = ResolvedKind::Mcp {
        full_name: "mcp__postgres__query".into(),
        server: "postgres".into(),
        tool: "query".into(),
    };
    let mcp_access = port.access(&mcp, &json!({}));
    assert_eq!(mcp_access.writes.len(), 1);
    assert_eq!(mcp_access.writes[0].key.namespace, "mcp_server");
    assert_eq!(mcp_access.writes[0].key.components, ["postgres"]);

    let blocking_subagent = call("spawn_subagent", json!({"prompt": "work"}));
    let blocking_access = port.access(
        &port
            .resolve_kind(&blocking_subagent)
            .expect("resolve spawn_subagent"),
        &blocking_subagent.input,
    );
    assert!(blocking_access.barrier);

    let worktree_subagent = call(
        "spawn_subagent",
        json!({"prompt": "work", "worktree": true}),
    );
    let worktree_access = port.access(
        &port
            .resolve_kind(&worktree_subagent)
            .expect("resolve worktree subagent"),
        &worktree_subagent.input,
    );
    assert_eq!(worktree_access, ResourceAccess::independent());
}

#[tokio::test]
async fn tool_port_redacts_split_live_chunks_and_the_final_result() {
    const SECRET: &str = "sk-7325c3231cef402d8481c32a49c4898a";

    let context = test_context("kernel_tool_split_secret_redaction");
    let permissions = PermissionManager::try_new(PermissionMode::Auto).expect("permission manager");
    let port = TactToolPort::new(
        crate::tool::ToolRouter::new()
            .route(SplitSecretTool)
            .expect("register split-secret tool"),
        McpCatalogHandle::new(MCPToolRouter::new()),
        context,
        permissions.security_config(),
        permissions.scanner(),
        false,
    );
    let call = ToolCall {
        id: "split-secret-1".into(),
        name: SPLIT_SECRET_METADATA.name.into(),
        input: json!({}),
    };
    let resolved = port.resolve(&call).expect("resolve split-secret tool");
    let progress = Arc::new(RecordingProgress::default());
    let result = port
        .execute(
            resolved,
            ToolExecutionContext {
                call_id: call.id,
                run: RunContext {
                    session: SessionRef::new("test-session"),
                    run_id: RunId::new("test-run"),
                },
                progress: progress.clone(),
                cancel: CancelToken::new(),
                remaining_turns: None,
            },
        )
        .await
        .expect("execute split-secret tool");

    let live = progress.text();
    assert!(live.contains("[redacted:api-key]"), "{live}");
    assert!(
        !live.contains(SECRET),
        "live output leaked the token: {live}"
    );

    let final_text = result
        .outcome
        .content
        .iter()
        .filter_map(|block| match block {
            AgentContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<String>();
    assert!(final_text.contains("[redacted:api-key]"), "{final_text}");
    assert!(
        !final_text.contains(SECRET),
        "final output leaked the token: {final_text}"
    );
}
