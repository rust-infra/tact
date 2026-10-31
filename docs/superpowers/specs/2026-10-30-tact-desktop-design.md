# Tact Desktop Architecture

> Date: 2026-10-30
> Status: implemented; verification is tracked by the implementation plan and
> the repository Rust/book gates.
> Related: `crates/tact_host/`, `crates/protocol/`, `crates/tact-ui/`,
> `crates/tui/`, `docs/superpowers/specs/2026-10-30-tact-desktop-prd.md`
> Plan: `docs/superpowers/plans/2026-10-30-tact-desktop.md`

`gpui-ai` is pinned to `0.8.0` and patched from `vendor/gpui-ai`. The local
fork keeps the upstream component grammar and adds configurable labels,
localized accessibility copy, and empty-state text required by the desktop
shell. `vendor/gpui-ai/README.md` records the patch scope and the update
procedure; do not replace it with an unpatched crates.io build without carrying
those changes forward.

## Goal

Add a native macOS, Windows, and Linux Tact Desktop frontend that is a complete
product surface over the existing Tact runtime. The desktop owns layout, focus,
motion, and presentation. `tact_host` remains the single owner of the live
session, the agent command loop, the session lock, and teardown.
`tact_protocol` remains the only runtime-to-frontend contract.

The architecture must prove the mixed transcript before feature breadth. A
message, thinking trace, tool group, diff, permission gate, Goal, and subagent
must keep stable identity while streaming and while the user scrolls away from
the live tail.

## Non-Goals

- Do not implement the agent loop, tool router, MCP client, hooks, permission
  policy, Goals, subagents, memory, plugins, or session persistence in desktop
  crates.
- Do not make `gpui-ai::Chat` the Tact transcript. Its message model cannot
  interleave arbitrary tools, reasoning, diffs, gates, and notices.
- Do not make GPUI a dependency of `tact`, `tact_host`, `tact_protocol`, or the
  shared command catalog.
- Do not create a second runtime or a second session database.
- Do not create a second hand-maintained command list for the desktop.
- Do not ship a web or remote frontend in the first release. The first
  transport is the existing in-process `tact_host` channel.
- Do not preserve terminal keybindings literally. Desktop shortcuts express
  the same intent with platform-native conventions.

## Decision Summary

| ID | Decision |
|---|---|
| D1 | Add a GPUI-free `tact_desktop_core` controller/reducer and a thin `tact-desktop` GPUI shell. |
| D2 | Add `tact_host::HostSession` as the only owner of live-session bootstrap, command driving, lock release, account updates, and teardown ordering. |
| D3 | Add `tact_catalog` as the single frontend-neutral source for slash, CLI, setting, and composer command metadata and fuzzy matching. |
| D4 | Extend the host protocol with kernel-originated `RunStarted` and `MessageStarted` hints, typed gate purpose/origin, and typed surface snapshots. |
| D5 | Keep project-local `<workdir>/.tact/tact.db` authoritative; add a desktop-only project registry for recent project roots. |
| D6 | Migrate the session store additively with `title` and `archived_at`, and derive a title from the first real user prompt. |
| D7 | Make the mixed transcript a Tact-owned virtualized row model; use real `gpui-ai` components for leaf AI surfaces. |
| D8 | Group tools by a kernel-provided run/message identity, never by adjacency or wall-clock time. |
| D9 | Reconcile gates from `UiResponder::snapshot()`, not from one-shot `RequestSelect` events. |
| D10 | Keep one active `HostSession` per desktop controller; a project switch is a serialized close-then-open transition. |
| D11 | Treat Tact configuration as process-global for v1. Desktop loads it once; project switching does not reinstall provider settings. Permission mode is always re-applied per session because it is a safety control, not a provider preference. |
| D12 | Use Light and Dark desktop appearances with shared component geometry and a reduced-motion policy on macOS, Windows, and Linux. |
| D13 | Split settings into startup-global settings (require a restart) and live session settings (routed through `UserCommand`). A settings surface must show which kind it is. |
| D14 | The controller actor is the sole owner of `&mut HostSession`; `close` is retryable and a timeout leaves the handle and lock intact. |
| D15 | Gate cards carry the session epoch they were rendered under, and the controller rejects a response whose card epoch is stale. |

## Grounded Constraints

These facts shape the design and must not be rediscovered during implementation:

- `tact_host::session_bootstrap::open_session` creates or resumes a row,
  acquires the session lock, touches the session, and registers the lock.
- `bootstrap_session` constructs the only product `Agent` with the model,
  tools, MCP, hooks, permissions, task/goal/background/team/worktree/subagent
  managers, and either interactive or observing UI wiring.
- `run_command_loop_with_account` owns the `Agent` for the session lifetime.
  The frontend must not hold or mutate that `Agent` concurrently.
- `tact_protocol::update_channel` separates bounded advisory progress from
  reliable ordered terminal updates. The desktop bridge must consume it on a
  Tokio task and batch into GPUI.
- `UiResponder::snapshot()` is the authoritative pending gate list. A
  `RequestSelect` event is only a wake-up hint.
- `TactPath::session_db_path()` is `<workdir>/.tact/tact.db`, so session
  isolation is already project-local. There is no global project database.
- `SessionSummary` currently has no title or archive fields. The `sessions`
  table has no `title` or `archived_at` columns.
- The TUI slash command enum and subcommand tree are private to `crates/tui`.
  Desktop cannot import them, and duplicating them would drift.
- `gpui-ai` v0.8.0 pins `gpui-pre = 0.3.3`, `gpui-pre-platform = 0.3.3`,
  `gpui-component = 0.6.0`, `gpui-base = 0.6.0`, and
  `gpui-kit-assets = 0.6.0`. Pin `gpui-component-macros = 0.6.0` as well:
  the component crate's 0.6.0 release accepts a later macro patch whose plot
  API does not match that release. All five component crates resolve in the
  local registry, so the pins are verifiable before Task 0. `#[gpui::test]` is
  gated behind `gpui-pre`'s `test-support` feature; the desktop crate must
  enable it in `[dev-dependencies]` (as `gpui-ai` itself does).
- `gpui-ai` components are controlled: they render snapshots and emit typed
  events. `PromptBar` commands are flat; `ThreadList` has no project/filter
  model; `CommandSearch` is a flat item list.
- Tact configuration is installed process-globally and must be installed
  exactly once. Provider and model changes also update process-global state.
  Permission mode is read from that global config by the host, so the desktop
  explicitly sends `UserCommand::SetPermissionMode` after `HostEvent::Ready`
  for every session instead of inheriting the previous project's mode.
- `SqliteSessionStore::new` runs migrations on every open. New columns use the
  existing `add_nullable_column` / `PRAGMA table_info` guard, never a bare
  `ALTER TABLE`, because SQLite has no `ADD COLUMN IF NOT EXISTS`.
- `UiResponder` is recreated per session and its `next_id` starts at zero, so
  `request_id` is only unique within a session. Gate staleness must be checked
  with the session epoch, not with `request_id` alone.
- `SessionLockRegistry::spawn_exit_listener` exists but is currently installed
  only by `tact-ui`. The desktop binary must install it before opening a
  session, or a Cmd-Q termination can leave a lock behind.
- `AgentEvent::RunStarted` is emitted by both `AgentKernel::run` and
  `AgentKernel::compact`, and each path emits a terminal `RunFinished` or
  `RunFailed`. `RunStarted` alone is therefore not an activation boundary.

## Usage (caller's view)

### GPUI shell startup

The GPUI crate imports only `tact_desktop_core`, `tact_catalog`, and GPUI
component crates. It never imports `tact`, `tact_host`, `tact_protocol`, Tokio
channels, or SQLite.

```rust
use tact_desktop_core::{
    DesktopConfig, DesktopController, DesktopIntent, InitialSession,
};

fn main() {
    gpui_platform::application()
        .with_assets(gpui_component_assets::Assets)
        .run(|cx| {
            gpui_ai::init(cx);

            let config = DesktopConfig::load(app_data_dir())
                .expect("desktop settings load")
                .with_initial_project(PathBuf::from("/Users/me/Projects/tact"))
                .with_initial_session(InitialSession::ResumeLast);

            let (controller, events) =
                DesktopController::start(config).expect("desktop controller starts");

            cx.open_window(WindowOptions::default(), move |_, cx| {
                cx.new(|cx| DesktopRoot::new(controller, events, cx))
            })
            .expect("window opens");
        });
}
```

`DesktopRoot` owns the GPUI view state: focus handles, scroll position, draft
text, selection, expansion state, and overlay stack. It polls
`DesktopEvents::recv()` and applies `StateCommit` values to row entities.

```rust
impl DesktopRoot {
    fn start_event_pump(&mut self, cx: &mut Context<Self>) {
        let mut events = self.events.take().expect("event receiver installed once");
        cx.spawn(async move |this, cx| {
            while let Ok(event) = events.recv().await {
                this.update(cx, |root, cx| {
                    root.apply(event, cx);
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }
}
```

### Semantic intents

The shell sends distilled intents. It does not assemble `UserCommand` values or
coordinate the runtime by hand.

```rust
controller.dispatch(DesktopIntent::Submit(Submission {
    text: "Fix the flaky retry test".into(),
    attachments: vec![PathBuf::from("crates/agent_core/src/retry.rs")],
    model: None,
    permission_mode: None,
}))?;

controller.dispatch(DesktopIntent::RespondGate {
    session_epoch: snapshot.session.epoch,
    request_id: gate.request_id,
    response: GateResponse::Select(Some(0)),
})?;

controller.dispatch(DesktopIntent::OpenProject {
    root: PathBuf::from("/Users/me/Projects/another-project"),
})?;
```

### Host session

`HostSession` is the deep host module that the desktop core consumes. It hides
the order that is easy to get wrong: create or resume, lock, bootstrap, drive,
cancel, drain, release. The controller actor is the only owner of the mutable
handle; it selects between host events and the shell's shutdown intent, so a
window close never races a borrowed `next_event`.

```rust
use tact_host::{
    HostSession, HostSessionConfig, SessionLockRegistry, SessionSelection, TactPath,
};

let mut host = HostSession::start(HostSessionConfig {
    tact_path: TactPath::new(work_dir.clone()),
    work_dir,
    session: SessionSelection::ResumeLast,
    args,
    session_store,
    lock_registry: SessionLockRegistry::new(),
    progress_capacity: 256,
    account_polling: true,
})
.await?;

loop {
    tokio::select! {
        event = host.next_event() => match event {
            Some(HostEvent::Agent(update)) => desktop.apply_agent_update(update),
            Some(HostEvent::Account(update)) => desktop.apply_account_update(update),
            Some(HostEvent::Ready) => desktop.ready(),
            Some(HostEvent::StartupFailed(error)) => desktop.startup_failed(error),
            Some(HostEvent::Disconnected) => desktop.disconnected(),
            None => break,
        },
        intent = shell.recv() => match intent {
            DesktopIntent::Shutdown => break,
            other => desktop.apply_intent(other),
        },
    }
}

// Retryable: a timeout leaves the handle, driver, and lock intact.
loop {
    match host.close().await {
        Ok(exit) => break exit,
        Err(HostCloseError::DrainTimedOut) => {
            desktop.show_stopping_with_retry();
            shell.wait_for_retry().await;
        }
        Err(error) => return Err(error),
    }
}
```

### Command Center and slash commands

The desktop and TUI read the same catalog. The desktop renders it through
`gpui-ai::CommandSearch`; the TUI keeps its terminal renderer but uses the same
descriptor IDs, paths, and subcommand tree.

```rust
use tact_catalog::{builtins, CatalogLocale, CommandId};

let catalog = builtins();
let goal_accept = catalog
    .resolve_path(&["goal", "accept"])
    .expect("catalog contains goal accept");

assert_eq!(
    goal_accept.id,
    CommandId::builtin_path("goal accept")
);
assert_eq!(
    goal_accept.label.get(CatalogLocale::Zh),
    "Goal accepted"
);
```

## Shape

### Crate and dependency map

```text
tact-desktop (GPUI binary, cross-platform shell, focus/scroll/motion)
  -> tact_desktop_core
       -> tact_host
       -> tact_catalog
       -> tact_protocol
       -> tact (store types only: SessionStore, SessionSummary,
                SessionListQuery, ArchiveFilter, TactPath)
  -> tact_catalog
  -> gpui-ai / gpui-component / gpui-base / gpui-component-assets

tact-ui (interactive TUI, headless CLI)
  -> tact_host
  -> tact_catalog
  -> tui

tui
  -> tact_catalog
  -> agent_tui_kit
  -> tact_protocol

tact_host
  -> tact
  -> tact_protocol
  -> tact_catalog only for command IDs used by host adapters

tact
  -> tact_protocol
  -> agent kernel crates
```

`gpui`, `gpui-ai`, and `gpui-component` never appear in `tact`, `tact_host`,
`tact_protocol`, `tact_catalog`, or `tact_desktop_core`.

### Ownership

| State or decision | Sole owner | Readers |
|---|---|---|
| Agent loop, tools, MCP router, permissions, Goals, tasks, background, subagents | `tact` / `tact_host::HostSession` | `AgentUpdate` projections |
| Live session lock, command channel, driver task, account channel | `tact_host::HostSession` | desktop core through the facade |
| Pending select and multi-select waiters | `tact::UiResponder` | gate snapshot reconciliation |
| Desktop transcript, composer queue, inspector, overlays | `tact_desktop_core::DesktopState` | GPUI snapshots |
| Project roots and recent-project metadata | `tact_desktop_core::ProjectRegistry` | session rail and project pane |
| Session title, archive, and lock state | project-local `SessionStore` | session catalog |
| Command names, paths, arguments, aliases, availability | `tact_catalog` | TUI, desktop, tests |
| Draft text, focus, scroll, selection, expansion, animation | `tact-desktop` shell | desktop core only receives semantic intents |
| Theme tokens, motion policy, accessibility labels | `tact-desktop` shell | no runtime ownership |

### Public desktop surface

The shell gets a small, runtime-free interface:

```rust
pub struct DesktopController {
    inner: Arc<ControllerInner>,
}

impl DesktopController {
    pub fn start(config: DesktopConfig)
        -> Result<(Self, DesktopEvents), DesktopStartError>;

    pub fn dispatch(&self, intent: DesktopIntent) -> Result<(), DesktopClosed>;

    pub fn snapshot(&self) -> Arc<DesktopSnapshot>;

    pub async fn when_idle(&self) -> Result<(), DesktopClosed>;

    pub async fn shutdown(self) -> Result<(), DesktopShutdownError>;
}

pub struct DesktopEvents {
    // private receiver
}

impl DesktopEvents {
    pub async fn recv(&self) -> Result<DesktopEvent, DesktopClosed>;
}

pub enum DesktopEvent {
    Committed(StateCommit),
    Effect(DesktopEffect),
    Fatal(FailureView),
}

pub struct StateCommit {
    pub revision: Revision,
    pub snapshot: Arc<DesktopSnapshot>,
    pub dirty_rows: Arc<[RowId]>,
    pub sections: SectionMask,
}

pub enum DesktopEffect {
    CopyText(String),
    OpenPath(PathBuf),
    RevealInFinder(PathBuf),
    OpenExternal(Url),
    OpenReader(ReaderRequest),
    SaveTranscript { markdown: String, suggested_name: String },
    Notify { title: String, body: String },
    WindowCloseRequested,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SectionMask(u16);

impl SectionMask {
    pub const SESSION: Self = Self(1 << 0);
    pub const TRANSCRIPT: Self = Self(1 << 1);
    pub const INSPECTOR: Self = Self(1 << 2);
    pub const GATES: Self = Self(1 << 3);
    pub const COMMANDS: Self = Self(1 << 4);
    pub const EXTENSIONS: Self = Self(1 << 5);
    pub const FAILURE: Self = Self(1 << 6);
}

pub type Revision = u64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DesktopStartError {
    Config(String),
    Runtime(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DesktopShutdownError {
    Host(HostCloseError),
    Runtime(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DesktopClosed;

pub struct DesktopConfig {
    pub app_data_dir: PathBuf,
    pub initial_project: Option<PathBuf>,
    pub initial_session: InitialSession,
    pub host: HostLaunchOptions,
    pub update_batch: Duration,
    pub gate_reconcile: Duration,
    pub appearance: Appearance,
    pub language: CatalogLocale,
    pub reduced_motion: ReducedMotion,
}

impl DesktopConfig {
    pub fn load(app_data_dir: PathBuf) -> Result<Self, DesktopStartError>;
    pub fn save(&self) -> Result<(), DesktopStartError>;
    pub fn install_process_config(&self) -> Result<(), DesktopStartError>;
    /// Persist the subagent model through the existing Tact config writer.
    /// Subagent model changes are not a `UserCommand`; they apply to the next
    /// spawned child.
    pub fn persist_subagent_model(&self, model: &str) -> Result<(), DesktopStartError>;
    pub fn with_initial_project(self, root: PathBuf) -> Self;
    pub fn with_initial_session(self, initial: InitialSession) -> Self;
}

pub enum InitialSession {
    New,
    ResumeLast,
    Resume(SessionId),
    None,
}

pub struct HostLaunchOptions {
    pub config_path: Option<PathBuf>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub api_key: Option<String>,
    pub base_url: Option<String>,
    pub max_tokens: Option<u32>,
    pub model_context_window: Option<usize>,
    pub snapshot_max_items: Option<usize>,
    pub micro_compact: Option<bool>,
    pub tokio_console: bool,
    pub skill_body_auto_inject: bool,
    pub permission_mode: PermissionModeView,
}

pub enum DesktopIntent {
    OpenProject { root: PathBuf },
    CloseProject { project_id: ProjectId },
    NewSession,
    ResumeSession { session_id: SessionId },
    ResumeLast,
    Submit(Submission),
    Queue(Submission),
    EditQueued { index: usize, text: String },
    RemoveQueued { index: usize },
    CancelRun,
    RespondGate {
        session_epoch: SessionEpoch,
        request_id: RequestId,
        response: GateResponse,
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
    ToggleGroup { id: RowId },
    ToggleTool { group: RowId, tool: RowId },
    OpenReader(ReaderRef),
    SetTailFollowing(bool),
    BackToBottom,
    CancelSubagent { child_id: String },
    SetSetting { id: SettingId, value: SettingValue },
    RetryProvider,
    RestartSession,
    Shutdown,
}

pub struct Submission {
    pub text: String,
    pub attachments: Vec<PathBuf>,
    pub model: Option<String>,
    pub permission_mode: Option<PermissionModeView>,
}

pub enum PermissionModeView {
    Default,
    Plan,
    Auto,
}

pub struct CommandArguments {
    pub tokens: Vec<String>,
}

pub enum GateResponse {
    Select(Option<usize>),
    MultiSelect(Option<Vec<usize>>),
}

pub enum ReaderRef {
    ToolOutput(RowId),
    Diff(RowId),
    Thinking(RowId),
    SystemPrompt,
    Stats,
    History,
    Help,
    Balance,
    Subagent(String),
    Background(String),
}

pub enum SettingValue {
    Bool(bool),
    Text(String),
    Choice(String),
}

pub struct ReaderRequest {
    pub title: Arc<str>,
    pub markdown: Arc<str>,
}
```

`DesktopSnapshot` is immutable. `dirty_rows` lets the shell update only the
row entities whose content changed. The transcript's structural vector uses a
persistent representation internally; the public surface exposes only indexed
iteration and `get`.

```rust
pub struct DesktopSnapshot {
    pub revision: Revision,
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
    pub show_hook_output: bool,
    pub failure: Option<FailureView>,
    pub appearance: Appearance,
    pub motion: ReducedMotion,
    pub language: CatalogLocale,
}
```

### `HostSession`

`HostSession` is added to `tact_host`; it is not a new runtime. It composes the
existing `open_session`, `bootstrap_session`, `run_command_loop_with_account`,
`SessionLockGuard`, `UiResponder`, and account polling.

```rust
pub struct HostSessionConfig {
    pub tact_path: TactPath,
    pub work_dir: PathBuf,
    /// Explicit session choice. Do not synthesize `CliArgs::session` for this:
    /// `args` carries only process flags, so a stray `--session` in argv cannot
    /// pin every project to the first project's session.
    pub session: SessionSelection,
    pub args: CliArgs,
    pub session_store: DynSessionStore,
    pub lock_registry: Arc<SessionLockRegistry>,
    pub progress_capacity: usize,
    pub account_polling: bool,
}

pub enum SessionSelection {
    New,
    Resume(String),
    ResumeLast,
}

pub async fn open_session_selected(
    selection: SessionSelection,
    args: &CliArgs,
    tact_path: &TactPath,
    session_store: &DynSessionStore,
    lock_registry: &SessionLockRegistry,
) -> Result<(String, Arc<SessionLockGuard>)>;

impl HostSession {
    pub async fn start(config: HostSessionConfig) -> Result<Self, HostStartError>;

    pub fn session_id(&self) -> &str;

    /// The controller actor is the only caller. It selects this future against
    /// the shutdown intent; it is never awaited from a detached task.
    pub async fn next_event(&mut self) -> Option<HostEvent>;

    pub fn try_next_event(&mut self) -> TryHostEvent;

    pub fn send(&self, command: UserCommand) -> Result<(), HostSendError>;

    pub fn pending_requests(&self) -> Vec<PendingUiRequest>;

    pub fn extension_host(&self) -> HostExtensionService;

    /// Retryable teardown. `DrainTimedOut` leaves `self` alive and the lock
    /// held, so the caller can wait and retry; it is not a terminal state.
    pub async fn close(&mut self) -> Result<HostExit, HostCloseError>;
}

pub enum HostEvent {
    Agent(AgentUpdate),
    Account(AccountUpdate),
    Ready,
    StartupFailed(HostFailure),
    Disconnected,
}

pub enum HostFailure {
    Config(String),
    Provider(String),
    Persistence(String),
    Bootstrap(String),
    DriverPanicked(String),
}

pub enum TryHostEvent {
    Empty,
    Disconnected,
    Available(HostEvent),
}

pub struct HostExit {
    pub session_id: String,
    pub cancelled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostStartError {
    Config(String),
    Persistence(String),
    Lock(String),
    Bootstrap(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostSendError {
    Closed,
    Disconnected,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostCloseError {
    DrainTimedOut,
    JoinFailed(String),
    LockReleaseFailed(String),
}
```

`HostExtensionService` owns the process-level plugin, marketplace, and skill
registry operations that are not part of one agent activation:

```rust
pub struct HostExtensionService {
    // private skill registry and plugin-home handles
}

pub struct ExtensionSnapshot {
    pub skills: Vec<SkillSnapshot>,
    pub plugins: Vec<PluginSnapshot>,
    pub marketplaces: Vec<MarketplaceSnapshot>,
}

pub struct SkillSnapshot {
    pub name: String,
    pub description: String,
    pub enabled: bool,
    pub path: String,
}

pub struct PluginSnapshot {
    pub name: String,
    pub marketplace: String,
    pub version: Option<String>,
    pub enabled: bool,
}

pub struct MarketplaceSnapshot {
    pub name: String,
    pub source: String,
}

pub enum PluginAction {
    Reload,
    Uninstall { name: String },
    Update { name: String },
    Install { spec: String },
    MarketplaceAdd { source: String },
    MarketplaceUpdate { name: String },
    MarketplaceRemove { name: String },
}

pub enum HostExtensionError {
    Unavailable(String),
    OperationFailed(String),
}

impl HostExtensionService {
    pub async fn snapshot(&self) -> Result<ExtensionSnapshot, HostExtensionError>;

    pub async fn reload_skills(&self) -> Result<ExtensionSnapshot, HostExtensionError>;

    pub async fn set_skill_enabled(
        &self,
        name: &str,
        enabled: bool,
    ) -> Result<ExtensionSnapshot, HostExtensionError>;

    pub async fn plugin_action(
        &self,
        action: PluginAction,
    ) -> Result<ExtensionSnapshot, HostExtensionError>;
}
```

`start` performs the fast part inline: create or resume the row, acquire the
lock, create the bounded progress and reliable terminal channels, create the
command channel and `UiResponder`, then spawn a supervisor. The supervisor
performs provider setup, MCP connection, hook setup, bootstrap, and driver
startup, then emits `Ready`. Buffered commands are consumed when the driver
begins. Startup is cancellable: a `close` that arrives before `Ready` drops the
in-flight bootstrap future (MCP handshakes included) instead of waiting it out.

`close` is the only teardown path and is idempotent/retryable:

1. Signal the supervisor to stop; if bootstrap has not finished, drop it.
2. Send `UserCommand::Cancel`.
3. Close the command sender.
4. Await the supervisor/driver with a bounded grace period.
5. Call `UiResponder::shutdown()` so pending waiters fail instead of hanging.
6. Release the session lock only after the driver is joined.
7. Drop the supervisor channels so `next_event` returns `None`.

If the grace period expires, `close` returns
`HostCloseError::DrainTimedOut` **and keeps `self` alive with the lock held**.
The actor reports "Waiting for session to stop" with a Retry action and never
opens a second runtime over the same session. The binary installs
`SessionLockRegistry::spawn_exit_listener` before the first `start` so a
normal process exit still clears the lock; the stale-lock steal is only the
last resort after a crash.

The TUI interactive path should migrate to `HostSession` after the facade's
focused tests pass. Headless remains a one-shot path because it intentionally
does not own a long-lived command loop. The TUI's "quit during startup" path
maps to `close` before `Ready`, which must return promptly because the
bootstrap future is dropped.

### Protocol additions

The protocol remains the only runtime-to-frontend contract. Four additive
groups are required:

#### Run and message identity

`TactEventBridge` already receives `AgentEvent::RunStarted`, `RunFinished`,
`RunFailed`, and `MessageStarted` but drops them. Emit them as host hints:

```rust
pub enum AgentUpdate {
    // existing variants omitted
    RunStarted { run_id: String },
    RunFinished { run_id: String },
    MessageStarted { message_id: String },
}
```

The tool lifecycle hints `StepFinished`, `StepFailed`, and `ToolProgress` also
gained `run_id: Option<String>`. The option keeps synthetic/legacy producers
compatible, while the real kernel bridge always fills it so a late event from a
completed run cannot mutate a new run that reused the provider's tool id.

`RunStarted` arrives before the first `TurnStats` of an activation, and
`RunFinished` arrives for the matching `RunFinished`/`RunFailed` kernel event.
`RunStarted` is only a *pending* boundary: `/compact` also emits it with no
turns. The reducer promotes a pending run to the active run on the first
`TurnStats`; a `RunFinished` that closes a pending run that never saw
`TurnStats` leaves no transcript row. This keeps compaction from opening a
phantom activation.

`MessageStarted` precedes the first text or thinking delta when the provider
emits it. The kernel accepts a first delta without a prior `MessageStarted`, so
the reducer's fallback assistant row (below) is the compatibility path, not an
exception. The TUI ignores all three variants. The desktop reducer uses them to
key assistant, thinking, and tool-group rows without guessing across Goal
activations, wake-up turns, or subagent traffic. All three hints use the
reliable ordered `send` lane, not the advisory progress lane.

#### Gate purpose and origin

Gate purpose must be semantic, not inferred from prompt text:

```rust
pub enum UiRequestPurpose {
    Permission,
    Question,
    PlanApproval,
    Other,
}

pub enum UiAgentRef {
    Main,
    Subagent { child_id: String },
}

pub struct UiRequestOrigin {
    pub agent: UiAgentRef,
    pub tool_id: Option<String>,
}
```

Add `purpose` and `origin` to `PendingUiRequest`,
`AgentUpdate::RequestSelect`, `AgentUpdate::RequestMultiSelect`, and the two
`UiResponder` registration methods. Existing callers that do not know the
origin use `UiAgentRef::Main` and `tool_id: None`; subagent and permission
paths stamp the exact origin. `PendingUiRequest::from_subagent` stays on the
wire as a compatibility field and is derived as
`matches!(origin.agent, UiAgentRef::Subagent { .. })`; the TUI keeps reading it
unchanged.

`ToolContext` currently exposes `is_subagent: bool` and `session_id`; the
subagent path maps `child_id = ctx.session_id.clone()` and
`tool_id = Some(ctx.tool_id.clone())` (or `None` when the tool has no id).

The responder signatures become:

```rust
impl UiResponder {
    pub fn register_select(
        &self,
        prompt: String,
        options: Vec<String>,
        log_confirm: bool,
        purpose: UiRequestPurpose,
        origin: UiRequestOrigin,
    ) -> (u64, tokio::sync::oneshot::Receiver<UiResponse>);

    pub fn register_multi(
        &self,
        prompt: String,
        options: Vec<String>,
        purpose: UiRequestPurpose,
        origin: UiRequestOrigin,
    ) -> (u64, tokio::sync::oneshot::Receiver<UiResponse>);

    pub async fn request_select(
        &self,
        ui_tx: &impl UpdateDispatch,
        prompt: String,
        options: Vec<String>,
        log_confirm: bool,
        purpose: UiRequestPurpose,
        origin: UiRequestOrigin,
    ) -> Result<Option<usize>, UiRequestError>;

    pub async fn request_multi(
        &self,
        ui_tx: &impl UpdateDispatch,
        prompt: String,
        options: Vec<String>,
        purpose: UiRequestPurpose,
        origin: UiRequestOrigin,
    ) -> Result<Option<Vec<usize>>, UiRequestError>;
}
```

`from_subagent` is not passed separately; the implementation derives it from
`origin` so there is one source of truth.

#### Typed surfaces

The desktop must not parse TUI Markdown to recover data. Add typed queries and
snapshots for live surfaces:

```rust
pub enum SurfaceQuery {
    McpServers,
    McpPrompts { server: Option<String> },
    Hooks,
    Stats,
    SystemPrompt { assembled: bool },
    History,
}

pub enum SurfaceSnapshot {
    McpServers(Vec<McpServerSnapshot>),
    McpPrompts(Vec<McpPromptSnapshot>),
    Hooks(HookSnapshot),
    Stats(StatsSnapshot),
    SystemPrompt(String),
    History(Vec<HistoryEntrySnapshot>),
}

pub struct McpServerSnapshot {
    pub name: String,
    pub transport: String,
    pub source: String,
    pub status: McpServerStatus,
}

pub enum McpServerStatus {
    Connected { tools: usize },
    NeedsAuthorization,
    NotConnected,
    Disabled,
}

pub struct McpPromptSnapshot {
    pub server: String,
    pub name: String,
    pub description: Option<String>,
}

pub struct HookSnapshot {
    pub trusted: Vec<HookItemSnapshot>,
    pub pending: Vec<HookItemSnapshot>,
}

pub struct HookItemSnapshot {
    pub source: String,
    pub event: String,
    pub matcher: Option<String>,
    pub command: String,
}

pub struct StatsSnapshot {
    pub turns_taken: u32,
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
    pub cache_hit_tokens: u32,
    pub reasoning_tokens: u32,
    pub elapsed_ms: u64,
}

pub struct HistoryEntrySnapshot {
    pub task: String,
    pub timestamp: String,
    pub summary: String,
}

pub enum AgentUpdate {
    // existing variants omitted
    SurfaceSnapshot {
        request_id: u64,
        snapshot: SurfaceSnapshot,
    },
}

pub enum UserCommand {
    // existing variants omitted
    QuerySurface {
        request_id: u64,
        query: SurfaceQuery,
    },
}
```

The existing `PopupMarkdown` variants remain for TUI readers. The desktop uses
typed snapshots for managers and readers, and may render Markdown from a typed
field.

#### Background snapshots

The current `/background` command returns a Markdown dump. The desktop needs a
stable collection and per-task cancellation:

```rust
pub struct BackgroundTaskSnapshot {
    pub id: String,
    pub command: String,
    pub status: BackgroundTaskStatusSnapshot,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub output_tail: String,
    pub output_path: Option<String>,
}

pub enum BackgroundTaskStatusSnapshot {
    Running,
    Completed,
    /// Mirrors the persisted `BackgroundTaskStatus::Error`; do not rename the
    /// stored vocabulary to `Failed`.
    Error,
}

pub enum AgentUpdate {
    // existing variants omitted
    BackgroundTasksChanged { tasks: Vec<BackgroundTaskSnapshot> },
}

pub enum UserCommand {
    // existing variants omitted
    SyncBackgroundTasks,
    CancelBackgroundTask { task_id: String },
}
```

`BackgroundTaskFinished` remains the card-finalization event. The desktop list
uses `BackgroundTasksChanged`; it does not derive status from a transcript row.

The existing `BackgroundManager` stores only records and an id counter; it has
no update sink and no per-task handle. Task 3 therefore extends it explicitly:

- `BackgroundManager` gains an `UpdateSink` (the same pattern as the agent
  bridge) so it can emit a replacement snapshot at task start, finish, error,
  and cancellation.
- `BackgroundManager::start` stores `task_id -> CancelToken` and removes the
  entry on terminal status. `CancelBackgroundTask` cancels only that entry;
  cancelling the session-level token remains the run-cancel path.
- `BackgroundTaskRecord` remains the persisted source; the snapshot projects
  `output` as `output_tail` and keeps `output_path`.

Without both changes the per-task Stop action is not implementable and must not
ship as an inert button.

### Desktop state

The desktop core parses protocol values once at its boundary. The public
desktop snapshot does not expose `AgentUpdate`, `UserCommand`, or SQLite types.

```rust
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ProjectId(String);

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SessionId(String);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct SessionEpoch(u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RequestId(u64);

pub struct SessionView {
    pub id: SessionId,
    pub epoch: SessionEpoch,
    pub title: Arc<str>,
    pub root_dir: PathBuf,
    pub phase: SessionPhase,
    pub locked: bool,
    pub updated_at: DateTime<Utc>,
}

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

pub struct RuntimeView {
    pub run_started_at: Option<DateTime<Utc>>,
    pub current_phase: RunPhase,
    pub latest_tool: Option<Arc<str>>,
    pub turns_taken: u32,
    pub model: Option<Arc<str>>,
    pub usage: UsageView,
    pub queue_paused: bool,
}

pub enum RunPhase {
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

pub struct ProjectView {
    pub id: Option<ProjectId>,
    pub root: Option<PathBuf>,
    pub display_name: Arc<str>,
    pub available: bool,
}

pub struct SessionListState {
    pub project: Option<ProjectId>,
    pub filter: ArchiveFilter,
    pub query: String,
    pub items: Vec<SessionSummaryView>,
    pub selected: Option<SessionId>,
    pub loading: bool,
    pub error: Option<FailureView>,
}

pub struct UsageView {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
    pub cache_hit_tokens: u32,
    pub reasoning_tokens: u32,
    pub context_window: Option<usize>,
}

/// Projection of `AccountUpdate`; the `/balance` reader and title-bar account
/// chip read this. `None` means no query has completed yet.
pub enum AccountView {
    Balance(BalanceView),
    UsageQuota(UsageQuotaView),
    Error(Arc<str>),
}

pub struct BalanceView {
    pub currency: Arc<str>,
    pub total: f64,
    pub granted: Option<f64>,
    pub topped_up: Option<f64>,
}

pub struct UsageQuotaView {
    pub plan: Option<Arc<str>>,
    pub used: Option<u64>,
    pub limit: Option<u64>,
    pub resets_at: Option<Arc<str>>,
}

pub struct ComposerView {
    pub queue: Arc<[QueuedPromptView]>,
    pub model: Arc<str>,
    pub effort: Option<Arc<str>>,
    pub permission_mode: PermissionModeView,
    pub slash_catalog: Arc<CommandCatalogView>,
}

pub struct QueuedPromptView {
    pub id: u64,
    pub text: Arc<str>,
    pub attachments: Arc<[PathBuf]>,
}

pub struct InspectorView {
    pub tab: InspectorTab,
    pub goals: Arc<[GoalView]>,
    pub tasks: Arc<[TaskView]>,
    pub subagents: Arc<[SubagentView]>,
    pub background: Arc<[BackgroundTaskView]>,
    pub context: UsageView,
}

pub enum InspectorTab {
    Run,
    Goals,
    Subagents,
    Context,
}

pub struct CommandCatalogView {
    pub recent: Arc<[CommandId]>,
    pub items: Arc<[CommandSearchHit]>,
}

pub struct ExtensionsView {
    pub tab: ExtensionsTab,
    pub query: String,
    pub snapshot: ExtensionSnapshot,
}

pub enum ExtensionsTab {
    Mcp,
    Skills,
    Plugins,
}

pub struct GoalView {
    pub id: Arc<str>,
    pub title: Arc<str>,
    pub objective: Arc<str>,
    pub status: GoalStatusSnapshot,
    pub phase: GoalPhaseSnapshot,
    pub criteria: Arc<[GoalCriterionSnapshot]>,
    pub acceptance: GoalAcceptanceSnapshot,
    pub budget: Option<GoalRunBudgetSnapshot>,
    pub archived: bool,
    /// Set when a lease was lost or recovery is required; the inspector shows
    /// a paused row and this explanation instead of a silent stall.
    pub recovery: Option<Arc<str>>,
}

pub struct TaskView {
    pub id: u64,
    pub subject: Arc<str>,
    pub status: TaskStatusSnapshot,
    pub owner: Arc<str>,
    pub blocks: Arc<[u64]>,
    pub blocked_by: Arc<[u64]>,
}

pub struct SubagentView {
    pub child_id: Arc<str>,
    pub status: SubagentStatusSnapshot,
    pub summary: Arc<str>,
    pub model: Option<Arc<str>>,
    pub context: UsageView,
    pub elapsed_ms: u64,
    pub last_activity: Option<Arc<str>>,
}

pub struct BackgroundTaskView {
    pub id: Arc<str>,
    pub command: Arc<str>,
    pub status: BackgroundTaskStatusSnapshot,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub output_tail: Arc<str>,
    pub output_path: Option<Arc<str>>,
}
```

`SessionEpoch` increments on every session switch. Gate responses carry the
epoch so a late click from the old session cannot answer a new session's
request.

`ComposerView` contains the queued prompts and model/permission controls. The
in-progress draft, cursor, selection, and mention popup stay in the GPUI shell;
they are not runtime state. On submit, the shell passes an immutable
`Submission` to the core. The core owns the queue because queue advancement
depends on terminal runtime events.

### Transcript identity

Row identity is allocated once and never reused within a controller.

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RowId(u64);

pub struct TranscriptRow {
    pub id: RowId,
    pub ordinal: u64,
    pub loop_id: Option<LoopId>,
    pub body: RowBody,
}

pub enum RowBody {
    User(UserRow),
    Assistant(AssistantRow),
    Thinking(ThinkingRow),
    ToolGroup(ToolGroupRow),
    /// A standalone diff row. Tool-owned diffs stay in `ToolBody::Diff`; this
    /// covers a diff that arrives without a tool card (for example a review
    /// reader) and keeps the PRD's `Diff` row kind representable.
    Diff(DiffRow),
    /// A gate that has no owning loop (plan approval, Goal acceptance). Gates
    /// raised inside a tool loop remain `ToolGroupItem::Gate` so they collapse
    /// with their group.
    Gate(GateCard),
    Notice(NoticeRow),
    HookContext(HookContextRow),
    HookStatus(HookStatusRow),
    Error(ErrorRow),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LoopId(u64);

pub struct DiffRow {
    pub path: Option<Arc<str>>,
    pub patch: Arc<str>,
    pub status: RowStatus,
}

pub struct TranscriptView {
    rows: im::Vector<Arc<TranscriptRow>>,
    revision: Revision,
}

impl TranscriptView {
    pub fn len(&self) -> usize;
    pub fn is_empty(&self) -> bool;
    pub fn get(&self, index: usize) -> Option<Arc<TranscriptRow>>;
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

pub struct UserRow {
    pub text: Arc<str>,
    pub attachments: Arc<[PathBuf]>,
}

pub struct AssistantRow {
    pub markdown: Arc<str>,
    pub status: RowStatus,
    pub suggestions: Arc<[Arc<str>]>,
}

pub struct ThinkingRow {
    pub markdown: Arc<str>,
    pub status: RowStatus,
    pub duration_ms: Option<u64>,
}

pub struct ToolGroupRow {
    pub loop_id: LoopId,
    pub items: Arc<[ToolGroupItem]>,
    pub summary: Arc<str>,
    pub status: RowStatus,
    pub collapsed: bool,
}

pub enum ToolGroupItem {
    Tool(ToolRow),
    Gate(GateCard),
}

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
}

pub enum ToolBody {
    None,
    Text(Arc<str>),
    Code { language: Option<Arc<str>>, text: Arc<str> },
    Diff { path: Option<Arc<str>>, patch: Arc<str> },
}

pub struct ToolOutputView {
    pub preview: Arc<str>,
    pub detail: Option<Arc<str>>,
    pub truncated: bool,
}

pub struct ToolMetaView {
    pub model: Option<Arc<str>>,
    pub token_usage: Option<UsageView>,
    pub context_window: Option<usize>,
    pub task_id: Option<Arc<str>>,
    pub subagent_id: Option<Arc<str>>,
}

pub struct NoticeRow {
    pub text: Arc<str>,
}

pub struct HookContextRow {
    pub source: Option<Arc<str>>,
    pub markdown: Arc<str>,
}

pub struct HookStatusRow {
    pub hook_id: u64,
    pub source: Option<Arc<str>>,
    pub message: Arc<str>,
    pub elapsed_ms: Option<u64>,
}

pub struct ErrorRow {
    pub message: Arc<str>,
    pub retryable: bool,
}

pub struct GateCard {
    pub id: RowId,
    pub request_id: RequestId,
    /// The epoch the gate was rendered under. The controller rejects a
    /// response whose card epoch is not the current epoch; request ids restart
    /// at zero per session and are not globally unique.
    pub session_epoch: SessionEpoch,
    pub purpose: UiRequestPurpose,
    pub kind: GateKind,
    pub prompt: Arc<str>,
    pub options: Arc<[Arc<str>]>,
    pub selection: GateSelection,
    pub origin: GateOrigin,
    pub status: GateState,
}

pub enum GateSelection {
    Single(Option<usize>),
    Multiple(Vec<usize>),
}

pub enum GateOrigin {
    Main,
    Subagent { child_id: Arc<str> },
    Tool { tool_id: Arc<str> },
}

pub enum GateState {
    Pending,
    Submitting,
    Resolved,
    Withdrawn,
    Stale,
}

pub enum GateKind {
    Select,
    MultiSelect,
}
```

The reducer keys live rows as follows:

```text
User row       -> session epoch + submission sequence
Assistant row  -> MessageStarted.message_id
Thinking row   -> MessageStarted.message_id + thinking block index
Tool group     -> RunStarted.run_id + TurnStats.turns_taken
Tool row       -> run identity + tool_id
Gate row       -> request_id
Hook status    -> hook id
Notice/error   -> reducer sequence
```

If a producer omits `MessageStarted`, the reducer creates one fallback
assistant row for the current run and records a protocol warning. It does not
silently merge unrelated messages.

### Transcript projection

```rust
pub(crate) struct TranscriptReducer {
    rows: im::Vector<Arc<TranscriptRow>>,
    by_id: HashMap<RowId, RowLocation>,
    active_run: Option<RunCursor>,
    current_loop: Option<LoopCursor>,
    active_assistant: Option<RowId>,
    active_thinking: Option<RowId>,
    tools: HashMap<ToolKey, ToolLocation>,
    hook_status: HashMap<u64, RowId>,
    gates: HashMap<RequestId, RowId>,
    next_row_id: u64,
}

impl TranscriptReducer {
    pub(crate) fn apply_agent_update(
        &mut self,
        update: AgentUpdate,
        context: &ReduceContext,
    ) -> RowDelta;

    pub(crate) fn reconcile_gates(
        &mut self,
        snapshot: &[PendingUiRequest],
        context: &ReduceContext,
    ) -> RowDelta;

    pub(crate) fn apply_gate_response(
        &mut self,
        request_id: RequestId,
        response: GateResponse,
    ) -> RowDelta;

    pub(crate) fn seed_persisted(
        &mut self,
        messages: &[AgentMessage],
        context: &ReduceContext,
    ) -> RowDelta;
}
```

Projection rules:

- `RunStarted` records a *pending* run cursor keyed by `run_id`.
  `MessageStarted` starts a message cursor.
- The first `TurnStats` after `RunStarted` promotes the pending run to the
  active run and allocates a new `LoopId`; tool calls are grouped by that ID,
  not by adjacency or timestamps.
- `RunFinished` closes the matching run cursor. If a pending run never saw
  `TurnStats` (the `/compact` path), it is dropped without creating a row or
  changing `RuntimeView.current_phase`.
- `StepStarted` creates the tool card and its owning group. `StepFinished`,
  `StepFailed`, and `ToolProgress` carry the originating `run_id` when it is
  available and pair by `(run_id, tool_id)`; an explicit event from a completed
  run is ignored. Unknown IDs append a protocol notice instead of corrupting
  another card.
- A tool row whose run never saw a `TurnStats` is attached to a synthetic
  one-tool group with `loop_id: None`; it is never merged into another run's
  group.
- `ToolProgress` coalesces and appends only within the same
  `(run_id, tool_id)` identity. Progress for an unknown tool or a non-active
  explicit run is ignored.
- `StepFinished` with `presentation.keep_live` leaves the card running until
  `BackgroundTaskFinished` or `SubagentFinished`.
- `ToolMeta`, `ToolTranscript`, and `ToolTranscriptStream` update the same tool
  card by `tool_id`. A subagent transcript is a scoped `TranscriptReducer`
  stored on that card.
- `StreamChunk` appends to the active assistant row. Batches coalesce before
  the reducer sees them. Markdown stays as `Arc<str>`; GPUI renders it through
  `StreamingText` or `TextView::markdown`.
- `ThinkingChunk::Started/Delta/Finished` create, append, and finalize one
  thinking row. A second `Started` finalizes the previous block defensively.
- `Info`, `MdInfo`, `HookContext`, `HookStatus`, `Error`, completion, and
  cancellation create typed notice/error rows. `HookStatus` is rewritten in
  place without changing row order.
- `TasksChanged`, `GoalsChanged`, `SubagentsChanged`, and
  `BackgroundTasksChanged` replace their complete views. They are not appended
  as transcript rows.
- `PopupMarkdown` becomes a reader request effect, not a transcript row.
- `TaskComplete`, `TaskCancelled`, and `TaskIdle` close the active run cursor.
  `TaskCancelled` pauses queue advancement.
- A gate hint that has no active loop becomes `RowBody::Gate`; a gate inside a
  loop is `ToolGroupItem::Gate`. A diff without a tool card becomes
  `RowBody::Diff`.

Persisted history uses the same row grammar:

- `MessageOrigin::User` becomes a user row.
- `MessageOrigin::Assistant` becomes assistant text, thinking blocks, and tool
  uses.
- Tool results pair to tool uses by `tool_use_id`.
- Injected context becomes a hook-context row.
- Goal, continuation, subagent, and compaction origins become typed notice
  rows.
- Tool calls in one persisted assistant message form one tool group.

### Gates

`UiResponder::snapshot()` is the source of truth. The reconciler runs every 250
ms and immediately after any gate hint.

```text
UiResponder::request_select
  -> register pending request
  -> AgentUpdate hint
  -> HostSession next_event
  -> controller calls HostSession::pending_requests()
  -> reducer upserts gates by request_id
  -> shell renders GateCard

shell RespondGate
  -> controller validates session_epoch and request kind
  -> marks card Submitting
  -> HostSession::send(UserCommand::UiResponse)
  -> next snapshot no longer contains request_id
  -> reducer collapses card to Resolved
```

Rules:

- A gate is never created solely from a one-shot event.
- A request present in the snapshot becomes `Pending` if new.
- A locally pending request absent from the snapshot becomes `Withdrawn`.
- A locally answered request absent from the snapshot becomes `Resolved`.
- A stale response returning `false` is a no-op and collapses as `Stale`.
- A response is rejected unless `card.session_epoch == snapshot.session.epoch`.
  `request_id` restarts at zero per session, so an old card's id can collide
  with a live request in the new session; the epoch is the discriminator.
- A session switch clears `pending_gates` and marks every old card `Withdrawn`
  before the new epoch is published; late clicks then fail the epoch check.
- A resolved gate keeps its row and collapses its prompt/options to a compact
  summary line.
- `UiRequestOrigin` supplies the main/subagent and tool attribution. Prompt
  text is never parsed for attribution.

### Projects and sessions

Project discovery is desktop navigation, not a second session database.

```rust
pub struct ProjectRegistry {
    path: PathBuf,
    projects: Vec<ProjectRecord>,
}

pub struct ProjectRecord {
    pub id: ProjectId,
    pub root_dir: PathBuf,
    pub display_name: String,
    pub last_opened_at: DateTime<Utc>,
}

impl ProjectRegistry {
    pub fn load(path: PathBuf) -> Result<Self, ProjectRegistryError>;
    pub fn projects(&self) -> &[ProjectRecord];
    pub fn upsert_folder(&mut self, root: &Path)
        -> Result<ProjectRecord, ProjectRegistryError>;
    pub fn remove(&mut self, id: ProjectId) -> Result<bool, ProjectRegistryError>;
    fn flush_atomic(&self) -> Result<(), ProjectRegistryError>;
}
```

The registry lives in the platform app-data directory, deduplicates by
canonical root, and stores only roots and presentation metadata. Missing roots
remain visible as unavailable until the user removes them. Only folders chosen
by the user enter the registry; Tact does not scan the filesystem.

`SessionCatalog` opens the selected project's SQLite store lazily. Session rows
remain authoritative in `<root>/.tact/tact.db`.

```rust
pub struct SessionCatalog {
    projects: ProjectRegistry,
    summaries: HashMap<ProjectId, Arc<[SessionSummaryView]>>,
}

pub struct SessionSummaryView {
    pub id: SessionId,
    pub project_id: ProjectId,
    pub title: Arc<str>,
    pub archived: bool,
    pub updated_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub message_count: i64,
    pub locked: bool,
}

impl SessionCatalog {
    pub async fn refresh(
        &mut self,
        project: &ProjectRecord,
        filter: ArchiveFilter,
    ) -> Result<Arc<[SessionSummaryView]>, SessionCatalogError>;

    pub fn search(
        &self,
        project_id: ProjectId,
        query: &str,
        filter: ArchiveFilter,
    ) -> Vec<SessionSummaryView>;
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ArchiveFilter {
    #[default]
    Active,
    Archived,
    All,
}
```

`ArchiveFilter` and `SessionListQuery` live in `tact::store` (next to
`SessionSummary` and the `SessionStore` trait), not in `tact_desktop_core`.
A store trait method cannot take a type from a crate that depends on the store.
`tact_desktop_core` re-exports or imports them from `tact`.

The session manager uses `tact_catalog::fuzzy_score` for title, ID prefix, and
recent-use ranking. There is no second fuzzy matcher.

### Session metadata migration

The migration is additive and compatible with existing databases. `SQLite`
does not support `ADD COLUMN IF NOT EXISTS`, and `SqliteSessionStore::new`
runs migrations on every open, so the implementation must probe
`PRAGMA table_info(sessions)` and only then `ALTER` (the existing
`add_nullable_column` helper already does this). A bare `ALTER` would make the
second launch fail with `duplicate column name`.

```text
if !has_column(sessions, "title")        -> ALTER TABLE sessions ADD COLUMN title TEXT
if !has_column(sessions, "archived_at")  -> ALTER TABLE sessions ADD COLUMN archived_at TIMESTAMP
CREATE INDEX IF NOT EXISTS idx_sessions_root_archived_updated
  ON sessions(root_dir, archived_at, updated_at DESC)
```

`SessionSummary` gains `title`, `archived_at`, and a derived `locked`
bool. `locked` is not a new SQL column. It is projected at read time from the
existing `locked_by` / `lock_epoch` fields: a session is locked when
`locked_by != 0` and the recorded lock epoch still belongs to a live process.
Existing `list_sessions(Some(root_dir))` remains as a compatibility wrapper
over the new filtered query.

```rust
pub struct SessionSummary {
    pub id: String,
    pub root_dir: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub message_count: i64,
    pub title: Option<String>,
    pub archived_at: Option<DateTime<Utc>>,
    pub locked: bool,
}
```

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionListQuery {
    pub root_dir: Option<String>,
    pub archive: ArchiveFilter,
}

async fn list_sessions_filtered(
    &self,
    query: SessionListQuery,
) -> Result<Vec<SessionSummary>>;

async fn set_session_title(
    &self,
    session_id: &str,
    title: Option<&str>,
) -> Result<()>;

async fn set_session_archived(
    &self,
    session_id: &str,
    archived: bool,
) -> Result<()>;
```

Title derivation is a store invariant:

- When `append_agent_message` inserts a `MessageOrigin::User` message and
  `title IS NULL`, set the title from the first text block.
- Collapse whitespace and truncate to 80 Unicode scalar values.
- Do not derive from tool results, injected context, Goal context, subagent
  results, or continuation messages.
- A manual rename writes the title and suppresses later derivation.
- An idempotent backfill runs on store open for legacy sessions with null
  titles.
- An empty derived title remains null; the UI renders "Untitled session".

Archive sets `archived_at = now`; restore sets it to null. Archive does not
change `updated_at`. `ResumeLast` queries `ArchiveFilter::Active` so an
archived session cannot become the resume target.

### Shared command catalog

`tact_catalog` is a leaf crate. It has no renderer, runtime, Tokio, SQLite, or
process dependency.

```rust
pub enum BuiltinCommand {
    Theme,
    Model,
    ModelSubagent,
    Permission,
    ViewSystemPrompt,
    Save,
    Compact,
    Cancel,
    SubagentCancel,
    Quit,
    Help,
    History,
    Skill,
    Plugin,
    Mcp,
    Hooks,
    HookOutput,
    Balance,
    Lang,
    Stats,
    TasksDag,
    Background,
    Goal,
}

pub enum CommandId {
    Builtin(BuiltinCommand),
    BuiltinPath { root: BuiltinCommand, path: &'static [&'static str] },
    Cli(CliEntry),
    Setting(SettingId),
    Composer(ComposerAction),
    DynamicSkill(String),
}

pub struct CommandDescriptor {
    pub id: CommandId,
    pub parent: Option<CommandId>,
    pub children: Vec<CommandId>,
    pub group: CommandGroup,
    pub path: Vec<String>,
    pub label: LocalizedText,
    pub description: LocalizedText,
    pub aliases: Vec<String>,
    pub shortcut: Option<Shortcut>,
    pub args: Vec<ArgumentSpec>,
    pub availability: Availability,
    pub action: CommandAction,
}

pub struct CommandCatalog {
    // private indexes
}

impl CommandCatalog {
    pub fn get(&self, id: &CommandId) -> Option<&CommandDescriptor>;
    pub fn resolve_path(&self, path: &[&str]) -> Option<&CommandDescriptor>;
    pub fn children(&self, parent: &CommandId)
        -> impl Iterator<Item = &CommandDescriptor>;
    pub fn search(&self, query: &str, locale: CatalogLocale)
        -> Vec<CommandSearchHit>;
}

pub fn builtins() -> &'static CommandCatalog;
pub fn fuzzy_score(query: &str, candidate: &str) -> Option<i32>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogLocale {
    En,
    Zh,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalizedText {
    pub en: &'static str,
    pub zh: &'static str,
}

impl LocalizedText {
    pub fn get(self, locale: CatalogLocale) -> &'static str {
        match locale {
            CatalogLocale::En => self.en,
            CatalogLocale::Zh => self.zh,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandGroup {
    Session,
    Run,
    Appearance,
    Account,
    Goals,
    Extensions,
    Readers,
    Composer,
    Cli,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shortcut {
    pub modifiers: &'static str,
    pub key: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArgumentSpec {
    pub name: &'static str,
    pub required: bool,
    pub value: ArgumentValue,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArgumentValue {
    FreeText,
    Choice(ChoiceSource),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChoiceSource {
    Skills,
    Plugins,
    McpServers,
    HookSources,
    Goals,
    FilesAndImages,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Availability {
    Always,
    NeedsAccount,
    NeedsActiveSession,
    NeedsProject,
    NeedsInstalledSkill,
    Disabled(DisabledReason),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DisabledReason {
    MissingAccount,
    NoActiveSession,
    NoProject,
    Unavailable(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandAction {
    Builtin(BuiltinCommand),
    Cli(CliEntry),
    Setting(SettingId),
    Composer(ComposerAction),
    DynamicSkill(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CliEntry {
    Headless,
    UpgradeCheck,
    UpgradeInstall,
    PluginList,
    PluginInstall,
    PluginUninstall,
    PluginUpdate,
    PluginReload,
    MarketplaceAdd,
    MarketplaceList,
    MarketplaceUpdate,
    MarketplaceRemove,
    McpList,
    McpGet,
    McpAdd,
    McpRemove,
    McpAuth,
    McpLogin,
    McpLogout,
    HooksList,
    HooksTrust,
    HooksForget,
    ListSessions,
    ResumeLast,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingId {
    Provider,
    Model,
    ModelSubagent,
    Effort,
    ApiKey,
    BaseUrl,
    MaxTokens,
    ThinkingBudget,
    PermissionMode,
    Notifications,
    ModelContextWindow,
    Theme,
    Language,
    SnapshotMaxItems,
    MicroCompact,
    TokioConsole,
    SkillBodyAutoInject,
    HookOutput,
    ConfigPath,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComposerAction {
    MentionFile,
    AttachImage,
    Voice,
    Queue,
    Stop,
    Compact,
    SessionActions,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandSearchHit {
    pub id: CommandId,
    pub score: i32,
    pub title: LocalizedText,
    pub subtitle: LocalizedText,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectRegistryError {
    Io(String),
    Corrupt(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionCatalogError {
    Store(String),
    ProjectUnavailable(String),
}
```

`BuiltinCommand` and its subcommand tree move from
`crates/tui/src/widgets/state/slash.rs` into `tact_catalog`. The TUI imports the
same enum and tree; its private `SlashCommand` type becomes a compatibility
alias or is removed in the same migration. The TUI keeps an exhaustive
dispatch match.

The catalog also owns:

- `CliEntry`: headless, upgrade, plugin, marketplace, MCP, hooks, session
  listing, resume, and every declared CLI subcommand.
- `/mcp prompts`, `/mcp prompt`, and `/mcp auth` are builtin subcommand paths,
  not CLI entries. They are represented by `CommandId::BuiltinPath`
  descriptors. `McpAuth` is added to `CliEntry` only because the CLI subcommand
  is `mcp login` with a visible `auth` alias; both IDs execute the same
  `UserCommand::McpAuth`.
- One action has one canonical `CommandId`. When a slash subcommand and a CLI
  subcommand are the same operation (`mcp list`, `plugin reload`, `hooks
  trust`), the builtin path is canonical for the Command Center and the CLI
  entry is a route alias, not a second search result. `CommandCatalog::search`
  deduplicates by canonical action and keeps the builtin path's label.
- `SettingId`: config path, provider, model, subagent model, reasoning effort,
  API key, base URL, max tokens, thinking budget, permission mode,
  notifications, context window, snapshot size, micro-compaction, tokio
  diagnostics, skill-body injection, hook-output visibility, theme, and
  language.
- `ComposerAction`: file mention, image attachment, voice input, queue, stop,
  compact, and session actions.

Settings have exactly one of two application modes, and the UI must show it:

| Mode | Settings | Behavior |
|---|---|---|
| Startup-global | config path, provider, API key, base URL, max tokens, context window, snapshot size, micro-compaction, tokio diagnostics, skill-body injection | written to desktop settings and applied on the next app launch; the row says "Requires restart" |
| Live session | model, subagent model, effort, thinking budget, permission mode, notifications, theme, language, hook-output | routed through a `UserCommand` or a shell setting immediately |

`DesktopIntent::SetSetting` never silently pretends a startup-global value took
effect. `HostLaunchOptions` is the startup projection of those values.

Descriptions are localized `LocalizedText` values. The TUI's existing message
catalog may adapt them, but it must not keep a second command-name list.

Source-sync tests:

1. `BuiltinCommand::iter()` equals the catalog's builtin root commands exactly
   once each (23).
2. Every declared subcommand path exists exactly once and has a parent.
3. The TUI dispatch match is exhaustive over `BuiltinCommand`.
4. The desktop router is exhaustive over `CommandAction` and builtin paths.
5. A Clap `CommandFactory` test compares every CLI path and top-level flag with
   `CliEntry` and `SettingId`, with an explicit exception table for selectors
   that are not settings: `--session` and `--resume-last` route to the session
   picker; `--notifications`/`--no-notifications` and
   `--no-micro-compact` are two spellings of one `SettingId`; `--list-sessions`
   is a `CliEntry`.
6. Every catalog descriptor has a desktop route or an explicit non-executable
   reason.
7. `CommandCatalog::search` returns one hit per canonical action, even when a
   builtin path and a CLI alias share an action.

### GPUI shell

The shell is a controlled renderer:

- `PromptBar` owns the native editor, mentions, attachments, model control, and
  submit/cancel events. Tact supplies the nested slash overlay.
- `CommandSearch` receives flat `CommandSearchItem` values with stable
  `CommandId` strings, subtitles, and keywords.
- `Thinking` and `StreamingText` render Markdown through
  `TextView::markdown`.
- `ToolGroup` and `ToolCall` render Tact tool rows. `ApprovalCard`,
  `QuestionFlow`, `ChoiceGroup`, and `Toggle` render gate semantics.
- `ContextMeter`, `MessageQueue`, `AttachmentStrip`, `Suggestions`,
  `SelectionActions`, and `VoiceControls` render controlled snapshots.
  `VoiceControls` is mounted only when the P2 transcription pipeline lands;
  the P0/P1 composer must not show a dead microphone.
- Tact-owned components are the mixed transcript list, Goal panel, Subagent
  drawer, Session Manager, project switcher, and command/slash overlay.
- `gpui_ai::init(cx)` runs once. Every window is rooted in
  `gpui_component::Root`.
- The shell owns a keymap module implementing PRD section 12 (`Cmd+K`, `Cmd+B`,
  `Cmd+Alt+B`, `Cmd+Enter`, `Esc`, `Cmd+.`, `Cmd+1..4`, `Alt+Up/Down`,
  `Space`, `Tab`/`Shift+Tab`). Catalog shortcuts are display metadata; the
  keymap is the executable binding and has its own test.
- Responsive layout is shell-owned: below the three-column breakpoint, the
  session rail and inspector become dismissible overlays with a scrim. Text
  scale 125%/150% is a layout test, not an open question.

The transcript view is a virtualized list keyed by `RowId`. It keeps
`HashMap<RowId, Entity<TranscriptRowView>>` and updates only rows named in
`StateCommit.dirty_rows`. A session switch replaces the row map only after
`SessionOpened`; it does not rebuild the transcript for each stream chunk.

### Keyboard and focus

Desktop shortcuts are native on macOS, Windows, and Linux but preserve the TUI intent:

- `Cmd+K`: Command Center.
- `Cmd+B` / `Cmd+Alt+B`: toggle the session rail / inspector.
- `Cmd+Enter`: submit the composer.
- `Esc`: close the topmost overlay; otherwise clear selection or cancel the
  active request.
- `Cmd+.`: stop the active run after the destructive-action confirmation when
  applicable.
- `Cmd+1..4`: switch Run/Goals/Subagents/Context inspector tabs.
- `Option+Up/Down`: move through queued prompts.
- `Space`: toggle a selected multi-select option.
- `Enter`: activate the focused row, gate action, or command.
- `Tab` / `Shift+Tab`: traverse controls; focus is always visible.

`DesktopRoot` owns a focus-restoration stack. Every overlay stores its trigger
and restores focus on close; a gate response never steals focus from a newer
overlay.

### PRD coverage and deferrals

P0 and P1 are delivery requirements. P2 is polish unless an entry is already
part of the `tact-ui` surface, in which case the desktop must provide a route
even when the renderer is simple.

| PRD surface | Desktop owner | Scope |
|---|---|---|
| Sticky Tasks and Back to bottom | `StickyView` and transcript tail state | P0 |
| Permission and multi-select gates | `GateCard`, `QuestionFlow`, `ChoiceGroup`, `Toggle` | P0 |
| `/tasks-dag` | Tact-owned Task DAG overlay from `TaskView.blocks` / `blocked_by` | P1 entry, list fallback; graph is P2 polish |
| `/balance` | `AccountView` from `AccountUpdate` into an `InsightCard` reader | P1 |
| `/view-system-prompt` | `SurfaceSnapshot::SystemPrompt` reader | P1 |
| `/model-subagent` | model picker variant scoped to the subagent model | P1 |
| `/save` | `DesktopEffect::SaveTranscript` writing a Markdown export | P1 |
| `/help` | `ReaderRef::Help` keyboard-help reader | P1 |
| `/hook-output` | shell visibility toggle persisted through `SettingId::HookOutput` | P1 |
| Suggestions and SelectionActions | `gpui-ai` controlled components | P1 |
| Voice input | `VoiceControls` plus composer transcription state | P2, desktop-finish |
| ANSI output | normalize into text runs; preserve code/evidence rendering | P2 |
| Mermaid diagrams | P2 renderer; fall back to a fenced code block with a notice | P2 |
| SearchResults, ArtifactPanel, DiffTable, ContextCard | P2 readers/panels using the corresponding `gpui-ai` components | P2 |

The `/tasks-dag` entry is required in the Command Center. The first P1 renderer
may show a dependency list when the graph adapter is not yet available; it must
not leave the entry inert.

### Themes, motion, and accessibility

Only two desktop appearances ship:

```rust
pub enum Appearance {
    Light,
    Dark,
}

pub struct ThemeTokens {
    pub background: Hsla,
    pub surface: Hsla,
    pub elevated: Hsla,
    pub text: Hsla,
    pub muted: Hsla,
    pub accent: Hsla,
    pub warning: Hsla,
    pub success: Hsla,
    pub danger: Hsla,
    pub border: Hsla,
    pub code_background: Hsla,
}
```

The desktop stores appearance in `~/.tact/desktop/settings.json` or the
platform app-data equivalent. The TUI's terminal themes remain TUI-owned. An
unsupported imported theme maps to Light or Dark with a one-time notice.

Motion is a typed policy:

```rust
pub enum ReducedMotion {
    System,
    Always,
    Never,
}

pub struct MotionPolicy {
    pub reduce: bool,
    pub enter: Duration,
    pub exit: Duration,
    pub stagger: Duration,
}
```

Reduced motion switches transitions to direct state changes. It does not
disable streaming updates, status glyph changes, or focus movement. Expand and
collapse animations measure the target height first and reverse before hiding
the body.

Accessibility is part of the row model:

- every row exposes a role, label, status, and expanded/collapsed state;
- gates name the requesting agent and the pending action;
- tabs and inspector regions have labels;
- focus restoration uses an explicit stack in `DesktopRoot`;
- color is never the only state carrier;
- every overlay has a keyboard dismissal path.

### Failure model

`FailureView` is the only public failure type. Controller fatals, banners, and
`DesktopSnapshot.failure` all use it. There is no separate `DesktopFailure`.

```rust
pub enum FailureView {
    Startup { message: String },
    SessionLocked { session_id: SessionId, owner: Option<u32> },
    SessionOpen { root: PathBuf, message: String },
    DriverClosed { session_id: SessionId, recoverable: bool },
    RuntimeDisconnected { session_id: SessionId },
    Store { operation: String, message: String },
    GateUnavailable { message: String },
}
```

A runtime disconnect shows a persistent banner with `Retry session` and
`Open another session`. A locked session shows the lock explanation and a
session picker action. A failed provider turn leaves the assistant row failed
with retry. A failed tool row is expanded by default and preserves output. A
lost gate hint is repaired by snapshot reconciliation.

### Configuration boundary

Tact configuration is process-global and installed exactly once. The desktop
binary loads `DesktopConfig` and calls
`DesktopConfig::install_process_config()` before starting the controller. That
method is implemented in `tact_desktop_core` and delegates to the host/config
facade; the GPUI shell does not import `tact` or `tact_host`. Project switching
does not reinstall provider settings.

Consequences:

- One active `HostSession` per controller.
- Project switching is close-then-open; two projects never own live Agents at
  the same time.
- Project-local `config.toml` is not hot-reloaded during the app lifetime. The
  app-level config path and overrides are the source for the session.
- A future per-session configuration refactor is required before project-local
  provider settings can differ without restart.
- Permission mode is a safety control, so it is never inherited silently. After
  every `HostEvent::Ready`, the controller sends the user's current permission
  choice with `UserCommand::SetPermissionMode` before accepting a submit. The
  global config remains the bootstrap default only.
- The desktop binary installs `SessionLockRegistry::spawn_exit_listener` before
  `DesktopController::start`, so a normal process exit clears the lock even if
  the GUI never reaches `HostSession::close`.
- `DesktopConfig::install_process_config()` is called once from `main.rs`.
  `DesktopController` must not call it; a test asserts the installation count
  stays at one across two project switches.

### Settings application

`DesktopIntent::SetSetting` is a real command route, not a store write:

| Setting | Route |
|---|---|
| model | `UserCommand::SetModel` (session), plus desktop settings |
| subagent model | `DesktopConfig::persist_subagent_model` (applies to the next child) |
| effort | `UserCommand::SetReasoningEffort` |
| thinking budget | `UserCommand::SetThinkingBudget` |
| permission mode | `UserCommand::SetPermissionMode` |
| notifications, hook-output, theme, language | shell state + desktop settings |
| config path, provider, API key, base URL, max tokens, context window, snapshot size, micro-compaction, tokio diagnostics, skill-body injection | desktop settings + "Requires restart" marker |

A startup-global setting must never be reported as applied to the live session.
The settings surface reads `DesktopConfig` for those values and marks the row.

This is an explicit v1 tradeoff, not an accidental omission.

## Synthesis decision

The arena used two independent candidates. Candidate 2 became the base because
its `tact_host::HostSession` facade gives runtime ownership and teardown a
single home, and its immutable snapshot/dirty-row commit model gives the GPUI
shell the smallest semantic surface.

Candidate 1 was grafted in two places:

1. kernel-originated `RunStarted` and `MessageStarted` hints replace desktop
   inference of run and message identity;
2. typed `UiRequestOrigin` and `UiRequestPurpose` replace prompt-text
   attribution for gates.

The cross-judge recommended Candidate 2 and the same two grafts. The resulting
shape keeps Candidate 2's host and controller boundaries while adopting
Candidate 1's stronger transcript identity contract.

## Tradeoffs accepted

- We accept an additive protocol wave in exchange for stable row identity,
  typed gates, typed surfaces, and no desktop parsing of TUI Markdown.
- We accept a new `tact_catalog` crate and a TUI migration in exchange for one
  command source and compile-time coverage.
- We accept `HostSession` as a new deep host facade in exchange for one
  teardown order shared by frontends.
- We accept a desktop-owned project registry in exchange for not inventing a
  global runtime database or a second session store.
- We accept in-memory fuzzy ranking over one project's sessions in exchange for
  one matcher shared by sessions, commands, and extensions.
- We accept title derivation as a first-user-prompt heuristic; manual rename
  remains authoritative.
- We accept one active session per controller in exchange for a simple,
  auditable lock and teardown lifecycle.
- We accept process-global Tact configuration for v1 and document the
  project-local config limitation.

## Alternatives considered

- **One `tact-desktop` crate with GPUI and state together.** It compiles faster
  initially, but every reducer test drags GPUI and a GPUI entity can import
  `tact_host` or `UserCommand` by accident. Rejected.
- **Use `gpui-ai::Chat` and `ThreadList` as the primary surfaces.** They cannot
  represent mixed tools, thinking, diffs, gates, and subagent work; `ThreadList`
  lacks project, archive, and fuzzy filters. Rejected.
- **Infer run and tool-group identity from `TurnStats` alone.** Goal
  activations, wake-up turns, and subagent traffic make the boundary ambiguous.
  Rejected in favor of kernel hints.
- **Use a global project/session database.** It duplicates the existing
  per-project store, creates a second lock model, and makes the migration
  global. Rejected.
- **Let desktop parse `PopupMarkdown` for managers.** It turns presentation
  strings into data and couples desktop to TUI rendering. Rejected in favor of
  typed surface snapshots.
- **Reinitialize Tact configuration for every project switch.** The current
  config and provider state are process-global and not safe to replace while
  an Agent or detached task can still observe them. Rejected for v1.

## Open questions and risks

- Can `gpui-ai` row entities and the selected virtual list update one streaming
  Markdown row without rebuilding the full transcript? The first spike must
  prove this.
- Can `ToolContext` stamp exact tool and subagent origin without breaking
  existing subagent context cloning and tests? If not, the first gate slice
  attaches to the current group and the exact origin waits.
- Does the TUI tolerate the new `AgentUpdate` variants without changing its
  visible behavior?
- Should `--list-sessions` show archived sessions by default, or grow an
  explicit `--archived`/`--all` flag?
- Should desktop appearance live only in desktop settings, or should
  `config.example.toml` grow a `[desktop]` section?
- Is one active session per window acceptable for the first release, or does
  the product need multiple independent windows with separate locks?
- Real AccessKit and reduced-motion verification must happen on each target
  machine; the 125%/150% layout assertions run in the shell test suite.

## Testing strategy

Focused tests, in build order:

- `tact_catalog::catalog`
  - `every_builtin_variant_has_one_catalog_spec`
  - `every_cli_entry_has_one_catalog_spec`
  - `every_setting_has_one_catalog_spec`
  - `slash_subcommands_form_a_tree_without_duplicate_siblings`
  - `fuzzy_search_ranks_prefix_then_word_then_subsequence`
  - `command_requirements_disable_unavailable_actions`
  - `search_returns_one_hit_per_canonical_action`
- `tact_catalog` / `tui` compatibility
  - `tui_dispatch_is_exhaustive_over_builtin_catalog`
  - `tui_subcommand_completion_matches_catalog_tree`
  - `cli_tree_and_flags_match_command_catalog`
  - `every_declared_subcommand_has_a_handler`
- `tact` store migration
  - `migration_adds_title_and_archived_at_to_existing_sessions`
  - `migration_is_idempotent_across_two_store_opens`
  - `first_real_user_prompt_derives_title`
  - `injected_context_does_not_derive_title`
  - `tool_result_does_not_derive_title`
  - `manual_title_is_not_overwritten`
  - `archive_filter_returns_active_archived_and_all`
  - `archive_and_restore_are_idempotent`
  - `resume_last_skips_archived_sessions` (lives in `tact_host`, where the
    resume selection is implemented)
- `tact_host::HostSession`
  - `run_started_precedes_first_turn_stats`
  - `run_finished_closes_the_run_cursor`
  - `compaction_run_does_not_open_an_activation`
  - `message_started_precedes_first_stream_chunk_when_provided`
  - `session_switch_cancels_closes_awaits_and_releases_lock`
  - `driver_timeout_does_not_release_the_lock_and_close_is_retryable`
  - `close_during_startup_drops_bootstrap_and_releases`
  - `exit_listener_is_installed_by_desktop_main`
  - `typed_surface_query_preserves_request_id`
  - `background_snapshot_replaces_previous_collection`
  - `cancel_background_task_cancels_only_that_task`
- `tact_desktop_core::transcript`
  - `run_started_and_message_started_keep_stable_rows`
  - `turn_stats_allocates_one_tool_group`
  - `stream_chunks_append_to_one_assistant_row`
  - `thinking_lifecycle_creates_one_row_and_finishes`
  - `tool_progress_batches_do_not_duplicate_tool_rows`
  - `tool_group_includes_gate_and_subagent_rows`
  - `standalone_gate_and_diff_have_top_level_rows`
  - `stale_gate_hint_is_repaired_from_snapshot`
  - `resolved_gate_collapses_without_losing_history`
  - `session_epoch_rejects_stale_gate_response`
  - `back_to_bottom_restores_tail_following`
- `tact_desktop_core::sessions`
  - `project_registry_deduplicates_canonical_roots`
  - `missing_project_remains_visible_unavailable`
  - `session_search_uses_shared_fuzzy_matcher`
  - `archived_session_is_hidden_from_active_filter`
- `tact_desktop_core::controller`
  - `controller_never_reinitializes_process_config`
  - `permission_mode_is_reapplied_after_ready`
  - `startup_global_setting_reports_restart_required`
  - `live_setting_routes_to_user_command`
- `tact-desktop` GPUI smoke
  - `desktop_shell_renders_all_transcript_row_kinds`
  - `reduced_motion_uses_direct_state_changes`
  - `light_and_dark_share_component_geometry`
  - `gate_focus_restores_to_trigger`
  - `session_switch_keeps_composer_focus`
  - `keymap_matches_prd_interaction_table`
  - `narrow_window_moves_panels_to_scrimmed_overlays`
  - `text_scale_125_and_150_remain_usable`
  - `voice_controls_absent_until_p2_pipeline`

Run focused tests while building, widen only after the focused set passes, and
run `scripts/check-rust.sh` once at the end. Docs-only changes use
`python3 scripts/book-check.py book` instead of the Rust gate.

## Implementation slices

1. **Contract and catalog** - add `tact_catalog`, move the slash tree, add
   catalog coverage tests, and prove TUI compatibility.
2. **Host identity** - add `HostSession`, `RunStarted`/`RunFinished`,
   `MessageStarted`, gate purpose/origin, background sink and per-task cancel,
   and typed surface snapshots.
3. **Session metadata** - add the guarded title/archive migration, filtered
   listing, title derivation, and project registry.
4. **Transcript core** - reducer, row identity, tool groups, standalone
   gate/diff rows, history seeding, gate reconciliation, and dirty-row commits.
5. **Controller** - Tokio actor, session switching, epoch-scoped gates, effect
   execution, settings routing, project and session catalog, and failure
   boundaries.
6. **GPUI shell** - title bar, session rail, transcript, composer, inspector,
   sticky tasks, overlays, keymap, responsive layout, and themes.
7. **Parity** - Goals, subagents, background, extensions, readers, slash
   overlay, Command Center, settings, and all `tact-ui` entries.
8. **Finish** - accessibility, motion, i18n, macOS/Windows/Linux packaging, and
   end-to-end parity.

## Delivery verification

The implementation plan carries the acceptance steps. `scripts/check-rust.sh`
runs the workspace tests, desktop dependency guard, desktop parity suite, and
book links; the desktop release workflow builds and smoke-tests native macOS,
Windows, and Linux artifacts.
