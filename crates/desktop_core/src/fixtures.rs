//! Representative runtime data for the offscreen UI capture harness.
//!
//! The capture binary renders the real shell with these rows so every
//! transcript row kind and inspector panel can be reviewed without a live
//! model run. Nothing here is used by the product shell.

use std::sync::Arc;

use tact_protocol::{
    BackgroundTaskStatusSnapshot, SubagentStatusSnapshot, TaskStatusSnapshot, UiRequestPurpose,
};

use crate::snapshot::InspectorView;
use crate::transcript::{
    AssistantRow, BackgroundTaskView, DiffRow, ErrorRow, GateCard, GateKind, GateOrigin,
    GateSelection, GateState, HookContextRow, HookStatusRow, LoopId, NoticeRow, RequestId, RowBody,
    RowId, RowStatus, SessionEpoch, SubagentView, TaskView, ThinkingRow, ToolBody, ToolGroupItem,
    ToolGroupRow, ToolMetaView, ToolOutputView, ToolRow, TranscriptRow, TranscriptView, UsageView,
    UserRow,
};

const SESSION_EPOCH: u64 = 7;

fn row(id: u64, body: RowBody) -> TranscriptRow {
    TranscriptRow {
        id: RowId(id),
        ordinal: id,
        loop_id: Some(LoopId(1)),
        body,
    }
}

fn tool(
    id: u64,
    name: &str,
    summary: &str,
    status: RowStatus,
    body: ToolBody,
    preview: &str,
) -> ToolRow {
    ToolRow {
        id: RowId(id),
        tool_id: Arc::from(format!("tool-{id}")),
        name: Arc::from(name),
        arg_summary: Arc::from(summary),
        arg_full: Some(Arc::from(summary)),
        status,
        body,
        output: ToolOutputView {
            preview: Arc::from(preview),
            detail: None,
            truncated: false,
        },
        meta: ToolMetaView::default(),
        subagent: None,
        expanded: false,
        keep_live: false,
        destructive: false,
        duration_ms: Some(420),
    }
}

fn gate(id: u64, purpose: UiRequestPurpose, prompt: &str, options: &[&str]) -> GateCard {
    GateCard {
        id: RowId(id),
        request_id: RequestId(id),
        session_epoch: SessionEpoch(SESSION_EPOCH),
        purpose,
        kind: GateKind::Select,
        prompt: Arc::from(prompt),
        options: options.iter().map(|option| Arc::from(*option)).collect(),
        selection: GateSelection::Single(None),
        origin: GateOrigin::Main,
        status: GateState::Pending,
    }
}

/// A standalone permission gate, as the shell receives it in a snapshot.
pub fn demo_gate() -> GateCard {
    gate(
        12,
        UiRequestPurpose::Permission,
        "Allow Tact to run `cargo test --workspace`?",
        &["Allow once", "Allow for session", "Deny"],
    )
}

/// A transcript that exercises every row kind the shell can render.
pub fn demo_transcript() -> TranscriptView {
    let gate = gate(
        6,
        UiRequestPurpose::Permission,
        "Run `cargo test -p tact store::session_store`?",
        &["Allow once", "Allow for session", "Deny"],
    );
    let rows = vec![
        row(
            0,
            RowBody::User(UserRow {
                text: Arc::from(
                    "Add title and archive support to the session store, then show it in the manager.",
                ),
                attachments: Arc::from([]),
            }),
        ),
        row(
            1,
            RowBody::HookContext(HookContextRow {
                source: Some(Arc::from("plugin auto-memory-tact")),
                markdown: Arc::from(
                    "**Project:** tact\n\nNo active tasks or open decisions in this project.",
                ),
            }),
        ),
        row(
            2,
            RowBody::Thinking(ThinkingRow {
                markdown: Arc::from(
                    "The migration runs on every open, so probe `PRAGMA table_info` before each \
                     `ALTER TABLE`. A bare ALTER would fail the second launch.",
                ),
                status: RowStatus::Success,
                duration_ms: Some(1_800),
            }),
        ),
        row(
            7,
            RowBody::Assistant(AssistantRow {
                markdown: Arc::from(
                    "## Migration\n\nThe store now adds two nullable columns and backfills titles \
                     from the first real user prompt.\n\n```rust\nlet query = SessionListQuery {\n    \
                     root: Some(root),\n    filter: ArchiveFilter::Active,\n};\n```\n\n- Active rows \
                     hide archived sessions\n- `resume_last` skips archived rows",
                ),
                status: RowStatus::Success,
                suggestions: Arc::from([
                    Arc::from("Explain the migration"),
                    Arc::from("Write a test"),
                ]),
            }),
        ),
        row(
            3,
            RowBody::ToolGroup(ToolGroupRow {
                loop_id: LoopId(1),
                items: Arc::from([
                    ToolGroupItem::Tool(tool(
                        4,
                        "shell",
                        "cargo test -p tact store::session_store",
                        RowStatus::Success,
                        ToolBody::None,
                        "test result: ok. 8 passed; 0 failed",
                    )),
                    ToolGroupItem::Tool(tool(
                        5,
                        "edit",
                        "crates/tact/src/store/session_store/sqlite.rs",
                        RowStatus::Running,
                        ToolBody::Diff {
                            path: Some(Arc::from("crates/tact/src/store/session_store/sqlite.rs")),
                            patch: Arc::from(
                                "@@ -40,6 +40,9 @@\n+    add_nullable_column(&conn, \"sessions\", \"title\", \"TEXT\")?;\n+    add_nullable_column(&conn, \"sessions\", \"archived_at\", \"TIMESTAMP\")?;",
                            ),
                        },
                        "",
                    )),
                    ToolGroupItem::Gate(gate),
                ]),
                summary: Arc::from("shell · edit · permission"),
                status: RowStatus::Running,
                collapsed: false,
            }),
        ),
        row(
            8,
            RowBody::Diff(DiffRow {
                path: Some(Arc::from("crates/desktop_core/src/session.rs")),
                patch: Arc::from(
                    "@@ -12,3 +12,7 @@\n+    pub fn set_archived(&mut self, id: SessionId, archived: bool) {\n+        self.intent(DesktopIntent::SetSessionArchived { id, archived });\n+    }",
                ),
                status: RowStatus::Success,
            }),
        ),
        row(
            9,
            RowBody::Notice(NoticeRow {
                text: Arc::from("Context compacted: 12 messages summarized."),
            }),
        ),
        row(
            10,
            RowBody::HookStatus(HookStatusRow {
                hook_id: 1,
                source: Some(Arc::from("cargo fmt --check")),
                message: Arc::from("cargo fmt --check completed"),
                elapsed_ms: Some(640),
            }),
        ),
        row(
            11,
            RowBody::Error(ErrorRow {
                message: Arc::from("Provider returned 429: rate limit exceeded"),
                retryable: true,
            }),
        ),
        row(12, RowBody::Gate(demo_gate())),
    ];
    TranscriptView {
        rows: rows.into_iter().map(Arc::new).collect(),
    }
}

/// Inspector panels populated with goals-free live data.
pub fn demo_inspector() -> InspectorView {
    InspectorView {
        tab: crate::snapshot::InspectorTab::Run,
        goals: Arc::from([]),
        tasks: Arc::from([
            TaskView {
                id: 1,
                subject: Arc::from("Add the schema migration"),
                status: TaskStatusSnapshot::Completed,
                owner: Arc::from("main"),
                blocks: Arc::from([2]),
                blocked_by: Arc::from([]),
            },
            TaskView {
                id: 2,
                subject: Arc::from("Filter the session list"),
                status: TaskStatusSnapshot::InProgress,
                owner: Arc::from("main"),
                blocks: Arc::from([]),
                blocked_by: Arc::from([1]),
            },
            TaskView {
                id: 3,
                subject: Arc::from("Cover archive and restore"),
                status: TaskStatusSnapshot::Pending,
                owner: Arc::from("main"),
                blocks: Arc::from([]),
                blocked_by: Arc::from([2]),
            },
        ]),
        subagents: Arc::from([
            SubagentView {
                child_id: Arc::from("child-1"),
                status: SubagentStatusSnapshot::Completed,
                summary: Arc::from("Reviewed the migration for idempotence"),
                model: Some(Arc::from("deepseek-v4.1-flash")),
                context: UsageView {
                    prompt_tokens: 8_200,
                    completion_tokens: 1_400,
                    total_tokens: 9_600,
                    cache_hit_tokens: 6_100,
                    reasoning_tokens: 900,
                    context_window: Some(128_000),
                },
                elapsed_ms: 42_000,
                last_activity: Some(Arc::from("Reported no blocking issues")),
            },
            SubagentView {
                child_id: Arc::from("child-2"),
                status: SubagentStatusSnapshot::Running,
                summary: Arc::from("Writing the archive filter tests"),
                model: Some(Arc::from("deepseek-v4.1-flash")),
                context: UsageView {
                    prompt_tokens: 3_100,
                    completion_tokens: 600,
                    total_tokens: 3_700,
                    cache_hit_tokens: 2_000,
                    reasoning_tokens: 200,
                    context_window: Some(128_000),
                },
                elapsed_ms: 12_000,
                last_activity: Some(Arc::from("cargo test -p tact store")),
            },
        ]),
        background: Arc::from([BackgroundTaskView {
            id: Arc::from("bg-1"),
            command: Arc::from("cargo test -p tact store::session_store"),
            status: BackgroundTaskStatusSnapshot::Running,
            started_at: 0,
            finished_at: None,
            output_tail: Arc::from("running 8 tests..."),
            output_path: Some(Arc::from("/tmp/tact-bg-1.log")),
        }]),
        context: UsageView {
            prompt_tokens: 24_000,
            completion_tokens: 4_200,
            total_tokens: 28_200,
            cache_hit_tokens: 18_000,
            reasoning_tokens: 2_100,
            context_window: Some(128_000),
        },
    }
}
