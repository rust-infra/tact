//! GPUI-free desktop domain model.

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
pub mod transcript;

pub use command_router::{
    CliRouteClass, CommandAvailability, CommandForm, CommandPlan, CommandTarget, SettingMode,
    canonical_key, catalog_entries, cli_catalog_parity_errors, cli_entry_has_setting_route,
    cli_route_class, form_for, is_enabled, plan as plan_command, search as search_commands,
    setting_mode,
};
pub use config::{
    Appearance, DesktopConfig, HostLaunchOptions, InitialSession, PermissionModeView,
    ReducedMotion, app_support_dir,
};
pub use controller::{DesktopController, DesktopEvents, tact_ui_binary_path};
pub use effect::{DesktopEffect, DesktopEvent};
pub use error::{DesktopClosed, DesktopShutdownError, DesktopStartError, FailureView};
pub use intent::{DesktopIntent, GateResponse, ReaderRef, ReaderRequest, SettingValue, Submission};
pub use project::{ProjectId, ProjectRecord, ProjectRegistry};
pub use session::{SessionCatalog, SessionSummaryView};
pub use session_lock::{exit_listener_install_count, install_exit_listener};
pub use snapshot::{
    AccountView, CommandCatalogView, ComposerView, DesktopSnapshot, ExtensionsTab, ExtensionsView,
    InspectorTab, InspectorView, ProjectView, QueuedPromptView, Revision, RunPhase, RuntimeView,
    SectionMask, SessionId, SessionListState, SessionPhase, SessionView, StateCommit,
};
pub use state::DesktopState;
pub use tact::store::ArchiveFilter;
pub use tact_catalog::{CatalogLocale, CommandCatalog, CommandId, LocalizedText};
pub use tact_protocol::{
    BackgroundTaskStatusSnapshot, GoalCriterionStatusSnapshot, GoalPhaseSnapshot,
    GoalStatusSnapshot, McpServerStatusSnapshot, SubagentStatusSnapshot, SurfaceSnapshot,
    TaskStatusSnapshot, UiRequestPurpose,
};
pub use transcript::{
    BackgroundTaskView, GateCard, GateKind, GateOrigin, GateSelection, GateState, GoalView, LoopId,
    RequestId, RowBody, RowId, RowStatus, SessionEpoch, SubagentView, TaskView, ToolBody,
    ToolGroupItem, ToolGroupRow, ToolMetaView, ToolOutputView, ToolRow, TranscriptRow,
    TranscriptView, UsageView,
};
pub(crate) use transcript::{ReduceContext, TranscriptReducer};
