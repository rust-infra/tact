//! The shell's JSON view of the desktop snapshot, and the commands TSX sends
//! back. The Rust views use the typed snapshot directly; TSX gets this.

use serde::{Deserialize, Serialize};
use tact_desktop_core::{
    AccountView, BackgroundTaskView, ComposerView, DesktopSnapshot, ExtensionsView, GateCard,
    GateSelection, GoalView, InspectorView, PermissionModeView, ProjectView, RowBody, RowStatus,
    SessionListState, SessionSummaryView, SessionView, SettingsView, SubagentView, TaskView,
    ToolBody, ToolGroupItem, ToolGroupRow, ToolRow, TrajectoryView, TranscriptRow, UsageView,
};

#[derive(Serialize)]
pub struct ShellSnapshot {
    pub revision: u64,
    pub appearance: &'static str,
    pub language: String,
    pub text_scale: f32,
    pub tail_following: bool,
    pub show_hook_output: bool,
    pub notifications: bool,
    pub voice_enabled: bool,
    pub voice_missing_api_key: bool,
    pub project: ProjectJson,
    pub projects: Vec<ProjectJson>,
    pub sessions: SessionListJson,
    pub session: SessionJson,
    pub runtime: RuntimeJson,
    pub rows: Vec<RowJson>,
    pub composer: ComposerJson,
    pub inspector: InspectorJson,
    pub pending_gates: Vec<GateJson>,
    pub settings: SettingsJson,
    pub extensions: ExtensionsJson,
    pub surface: Option<SurfaceJson>,
    pub account: Option<AccountJson>,
    pub failure: Option<String>,
}

impl From<&DesktopSnapshot> for ShellSnapshot {
    fn from(snapshot: &DesktopSnapshot) -> Self {
        Self {
            revision: snapshot.revision,
            appearance: match snapshot.appearance {
                tact_desktop_core::Appearance::Light => "light",
                tact_desktop_core::Appearance::Dark => "dark",
            },
            language: match snapshot.language {
                tact_catalog::CatalogLocale::En => "en",
                tact_catalog::CatalogLocale::Zh => "zh",
            }
            .to_owned(),
            text_scale: snapshot.text_scale,
            tail_following: snapshot.tail_following,
            show_hook_output: snapshot.show_hook_output,
            notifications: snapshot.notifications,
            voice_enabled: snapshot.voice_enabled,
            voice_missing_api_key: snapshot.voice_missing_api_key,
            project: ProjectJson::from(&snapshot.project),
            projects: snapshot.projects.iter().map(ProjectJson::from).collect(),
            sessions: SessionListJson::from(&snapshot.sessions),
            session: SessionJson::from(&snapshot.session),
            runtime: RuntimeJson::from(&snapshot.runtime),
            rows: snapshot
                .transcript
                .iter()
                .map(|row| RowJson::from(row.as_ref()))
                .collect(),
            composer: ComposerJson::from(&snapshot.composer),
            inspector: InspectorJson::from(&snapshot.inspector),
            pending_gates: snapshot.pending_gates.iter().map(GateJson::from).collect(),
            settings: SettingsJson::from(&snapshot.settings),
            extensions: ExtensionsJson::from(&snapshot.extensions),
            surface: snapshot.surface.as_ref().map(|surface| {
                let (title, body) = crate::surface::render(surface);
                SurfaceJson { title, body }
            }),
            account: snapshot.account.as_ref().map(AccountJson::from),
            failure: snapshot
                .failure
                .as_ref()
                .map(|failure| failure.message.to_string()),
        }
    }
}

#[derive(Serialize)]
pub struct ProjectJson {
    pub id: Option<String>,
    pub root: Option<String>,
    pub name: String,
    pub branch: Option<String>,
    pub repository: Option<String>,
    pub available: bool,
}

impl From<&ProjectView> for ProjectJson {
    fn from(project: &ProjectView) -> Self {
        Self {
            id: project.id.as_ref().map(|id| id.0.clone()),
            root: project.root.as_ref().map(|root| root.display().to_string()),
            name: project.display_name.to_string(),
            branch: project.branch.as_deref().map(str::to_owned),
            repository: project.repository.as_deref().map(str::to_owned),
            available: project.available,
        }
    }
}

#[derive(Serialize)]
pub struct SessionJson {
    pub id: Option<String>,
    pub title: String,
    pub phase: String,
    pub locked: bool,
}

impl From<&SessionView> for SessionJson {
    fn from(session: &SessionView) -> Self {
        Self {
            id: session.id.as_ref().map(|id| id.0.clone()),
            title: session.title.to_string(),
            phase: session_phase(&session.phase).to_owned(),
            locked: session.locked,
        }
    }
}

fn session_phase(phase: &tact_desktop_core::SessionPhase) -> &'static str {
    use tact_desktop_core::SessionPhase;
    match phase {
        SessionPhase::Empty => "empty",
        SessionPhase::Opening => "opening",
        SessionPhase::Starting => "starting",
        SessionPhase::Ready => "ready",
        SessionPhase::Running => "running",
        SessionPhase::Switching => "switching",
        SessionPhase::Closing => "closing",
        SessionPhase::Disconnected { .. } => "disconnected",
        SessionPhase::Failed { .. } => "failed",
    }
}

#[derive(Serialize)]
pub struct SessionListJson {
    pub filter: String,
    pub query: String,
    pub selected: Option<String>,
    pub loading: bool,
    pub items: Vec<SessionSummaryJson>,
}

impl From<&SessionListState> for SessionListJson {
    fn from(state: &SessionListState) -> Self {
        Self {
            filter: match state.filter {
                tact_desktop_core::ArchiveFilter::Active => "active",
                tact_desktop_core::ArchiveFilter::Archived => "archived",
                tact_desktop_core::ArchiveFilter::All => "all",
            }
            .to_owned(),
            query: state.query.clone(),
            selected: state.selected.as_ref().map(|id| id.0.clone()),
            loading: state.loading,
            items: state.items.iter().map(SessionSummaryJson::from).collect(),
        }
    }
}

#[derive(Serialize)]
pub struct SessionSummaryJson {
    pub id: String,
    pub project_id: String,
    pub title: String,
    pub updated_at: String,
    pub archived: bool,
    pub locked: bool,
    pub message_count: i64,
}

impl From<&SessionSummaryView> for SessionSummaryJson {
    fn from(session: &SessionSummaryView) -> Self {
        Self {
            id: session.id.clone(),
            project_id: session.project_id.0.clone(),
            title: session.title.clone(),
            updated_at: session.updated_at.to_rfc3339(),
            archived: session.archived_at.is_some(),
            locked: session.locked,
            message_count: session.message_count,
        }
    }
}

#[derive(Serialize)]
pub struct RuntimeJson {
    pub phase: String,
    pub elapsed_seconds: i64,
    pub turns: u32,
    pub model: Option<String>,
    pub usage: UsageJson,
    pub latest_tool: Option<String>,
    pub latest_tool_row: Option<u64>,
}

impl From<&tact_desktop_core::RuntimeView> for RuntimeJson {
    fn from(runtime: &tact_desktop_core::RuntimeView) -> Self {
        Self {
            phase: run_phase(&runtime.current_phase).to_owned(),
            elapsed_seconds: runtime
                .run_started_at
                .map(|started| (chrono::Utc::now() - started).num_seconds().max(0))
                .unwrap_or(0),
            turns: runtime.turns_taken,
            model: runtime.model.as_deref().map(str::to_owned),
            usage: UsageJson::from(&runtime.usage),
            latest_tool: runtime.latest_tool.as_deref().map(str::to_owned),
            latest_tool_row: runtime
                .latest_tool_detail
                .as_ref()
                .and_then(|detail| detail.row)
                .map(|row| row.value()),
        }
    }
}

fn run_phase(phase: &tact_desktop_core::RunPhase) -> &'static str {
    use tact_desktop_core::RunPhase;
    match phase {
        RunPhase::Idle => "idle",
        RunPhase::Waiting => "waiting",
        RunPhase::Thinking => "thinking",
        RunPhase::Streaming => "streaming",
        RunPhase::RunningTools => "running_tools",
        RunPhase::WaitingForGate => "waiting_for_gate",
        RunPhase::Cancelling => "cancelling",
        RunPhase::Complete => "complete",
        RunPhase::Failed => "failed",
    }
}

#[derive(Serialize, Clone)]
pub struct UsageJson {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
    pub cache_hit_tokens: u32,
    pub reasoning_tokens: u32,
    pub context_window: Option<usize>,
}

impl From<&UsageView> for UsageJson {
    fn from(usage: &UsageView) -> Self {
        Self {
            prompt_tokens: usage.prompt_tokens,
            completion_tokens: usage.completion_tokens,
            total_tokens: usage.total_tokens,
            cache_hit_tokens: usage.cache_hit_tokens,
            reasoning_tokens: usage.reasoning_tokens,
            context_window: usage.context_window,
        }
    }
}

#[derive(Serialize)]
pub struct RowJson {
    pub id: u64,
    pub ordinal: u64,
    pub time: Option<String>,
    pub body: BodyJson,
}

impl From<&TranscriptRow> for RowJson {
    fn from(row: &TranscriptRow) -> Self {
        Self {
            id: row.id.value(),
            ordinal: row.ordinal,
            time: row.time.as_deref().map(str::to_owned),
            body: BodyJson::from(&row.body),
        }
    }
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BodyJson {
    User {
        text: String,
        attachments: Vec<String>,
    },
    Assistant {
        markdown: String,
        status: String,
        suggestions: Vec<String>,
    },
    Thinking {
        markdown: String,
        status: String,
        duration_ms: Option<u64>,
    },
    Plan {
        title: String,
        steps: Vec<PlanStepJson>,
        tool_execution: bool,
    },
    ToolGroup {
        summary: String,
        collapsed: bool,
        items: Vec<ToolItemJson>,
    },
    Diff {
        path: Option<String>,
        patch: String,
        status: String,
    },
    Gate {
        gate: GateJson,
    },
    Notice {
        text: String,
    },
    HookContext {
        source: Option<String>,
        markdown: String,
    },
    HookStatus {
        message: String,
    },
    Error {
        message: String,
        retryable: bool,
    },
}

impl From<&RowBody> for BodyJson {
    fn from(body: &RowBody) -> Self {
        match body {
            RowBody::User(user) => BodyJson::User {
                text: user.text.to_string(),
                attachments: user
                    .attachments
                    .iter()
                    .map(|path| {
                        path.file_name()
                            .map(|name| name.to_string_lossy().to_string())
                            .unwrap_or_else(|| path.display().to_string())
                    })
                    .collect(),
            },
            RowBody::Assistant(assistant) => BodyJson::Assistant {
                markdown: assistant.markdown.to_string(),
                status: row_status(assistant.status).to_owned(),
                suggestions: assistant
                    .suggestions
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
            },
            RowBody::Thinking(thinking) => BodyJson::Thinking {
                markdown: thinking.markdown.to_string(),
                status: row_status(thinking.status).to_owned(),
                duration_ms: thinking.duration_ms,
            },
            RowBody::Plan(plan) => BodyJson::Plan {
                title: plan.title.to_string(),
                steps: plan
                    .steps
                    .iter()
                    .map(|step| PlanStepJson {
                        label: step.label.to_string(),
                        status: row_status(step.status).to_owned(),
                    })
                    .collect(),
                tool_execution: plan.steps.iter().all(|step| step.tool_id.is_some()),
            },
            RowBody::ToolGroup(group) => BodyJson::ToolGroup {
                summary: group.summary.to_string(),
                collapsed: group.collapsed,
                items: tool_items(group),
            },
            RowBody::Diff(diff) => BodyJson::Diff {
                path: diff.path.as_deref().map(str::to_owned),
                patch: diff.patch.to_string(),
                status: row_status(diff.status).to_owned(),
            },
            RowBody::Gate(gate) => BodyJson::Gate {
                gate: GateJson::from(gate),
            },
            RowBody::Notice(notice) => BodyJson::Notice {
                text: notice.text.to_string(),
            },
            RowBody::HookContext(context) => BodyJson::HookContext {
                source: context.source.as_deref().map(str::to_owned),
                markdown: context.markdown.to_string(),
            },
            RowBody::HookStatus(status) => BodyJson::HookStatus {
                message: status.message.to_string(),
            },
            RowBody::Error(error) => BodyJson::Error {
                message: error.message.to_string(),
                retryable: error.retryable,
            },
        }
    }
}

#[derive(Serialize)]
pub struct PlanStepJson {
    pub label: String,
    pub status: String,
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ToolItemJson {
    Tool { tool: ToolJson },
    Gate { gate: GateJson },
}

fn tool_items(group: &ToolGroupRow) -> Vec<ToolItemJson> {
    group
        .items
        .iter()
        .map(|item| match item {
            ToolGroupItem::Tool(tool) => ToolItemJson::Tool {
                tool: ToolJson::from(tool),
            },
            ToolGroupItem::Gate(gate) => ToolItemJson::Gate {
                gate: GateJson::from(gate),
            },
        })
        .collect()
}

#[derive(Serialize)]
pub struct ToolJson {
    pub id: u64,
    pub tool_id: String,
    pub name: String,
    pub arg_summary: String,
    pub status: String,
    pub body: ToolBodyJson,
    pub output_preview: String,
    pub output_detail: Option<String>,
    pub expanded: bool,
    pub duration_ms: Option<u64>,
    pub model: Option<String>,
    pub task_id: Option<String>,
    pub subagent_id: Option<String>,
    pub subagent: Option<Vec<RowJson>>,
}

impl From<&ToolRow> for ToolJson {
    fn from(tool: &ToolRow) -> Self {
        Self {
            id: tool.id.value(),
            tool_id: tool.tool_id.to_string(),
            name: tool.name.to_string(),
            arg_summary: tool.arg_summary.to_string(),
            status: row_status(tool.status).to_owned(),
            body: ToolBodyJson::from(&tool.body),
            output_preview: tool.output.preview.to_string(),
            output_detail: tool.output.detail.as_deref().map(str::to_owned),
            expanded: tool.expanded,
            duration_ms: tool.duration_ms,
            model: tool.meta.model.as_deref().map(str::to_owned),
            task_id: tool.meta.task_id.as_deref().map(str::to_owned),
            subagent_id: tool.meta.subagent_id.as_deref().map(str::to_owned),
            subagent: tool
                .subagent
                .as_ref()
                .map(|view| view.iter().map(|row| RowJson::from(row.as_ref())).collect()),
        }
    }
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ToolBodyJson {
    None,
    Text {
        text: String,
    },
    Code {
        language: Option<String>,
        text: String,
    },
    Diff {
        path: Option<String>,
        patch: String,
    },
}

impl From<&ToolBody> for ToolBodyJson {
    fn from(body: &ToolBody) -> Self {
        match body {
            ToolBody::None => ToolBodyJson::None,
            ToolBody::Text(text) => ToolBodyJson::Text {
                text: text.to_string(),
            },
            ToolBody::Code { language, text } => ToolBodyJson::Code {
                language: language.as_deref().map(str::to_owned),
                text: text.to_string(),
            },
            ToolBody::Diff { path, patch } => ToolBodyJson::Diff {
                path: path.as_deref().map(str::to_owned),
                patch: patch.to_string(),
            },
        }
    }
}

#[derive(Serialize)]
pub struct GateJson {
    pub id: u64,
    pub session_epoch: u64,
    pub request_id: u64,
    pub kind: String,
    pub prompt: String,
    pub options: Vec<String>,
    pub selected: Vec<usize>,
    pub status: String,
}

impl From<&GateCard> for GateJson {
    fn from(gate: &GateCard) -> Self {
        let selected = match &gate.selection {
            GateSelection::Single(Some(index)) => vec![*index],
            GateSelection::Multiple(indices) => indices.clone(),
            _ => Vec::new(),
        };
        Self {
            id: gate.id.value(),
            session_epoch: gate.session_epoch.0,
            request_id: gate.request_id.0,
            kind: match gate.kind {
                tact_desktop_core::GateKind::Select => "select",
                tact_desktop_core::GateKind::MultiSelect => "multi_select",
            }
            .to_owned(),
            prompt: gate.prompt.to_string(),
            options: gate.options.iter().map(|o| o.to_string()).collect(),
            selected,
            status: gate_state(&gate.status).to_owned(),
        }
    }
}

fn gate_state(state: &tact_desktop_core::GateState) -> &'static str {
    use tact_desktop_core::GateState;
    match state {
        GateState::Pending => "pending",
        GateState::Submitting => "submitting",
        GateState::Resolved => "resolved",
        GateState::Cancelled => "cancelled",
        GateState::Withdrawn => "withdrawn",
        GateState::Stale => "stale",
    }
}

pub fn row_status(status: RowStatus) -> &'static str {
    match status {
        RowStatus::Pending => "pending",
        RowStatus::Running => "running",
        RowStatus::Success => "success",
        RowStatus::Failed => "failed",
        RowStatus::Cancelled => "cancelled",
        RowStatus::Unknown => "unknown",
        RowStatus::Resolved => "resolved",
    }
}

#[derive(Serialize)]
pub struct ComposerJson {
    pub commands: Vec<CommandJson>,
    pub model: String,
    pub models: Vec<String>,
    pub subagent_model: String,
    pub subagent_models: Vec<String>,
    pub effort: Option<String>,
    pub permission: String,
    pub thinking_budget: Option<u32>,
    pub queue: Vec<QueuedJson>,
    pub history: Vec<String>,
}

impl From<&ComposerView> for ComposerJson {
    fn from(composer: &ComposerView) -> Self {
        Self {
            commands: tact_catalog::builtins()
                .entries()
                .map(|entry| CommandJson {
                    path: entry.path.clone(),
                    title: entry.path.join(" "),
                    description: entry
                        .description
                        .get(tact_catalog::CatalogLocale::En)
                        .to_string(),
                    needs_arguments: !matches!(
                        tact_desktop_core::form_for(&entry.id),
                        tact_desktop_core::CommandForm::None
                    ),
                })
                .collect(),
            model: composer.model.to_string(),
            models: composer.models.iter().map(|m| m.to_string()).collect(),
            subagent_model: composer.subagent_model.to_string(),
            subagent_models: composer
                .subagent_models
                .iter()
                .map(|m| m.to_string())
                .collect(),
            effort: composer.effort.as_deref().map(str::to_owned),
            permission: match composer.permission_mode {
                PermissionModeView::Default => "default",
                PermissionModeView::Plan => "plan",
                PermissionModeView::Auto => "auto",
            }
            .to_owned(),
            thinking_budget: composer.thinking_budget,
            queue: composer
                .queue
                .iter()
                .enumerate()
                .map(|(index, item)| QueuedJson {
                    index,
                    text: item.text.to_string(),
                })
                .collect(),
            history: composer.history.iter().map(|h| h.to_string()).collect(),
        }
    }
}

#[derive(Serialize)]
pub struct CommandJson {
    pub path: Vec<String>,
    pub title: String,
    pub description: String,
    /// The command asks for an argument the command center cannot supply.
    pub needs_arguments: bool,
}

#[derive(Serialize)]
pub struct QueuedJson {
    pub index: usize,
    pub text: String,
}

#[derive(Serialize)]
pub struct TrajectoryJson {
    pub summary: TrajectorySummaryJson,
    pub spans: Vec<TrajectorySpanJson>,
    pub records: Vec<TrajectoryRecordJson>,
}

impl From<&TrajectoryView> for TrajectoryJson {
    fn from(view: &TrajectoryView) -> Self {
        let summary = view.summary();
        Self {
            summary: TrajectorySummaryJson {
                turns: summary.turns,
                records: summary.records,
                tool_calls: summary.tool_calls,
                measured_ms: summary.measured_ms,
                measured: summary.measured,
                input_tokens: summary.input_tokens,
                output_tokens: summary.output_tokens,
                cache_hit_tokens: summary.cache_hit_tokens,
                cache_miss_tokens: summary.cache_miss_tokens,
                context_window: summary.context_window,
                context_used: summary.context_used,
            },
            spans: view
                .spans()
                .iter()
                .map(|span| TrajectorySpanJson {
                    index: span.index,
                    lane: span.lane.into(),
                    is_error: span.is_error,
                })
                .collect(),
            records: view
                .records()
                .iter()
                .map(|record| TrajectoryRecordJson {
                    index: record.index,
                    turn: record.turn,
                    step: record.step,
                    kind: record.kind.label().to_owned(),
                    text: record.text.to_string(),
                    tool_name: record.tool_name.as_deref().map(str::to_owned),
                    at: record.at.as_deref().map(str::to_owned),
                    status: record.status.into(),
                    status_label: record.status_label().to_owned(),
                })
                .collect(),
        }
    }
}

#[derive(Serialize)]
pub struct TrajectorySummaryJson {
    pub turns: usize,
    pub records: usize,
    pub tool_calls: usize,
    pub measured_ms: u64,
    pub measured: usize,
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cache_hit_tokens: u32,
    pub cache_miss_tokens: u32,
    pub context_window: Option<usize>,
    pub context_used: u32,
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TrajectoryLaneJson {
    Input,
    Model,
    Tools,
}

impl From<tact_desktop_core::TrajectoryLane> for TrajectoryLaneJson {
    fn from(lane: tact_desktop_core::TrajectoryLane) -> Self {
        match lane {
            tact_desktop_core::TrajectoryLane::Input => Self::Input,
            tact_desktop_core::TrajectoryLane::Model => Self::Model,
            tact_desktop_core::TrajectoryLane::Tools => Self::Tools,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RowStatusJson {
    Pending,
    Running,
    Success,
    Failed,
    Cancelled,
    Unknown,
    Resolved,
}

impl From<RowStatus> for RowStatusJson {
    fn from(status: RowStatus) -> Self {
        match status {
            RowStatus::Pending => Self::Pending,
            RowStatus::Running => Self::Running,
            RowStatus::Success => Self::Success,
            RowStatus::Failed => Self::Failed,
            RowStatus::Cancelled => Self::Cancelled,
            RowStatus::Unknown => Self::Unknown,
            RowStatus::Resolved => Self::Resolved,
        }
    }
}

#[derive(Serialize)]
pub struct TrajectorySpanJson {
    pub index: usize,
    pub lane: TrajectoryLaneJson,
    pub is_error: bool,
}

#[derive(Serialize)]
pub struct TrajectoryRecordJson {
    pub index: usize,
    pub turn: usize,
    pub step: usize,
    pub kind: String,
    pub text: String,
    pub tool_name: Option<String>,
    pub at: Option<String>,
    pub status: RowStatusJson,
    pub status_label: String,
}

#[derive(Serialize)]
pub struct TrajectoryDetailJson {
    pub index: usize,
    pub text: String,
    pub kind: String,
    pub status: RowStatusJson,
    pub status_label: String,
    pub tool_name: Option<String>,
    pub at: Option<String>,
    pub input: Option<String>,
    pub output: Option<String>,
    pub duration_ms: Option<u64>,
    pub usage: Option<UsageJson>,
}

impl From<&tact_desktop_core::TrajectoryRecord> for TrajectoryDetailJson {
    fn from(record: &tact_desktop_core::TrajectoryRecord) -> Self {
        Self {
            index: record.index,
            text: record.text.to_string(),
            kind: record.kind.label().to_owned(),
            status: record.status.into(),
            status_label: record.status_label().to_owned(),
            tool_name: record.tool_name.as_deref().map(str::to_owned),
            at: record.at.as_deref().map(str::to_owned),
            input: record.input.as_deref().map(str::to_owned),
            output: record.output.as_deref().map(str::to_owned),
            duration_ms: record.duration_ms,
            usage: record.usage.as_ref().map(UsageJson::from),
        }
    }
}

#[derive(Serialize)]
pub struct InspectorJson {
    pub tab: String,
    pub goals: Vec<GoalJson>,
    pub tasks: Vec<TaskJson>,
    pub subagents: Vec<SubagentJson>,
    pub background: Vec<BackgroundJson>,
    pub context: UsageJson,
}

impl From<&InspectorView> for InspectorJson {
    fn from(inspector: &InspectorView) -> Self {
        Self {
            tab: inspector_tab(inspector.tab).to_owned(),
            goals: inspector.goals.iter().map(GoalJson::from).collect(),
            tasks: inspector.tasks.iter().map(TaskJson::from).collect(),
            subagents: inspector.subagents.iter().map(SubagentJson::from).collect(),
            background: inspector
                .background
                .iter()
                .map(BackgroundJson::from)
                .collect(),
            context: UsageJson::from(&inspector.context),
        }
    }
}

fn inspector_tab(tab: tact_desktop_core::InspectorTab) -> &'static str {
    use tact_desktop_core::InspectorTab;
    match tab {
        InspectorTab::Run => "run",
        InspectorTab::Goals => "goals",
        InspectorTab::Subagents => "subagents",
        InspectorTab::Background => "background",
        InspectorTab::Context => "context",
    }
}

#[derive(Serialize)]
pub struct GoalJson {
    pub id: String,
    pub title: String,
    pub objective: String,
    pub status: String,
    pub phase: String,
    pub task_total: usize,
    pub task_completed: usize,
    pub wait_reason: Option<String>,
    pub archived: bool,
}

impl From<&GoalView> for GoalJson {
    fn from(goal: &GoalView) -> Self {
        let Some(snapshot) = goal.snapshot.as_ref() else {
            return Self {
                id: String::new(),
                title: "Goal".to_owned(),
                objective: String::new(),
                status: "pending".to_owned(),
                phase: String::new(),
                task_total: 0,
                task_completed: 0,
                wait_reason: None,
                archived: false,
            };
        };
        Self {
            id: snapshot.id.clone(),
            title: snapshot.title.clone(),
            objective: snapshot.objective.clone(),
            status: format!("{:?}", snapshot.status).to_lowercase(),
            phase: format!("{:?}", snapshot.phase).to_lowercase(),
            task_total: snapshot.task_total,
            task_completed: snapshot.task_completed,
            wait_reason: snapshot.wait_reason.clone(),
            archived: snapshot.archived,
        }
    }
}

#[derive(Serialize)]
pub struct TaskJson {
    pub id: u64,
    pub subject: String,
    pub status: String,
    pub owner: String,
    pub started_at: Option<i64>,
    pub completed_at: Option<i64>,
}

impl From<&TaskView> for TaskJson {
    fn from(task: &TaskView) -> Self {
        Self {
            id: task.id,
            subject: task.subject.to_string(),
            status: format!("{:?}", task.status).to_lowercase(),
            owner: task.owner.to_string(),
            started_at: task.started_at,
            completed_at: task.completed_at,
        }
    }
}

#[derive(Serialize)]
pub struct SubagentJson {
    pub child_id: String,
    pub status: String,
    pub summary: String,
    pub model: Option<String>,
    pub elapsed_ms: u64,
    pub last_activity: Option<String>,
}

impl From<&SubagentView> for SubagentJson {
    fn from(agent: &SubagentView) -> Self {
        Self {
            child_id: agent.child_id.to_string(),
            status: format!("{:?}", agent.status).to_lowercase(),
            summary: agent.summary.to_string(),
            model: agent.model.as_deref().map(str::to_owned),
            elapsed_ms: agent.elapsed_ms,
            last_activity: agent.last_activity.as_deref().map(str::to_owned),
        }
    }
}

#[derive(Serialize)]
pub struct BackgroundJson {
    pub id: String,
    pub command: String,
    pub status: String,
    pub started_at: i64,
    pub output_tail: String,
}

impl From<&BackgroundTaskView> for BackgroundJson {
    fn from(task: &BackgroundTaskView) -> Self {
        Self {
            id: task.id.to_string(),
            command: task.command.to_string(),
            status: format!("{:?}", task.status).to_lowercase(),
            started_at: task.started_at,
            output_tail: task.output_tail.to_string(),
        }
    }
}

#[derive(Serialize)]
pub struct SurfaceJson {
    pub title: String,
    pub body: String,
}

#[derive(Serialize)]
pub struct ExtensionsJson {
    pub tab: String,
    pub skills: Vec<SkillJson>,
    pub plugins: Vec<PluginJson>,
    pub marketplaces: Vec<MarketplaceJson>,
}

impl From<&ExtensionsView> for ExtensionsJson {
    fn from(extensions: &ExtensionsView) -> Self {
        Self {
            tab: match extensions.tab {
                tact_desktop_core::ExtensionsTab::Mcp => "mcp",
                tact_desktop_core::ExtensionsTab::Skills => "skills",
                tact_desktop_core::ExtensionsTab::Plugins => "plugins",
                tact_desktop_core::ExtensionsTab::Hooks => "hooks",
            }
            .to_owned(),
            skills: extensions
                .snapshot
                .skills
                .iter()
                .map(|skill| SkillJson {
                    name: skill.name.clone(),
                    description: skill.description.clone(),
                    enabled: skill.enabled,
                })
                .collect(),
            plugins: extensions
                .snapshot
                .plugins
                .iter()
                .map(|plugin| PluginJson {
                    name: plugin.name.clone(),
                    marketplace: plugin.marketplace.clone(),
                    version: plugin.version.clone(),
                    enabled: plugin.enabled,
                })
                .collect(),
            marketplaces: extensions
                .snapshot
                .marketplaces
                .iter()
                .map(|marketplace| MarketplaceJson {
                    name: marketplace.name.clone(),
                    source: marketplace.source.clone(),
                })
                .collect(),
        }
    }
}

#[derive(Serialize)]
pub struct SkillJson {
    pub name: String,
    pub description: String,
    pub enabled: bool,
}

#[derive(Serialize)]
pub struct PluginJson {
    pub name: String,
    pub marketplace: String,
    pub version: Option<String>,
    pub enabled: bool,
}

#[derive(Serialize)]
pub struct MarketplaceJson {
    pub name: String,
    pub source: String,
}

#[derive(Serialize)]
pub struct SettingsJson {
    pub provider: String,
    pub api_key_set: bool,
    pub base_url: String,
    pub max_tokens: Option<u32>,
    pub context_window: Option<usize>,
    pub snapshot_max_items: Option<usize>,
    pub micro_compact: bool,
    pub tokio_console: bool,
    pub skill_body_auto_inject: bool,
    pub config_path: Option<String>,
}

impl From<&SettingsView> for SettingsJson {
    fn from(settings: &SettingsView) -> Self {
        Self {
            provider: settings.provider.to_string(),
            api_key_set: settings.api_key_set,
            base_url: settings.base_url.to_string(),
            max_tokens: settings.max_tokens,
            context_window: settings.context_window,
            snapshot_max_items: settings.snapshot_max_items,
            micro_compact: settings.micro_compact,
            tokio_console: settings.tokio_console,
            skill_body_auto_inject: settings.skill_body_auto_inject,
            config_path: settings.config_path.clone(),
        }
    }
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AccountJson {
    Balance {
        currency: String,
        total: String,
    },
    UsageQuota {
        plan: Option<String>,
        used: Option<u64>,
        limit: Option<u64>,
    },
    Error {
        message: String,
    },
}

impl From<&AccountView> for AccountJson {
    fn from(account: &AccountView) -> Self {
        match account {
            AccountView::Balance { currency, total } => AccountJson::Balance {
                currency: currency.to_string(),
                total: total.to_string(),
            },
            AccountView::UsageQuota {
                plan, used, limit, ..
            } => AccountJson::UsageQuota {
                plan: plan.as_deref().map(str::to_owned),
                used: *used,
                limit: *limit,
            },
            AccountView::Error(message) => AccountJson::Error {
                message: message.to_string(),
            },
        }
    }
}

#[derive(Serialize)]
pub struct ReaderJson {
    pub title: String,
    pub markdown: String,
}

/// A command TSX sends back.
#[derive(Deserialize, Debug)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum UiCommand {
    NewSession,
    ResumeSession {
        session_id: String,
    },
    Submit {
        text: String,
        attachments: Vec<String>,
    },
    Queue {
        text: String,
    },
    CancelRun,
    RetryProvider,
    SetInspector {
        tab: String,
    },
    ToggleGroup {
        id: u64,
    },
    ToggleTool {
        group: u64,
        tool: u64,
    },
    RespondGate {
        session_epoch: u64,
        request_id: u64,
        selected: Vec<usize>,
        single: Option<usize>,
    },
    SetGateDraft {
        session_epoch: u64,
        request_id: u64,
        selected: Vec<usize>,
    },
    SetSessionFilter {
        filter: String,
    },
    SetSessionQuery {
        query: String,
    },
    RenameSession {
        session_id: String,
        title: String,
    },
    SetSessionArchived {
        session_id: String,
        archived: bool,
    },
    SendQueuedNow {
        index: usize,
    },
    RemoveQueued {
        index: usize,
    },
    MoveQueued {
        from: usize,
        to: usize,
    },
    EditQueued {
        index: usize,
        text: String,
    },
    CancelSubagent {
        child_id: String,
    },
    CancelBackgroundTask {
        task_id: String,
    },
    InvokeCommand {
        path: Vec<String>,
        tokens: Vec<String>,
    },
    OpenReader {
        reader_kind: String,
        id: Option<u64>,
        key: Option<String>,
    },
    OpenProject {
        root: String,
    },
    SetSetting {
        id: String,
        value: SettingValueJson,
    },
    SetSkillEnabled {
        name: String,
        enabled: bool,
    },
    SetTailFollowing {
        following: bool,
    },
    BackToBottom,
    Voice {
        command: String,
    },
    Shutdown,
}

#[derive(Deserialize, Debug)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SettingValueJson {
    Bool { value: bool },
    Text { value: String },
    Choice { value: String },
}
