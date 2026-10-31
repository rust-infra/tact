use std::{path::PathBuf, sync::Arc};

use chrono::{DateTime, Utc};
use tact::store::ArchiveFilter;
use tact_catalog::{CatalogLocale, CommandId, CommandSearchHit};
use tact_host::ExtensionSnapshot;

use tact_protocol::SurfaceSnapshot;

use crate::{
    Appearance, BackgroundTaskView, FailureView, GateCard, GoalView, ProjectId, ReducedMotion,
    RowId, SessionEpoch, SessionSummaryView, SubagentView, TaskView, TranscriptView, UsageView,
};

pub type Revision = u64;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SessionId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProjectView {
    pub id: Option<ProjectId>,
    pub root: Option<PathBuf>,
    pub display_name: Arc<str>,
    pub available: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionView {
    pub id: Option<SessionId>,
    pub epoch: SessionEpoch,
    pub title: Arc<str>,
    pub root_dir: Option<PathBuf>,
    pub phase: SessionPhase,
    pub locked: bool,
    pub updated_at: Option<DateTime<Utc>>,
}

impl Default for SessionView {
    fn default() -> Self {
        Self {
            id: None,
            epoch: SessionEpoch(0),
            title: Arc::from(""),
            root_dir: None,
            phase: SessionPhase::Empty,
            locked: false,
            updated_at: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionPhase {
    Empty,
    Opening,
    Starting,
    Ready,
    Running,
    Switching,
    Closing,
    Disconnected { reason: Arc<str> },
    Failed { reason: Arc<str> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionListState {
    pub project: Option<ProjectId>,
    pub filter: ArchiveFilter,
    pub query: String,
    pub items: Vec<SessionSummaryView>,
    pub selected: Option<SessionId>,
    pub loading: bool,
    pub error: Option<FailureView>,
}

impl Default for SessionListState {
    fn default() -> Self {
        Self {
            project: None,
            filter: ArchiveFilter::Active,
            query: String::new(),
            items: Vec::new(),
            selected: None,
            loading: false,
            error: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeView {
    pub run_started_at: Option<DateTime<Utc>>,
    pub current_phase: RunPhase,
    pub latest_tool: Option<Arc<str>>,
    pub turns_taken: u32,
    pub model: Option<Arc<str>>,
    pub usage: UsageView,
    pub queue_paused: bool,
}

impl Default for RuntimeView {
    fn default() -> Self {
        Self {
            run_started_at: None,
            current_phase: RunPhase::Idle,
            latest_tool: None,
            turns_taken: 0,
            model: None,
            usage: UsageView::default(),
            queue_paused: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RunPhase {
    #[default]
    Idle,
    Waiting,
    Thinking,
    Streaming,
    RunningTools,
    WaitingForGate,
    Cancelling,
    Complete,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComposerView {
    pub queue: Arc<[QueuedPromptView]>,
    pub model: Arc<str>,
    pub models: Arc<[Arc<str>]>,
    pub subagent_model: Arc<str>,
    pub subagent_models: Arc<[Arc<str>]>,
    pub effort: Option<Arc<str>>,
    pub permission_mode: crate::PermissionModeView,
    pub thinking_budget: Option<u32>,
    /// Recent submitted prompts for this session, newest first.
    pub history: Arc<[Arc<str>]>,
    pub slash_catalog: Arc<CommandCatalogView>,
}

impl Default for ComposerView {
    fn default() -> Self {
        Self {
            queue: Arc::from([]),
            model: Arc::from(""),
            models: Arc::from([]),
            subagent_model: Arc::from(""),
            subagent_models: Arc::from([]),
            effort: None,
            permission_mode: crate::PermissionModeView::Default,
            thinking_budget: None,
            history: Arc::from([]),
            slash_catalog: Arc::new(CommandCatalogView::default()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueuedPromptView {
    pub id: u64,
    pub text: Arc<str>,
    pub attachments: Arc<[PathBuf]>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommandCatalogView {
    pub recent: Arc<[CommandId]>,
    pub items: Arc<[CommandSearchHit]>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InspectorView {
    pub tab: InspectorTab,
    pub goals: Arc<[GoalView]>,
    pub tasks: Arc<[TaskView]>,
    pub subagents: Arc<[SubagentView]>,
    pub background: Arc<[BackgroundTaskView]>,
    pub context: UsageView,
}

impl Default for InspectorView {
    fn default() -> Self {
        Self {
            tab: InspectorTab::Run,
            goals: Arc::from([]),
            tasks: Arc::from([]),
            subagents: Arc::from([]),
            background: Arc::from([]),
            context: UsageView::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InspectorTab {
    #[default]
    Run,
    Goals,
    Subagents,
    Context,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionsView {
    pub tab: ExtensionsTab,
    pub query: String,
    pub snapshot: ExtensionSnapshot,
}

impl Default for ExtensionsView {
    fn default() -> Self {
        Self {
            tab: ExtensionsTab::Mcp,
            query: String::new(),
            snapshot: ExtensionSnapshot {
                skills: Vec::new(),
                plugins: Vec::new(),
                marketplaces: Vec::new(),
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ExtensionsTab {
    #[default]
    Mcp,
    Skills,
    Plugins,
    Hooks,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccountView {
    Balance {
        currency: Arc<str>,
        total: String,
    },
    UsageQuota {
        plan: Option<Arc<str>>,
        used: Option<u64>,
        limit: Option<u64>,
        resets_at: Option<Arc<str>>,
    },
    Error(Arc<str>),
}

#[derive(Debug, Clone)]
pub struct DesktopSnapshot {
    pub revision: Revision,
    pub projects: Arc<[ProjectView]>,
    pub project: ProjectView,
    pub sessions: SessionListState,
    pub session: SessionView,
    pub runtime: RuntimeView,
    pub transcript: TranscriptView,
    pub composer: ComposerView,
    pub inspector: InspectorView,
    pub pending_gates: Arc<[GateCard]>,
    pub catalog: Arc<CommandCatalogView>,
    pub extensions: ExtensionsView,
    pub account: Option<AccountView>,
    pub account_supported: bool,
    pub show_hook_output: bool,
    pub notifications: bool,
    pub failure: Option<FailureView>,
    pub appearance: Appearance,
    pub motion: ReducedMotion,
    pub text_scale: f32,
    pub language: CatalogLocale,
    pub tail_following: bool,
    pub surface: Option<SurfaceSnapshot>,
    pub settings: SettingsView,
}

/// Startup-global settings shown in the Settings surface. Live session
/// settings live in `ComposerView`/`SessionView`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SettingsView {
    pub provider: Arc<str>,
    pub api_key_set: bool,
    pub base_url: Arc<str>,
    pub max_tokens: Option<u32>,
    pub context_window: Option<usize>,
    pub snapshot_max_items: Option<usize>,
    pub micro_compact: bool,
    pub tokio_console: bool,
    pub skill_body_auto_inject: bool,
    pub config_path: Option<String>,
}

/// Build the startup-global settings view from the desktop configuration.
pub fn settings_from(config: &crate::DesktopConfig) -> SettingsView {
    SettingsView {
        provider: Arc::from(config.host.provider.clone().unwrap_or_default()),
        api_key_set: config
            .host
            .api_key
            .as_deref()
            .is_some_and(|key| !key.is_empty()),
        base_url: Arc::from(config.host.base_url.clone().unwrap_or_default()),
        max_tokens: config.host.max_tokens,
        context_window: config.host.model_context_window,
        snapshot_max_items: config.host.snapshot_max_items,
        micro_compact: config.host.micro_compact.unwrap_or(true),
        tokio_console: config.host.tokio_console,
        skill_body_auto_inject: config.host.skill_body_auto_inject,
        config_path: config
            .host
            .config_path
            .as_ref()
            .map(|path| path.display().to_string()),
    }
}

impl DesktopSnapshot {
    pub fn empty(config: &crate::DesktopConfig) -> Self {
        Self {
            revision: 0,
            projects: Arc::from([]),
            project: ProjectView::default(),
            sessions: SessionListState::default(),
            session: SessionView::default(),
            runtime: RuntimeView::default(),
            transcript: TranscriptView::default(),
            composer: ComposerView::default(),
            inspector: InspectorView::default(),
            pending_gates: Arc::from([]),
            catalog: Arc::new(CommandCatalogView::default()),
            extensions: ExtensionsView::default(),
            account: None,
            account_supported: false,
            show_hook_output: true,
            notifications: true,
            failure: None,
            appearance: config.appearance,
            motion: config.reduced_motion,
            text_scale: config.text_scale,
            language: config.language,
            tail_following: true,
            surface: None,
            settings: settings_from(config),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SectionMask(u16);

impl SectionMask {
    pub const SESSION: Self = Self(1 << 0);
    pub const TRANSCRIPT: Self = Self(1 << 1);
    pub const INSPECTOR: Self = Self(1 << 2);
    pub const GATES: Self = Self(1 << 3);
    pub const COMMANDS: Self = Self(1 << 4);
    pub const EXTENSIONS: Self = Self(1 << 5);
    pub const FAILURE: Self = Self(1 << 6);

    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}

#[derive(Debug, Clone)]
pub struct StateCommit {
    pub revision: Revision,
    pub snapshot: Arc<DesktopSnapshot>,
    pub dirty_rows: Arc<[RowId]>,
    pub sections: SectionMask,
}
