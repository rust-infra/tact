use std::path::PathBuf;
use std::sync::Arc;

use tact_protocol::{
    BackgroundTaskStatusSnapshot, GoalSnapshot, SubagentStatusSnapshot, TaskStatusSnapshot,
    UiRequestOrigin, UiRequestPurpose,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RowId(pub(crate) u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LoopId(pub(crate) u64);

impl LoopId {
    pub const fn value(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RequestId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct SessionEpoch(pub u64);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TranscriptRow {
    pub id: RowId,
    pub ordinal: u64,
    pub loop_id: Option<LoopId>,
    pub body: RowBody,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RowBody {
    User(UserRow),
    Assistant(AssistantRow),
    Thinking(ThinkingRow),
    ToolGroup(ToolGroupRow),
    Diff(DiffRow),
    Gate(GateCard),
    Notice(NoticeRow),
    HookContext(HookContextRow),
    HookStatus(HookStatusRow),
    Error(ErrorRow),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowStatus {
    Pending,
    Running,
    Success,
    Failed,
    Cancelled,
    Resolved,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserRow {
    pub text: Arc<str>,
    pub attachments: Arc<[PathBuf]>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssistantRow {
    pub markdown: Arc<str>,
    pub status: RowStatus,
    pub suggestions: Arc<[Arc<str>]>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThinkingRow {
    pub markdown: Arc<str>,
    pub status: RowStatus,
    pub duration_ms: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolGroupRow {
    pub loop_id: LoopId,
    pub items: Arc<[ToolGroupItem]>,
    pub summary: Arc<str>,
    pub status: RowStatus,
    pub collapsed: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[allow(clippy::large_enum_variant)]
pub enum ToolGroupItem {
    Tool(ToolRow),
    Gate(GateCard),
}

impl ToolGroupItem {
    pub fn id(&self) -> RowId {
        match self {
            Self::Tool(tool) => tool.id,
            Self::Gate(gate) => gate.id,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolRow {
    pub id: RowId,
    pub tool_id: Arc<str>,
    pub name: Arc<str>,
    pub arg_summary: Arc<str>,
    pub arg_full: Option<Arc<str>>,
    pub status: RowStatus,
    pub body: ToolBody,
    pub output: ToolOutputView,
    pub meta: ToolMetaView,
    pub subagent: Option<TranscriptView>,
    pub expanded: bool,
    pub keep_live: bool,
    pub destructive: bool,
    pub duration_ms: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolBody {
    None,
    Text(Arc<str>),
    Code {
        language: Option<Arc<str>>,
        text: Arc<str>,
    },
    Diff {
        path: Option<Arc<str>>,
        patch: Arc<str>,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ToolOutputView {
    pub preview: Arc<str>,
    pub detail: Option<Arc<str>>,
    pub truncated: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ToolMetaView {
    pub model: Option<Arc<str>>,
    pub token_usage: Option<UsageView>,
    pub context_window: Option<usize>,
    pub task_id: Option<Arc<str>>,
    pub subagent_id: Option<Arc<str>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffRow {
    pub path: Option<Arc<str>>,
    pub patch: Arc<str>,
    pub status: RowStatus,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GateCard {
    pub id: RowId,
    pub request_id: RequestId,
    pub session_epoch: SessionEpoch,
    pub purpose: UiRequestPurpose,
    pub kind: GateKind,
    pub prompt: Arc<str>,
    pub options: Arc<[Arc<str>]>,
    pub selection: GateSelection,
    pub origin: GateOrigin,
    pub status: GateState,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GateSelection {
    Single(Option<usize>),
    Multiple(Vec<usize>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GateOrigin {
    Main,
    Subagent {
        child_id: Arc<str>,
        tool_id: Option<Arc<str>>,
    },
    Tool {
        tool_id: Arc<str>,
    },
}

impl From<UiRequestOrigin> for GateOrigin {
    fn from(origin: UiRequestOrigin) -> Self {
        match origin {
            UiRequestOrigin {
                agent: tact_protocol::UiAgentRef::Subagent { child_id },
                tool_id,
            } => Self::Subagent {
                child_id: Arc::from(child_id),
                tool_id: tool_id.map(Arc::from),
            },
            UiRequestOrigin {
                agent: tact_protocol::UiAgentRef::Main,
                tool_id: Some(tool_id),
            } => Self::Tool {
                tool_id: Arc::from(tool_id),
            },
            UiRequestOrigin {
                agent: tact_protocol::UiAgentRef::Main,
                tool_id: None,
            } => Self::Main,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GateState {
    Pending,
    Submitting,
    Resolved,
    Cancelled,
    Withdrawn,
    Stale,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GateKind {
    Select,
    MultiSelect,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NoticeRow {
    pub text: Arc<str>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HookContextRow {
    pub source: Option<Arc<str>>,
    pub markdown: Arc<str>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HookStatusRow {
    pub hook_id: u64,
    pub source: Option<Arc<str>>,
    pub message: Arc<str>,
    pub elapsed_ms: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ErrorRow {
    pub message: Arc<str>,
    pub retryable: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TaskView {
    pub id: u64,
    pub subject: Arc<str>,
    pub status: TaskStatusSnapshot,
    pub owner: Arc<str>,
    pub blocks: Arc<[u64]>,
    pub blocked_by: Arc<[u64]>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SubagentView {
    pub child_id: Arc<str>,
    pub status: SubagentStatusSnapshot,
    pub summary: Arc<str>,
    pub model: Option<Arc<str>>,
    pub context: UsageView,
    pub elapsed_ms: u64,
    pub last_activity: Option<Arc<str>>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UsageView {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
    pub cache_hit_tokens: u32,
    pub reasoning_tokens: u32,
    pub context_window: Option<usize>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GoalView {
    pub snapshot: Option<GoalSnapshot>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BackgroundTaskView {
    pub id: Arc<str>,
    pub command: Arc<str>,
    pub status: BackgroundTaskStatusSnapshot,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub output_tail: Arc<str>,
    pub output_path: Option<Arc<str>>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TranscriptView {
    pub(crate) rows: im::Vector<Arc<TranscriptRow>>,
}

impl TranscriptView {
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    pub fn get(&self, index: usize) -> Option<Arc<TranscriptRow>> {
        self.rows.get(index).cloned()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Arc<TranscriptRow>> {
        self.rows.iter()
    }
}
