use std::path::PathBuf;

use tact::store::ArchiveFilter;
use tact_catalog::{CommandArguments, CommandId, SettingId};

use crate::{InspectorTab, PermissionModeView, ProjectId, RowId, SessionEpoch, SessionId};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DesktopIntent {
    OpenProject {
        root: PathBuf,
    },
    CloseProject {
        project_id: ProjectId,
    },
    NewSession,
    ResumeSession {
        session_id: SessionId,
    },
    ResumeLast,
    Submit(Submission),
    Queue(Submission),
    EditQueued {
        index: usize,
        text: String,
    },
    RemoveQueued {
        index: usize,
    },
    MoveQueued {
        from: usize,
        to: usize,
    },
    SendQueuedNow {
        index: usize,
    },
    CancelRun,
    RespondGate {
        session_epoch: SessionEpoch,
        request_id: u64,
        response: GateResponse,
    },
    SetGateDraft {
        session_epoch: SessionEpoch,
        request_id: u64,
        selected: Vec<usize>,
    },
    InvokeCommand {
        id: CommandId,
        args: CommandArguments,
    },
    RenameSession {
        session_id: SessionId,
        title: String,
    },
    SetSessionFilter(ArchiveFilter),
    SetSessionQuery(String),
    SetSessionArchived {
        session_id: SessionId,
        archived: bool,
    },
    RefreshSessions,
    SetInspector(InspectorTab),
    ToggleGroup {
        id: RowId,
    },
    ToggleTool {
        group: RowId,
        tool: RowId,
    },
    OpenReader(ReaderRef),
    SetTailFollowing(bool),
    BackToBottom,
    CancelSubagent {
        child_id: String,
    },
    CancelBackgroundTask {
        task_id: String,
    },
    SetSkillEnabled {
        name: String,
        enabled: bool,
    },
    SetSetting {
        id: SettingId,
        value: SettingValue,
    },
    RetryProvider,
    RestartSession,
    Shutdown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Submission {
    pub text: String,
    pub attachments: Vec<PathBuf>,
    pub model: Option<String>,
    pub permission_mode: Option<PermissionModeView>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateResponse {
    Select(Option<usize>),
    MultiSelect(Option<Vec<usize>>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingValue {
    Bool(bool),
    Text(String),
    Choice(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReaderRef {
    ToolOutput(RowId),
    Diff(RowId),
    Thinking(RowId),
    SystemPrompt,
    Stats,
    TasksDag,
    History,
    Help,
    Balance,
    Subagent(String),
    Background(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReaderRequest {
    pub title: std::sync::Arc<str>,
    pub markdown: std::sync::Arc<str>,
}
