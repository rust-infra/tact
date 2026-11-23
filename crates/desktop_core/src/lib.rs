//! GPUI-free desktop domain model.

pub mod acp_bridge;
#[cfg(feature = "acp-host")]
pub mod acp_host;
#[cfg(feature = "acp-host")]
pub mod acp_inprocess;
mod command_router;
mod commands;
mod config;
mod controller;
mod effect;
mod error;
pub mod gates;
mod intent;
mod project;
mod runtime;
mod session;
mod session_lock;
mod snapshot;
mod state;
pub mod trajectory;
pub mod transcript;

#[cfg(any(test, feature = "ui-capture"))]
pub mod fixtures;

pub use command_router::{
    CliRouteClass, CommandAvailability, CommandForm, CommandPlan, CommandTarget, SettingMode,
    canonical_key, catalog_entries, cli_catalog_parity_errors, cli_entry_has_setting_route,
    cli_route_class, command_is_executable, form_for, is_enabled, plan as plan_command,
    search as search_commands, setting_mode,
};
pub use config::{
    Appearance, DesktopConfig, DesktopTheme, HostLaunchOptions, InitialSession, PermissionModeView,
    ReducedMotion, app_support_dir, micro_compact_flag,
};
pub use controller::{DesktopController, DesktopEvents, help_reader, tact_ui_binary_path};
pub use effect::{DesktopEffect, DesktopEvent};
pub use error::{DesktopClosed, DesktopShutdownError, DesktopStartError, FailureKind, FailureView};
pub use intent::{DesktopIntent, GateResponse, ReaderRef, ReaderRequest, SettingValue, Submission};
pub use project::{ProjectId, ProjectRecord, ProjectRegistry};
pub use session::{SessionCatalog, SessionSummaryView};
pub use session_lock::{exit_listener_install_count, install_exit_listener};
pub use snapshot::{
    AccountView, CommandCatalogView, ComposerView, DesktopSnapshot, ExtensionsTab, ExtensionsView,
    InspectorTab, InspectorView, LatestToolView, ProjectView, QueuedPromptView, Revision, RunPhase,
    RuntimeView, SectionMask, SessionId, SessionListState, SessionPhase, SessionView, SettingsView,
    StateCommit,
};
pub use state::DesktopState;
/// The thought control's tiers for `model`, read from the installed config.
///
/// Re-exported here so the GPUI shell does not take a dependency on `tact`
/// just to ask what a model's reasoning control offers.
pub fn thought_tiers(model: &str) -> Option<tact_host::ThoughtTiers> {
    tact::config::try_settings().map(|settings| tact_host::thought_tiers(&settings, model))
}

pub use tact::store::ArchiveFilter;
pub use tact::voice::{VoiceCommand, VoiceEvent};
pub use tact_agent_api::MessagePhase;
pub use tact_catalog::{CatalogLocale, CommandCatalog, CommandId, LocalizedText};
pub use tact_host::{DEFAULT_THINKING_BUDGETS, ThoughtKind, ThoughtTiers};
pub use tact_protocol::{
    BackgroundTaskStatusSnapshot, GoalCriterionStatusSnapshot, GoalPhaseSnapshot,
    GoalStatusSnapshot, HookItemSnapshot, HookSnapshot, McpServerStatusSnapshot,
    SubagentStatusSnapshot, SurfaceSnapshot, TRY_AGAIN_PROMPT, TaskStatusSnapshot,
    UiRequestPurpose,
};
pub use trajectory::{
    TrajectoryKind, TrajectoryLane, TrajectoryRecord, TrajectoryRecordId, TrajectorySpan,
    TrajectorySummary, TrajectoryTurn, TrajectoryView, fold_step_status, search_terms,
};
pub use transcript::{
    BackgroundTaskView, GateCard, GateKind, GateOrigin, GateSelection, GateState, GoalView, LoopId,
    PlanRow, PlanStep, RequestId, RowBody, RowId, RowStatus, SessionEpoch, SubagentView, TaskView,
    ToolBody, ToolGroupItem, ToolGroupRow, ToolMetaView, ToolOutputView, ToolRow, TranscriptRow,
    TranscriptView, UsageView, one_line_summary,
};
pub(crate) use transcript::{ReduceContext, TranscriptReducer};
