//! Helpers for tact-ui integration tests (mock LLM + channel harness).

use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use tact_extensions::{
    Agent, AgentSystemPrompt,
    mcp::MCPToolRouter,
    permission::{PermissionManager, PermissionMode},
    store::{DynSessionStore, open_sqlite_session_store},
    tool::{test_support::test_context, toolset},
};
use tact_llm::{LlmProvider, MockClient, ProviderKind};
use tact_protocol::RuntimeEvent;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

static WORKSPACE_COUNTER: AtomicU64 = AtomicU64::new(0);

fn unique_workspace_name(prefix: &str) -> String {
    let n = WORKSPACE_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}-{n}")
}

/// Runs `future` to completion on a fresh thread with its own runtime.
///
/// The `build_test_agent*` constructors are synchronous, but building the
/// serving context is async. A scoped thread with its own runtime builds it
/// without the "cannot start a runtime from within a runtime" panic a bare
/// `block_on` would hit inside a `#[tokio::test]`. Same shape as the internal
/// `block_on` in `tact_extensions::tool::test_support::test_context`, which the
/// builders already rely on.
fn block_on<F>(future: F) -> F::Output
where
    F: std::future::Future + Send,
    F::Output: Send,
{
    std::thread::scope(|scope| {
        scope
            .spawn(|| {
                tokio::runtime::Runtime::new()
                    .expect("failed to create tokio runtime")
                    .block_on(future)
            })
            .join()
            .expect("block_on thread panicked")
    })
}

/// Builds the serving context a test agent is attached to.
///
/// Mirrors what production builds: `session_bootstrap::bootstrap_session` for
/// the headless/TUI hosts and the `#[cfg(test)]` `serving_context_with_recorder`
/// in `driver.rs`. The four service slots behind the Router are:
///
/// - `events` — a local [`tact::EventTransport`]. The integration suites assert
///   on the events that reach the agent's own `ui_tx`, so the serving context
///   only needs *a* transport, not the agent's; keeping it local means the
///   `events.*` capabilities are live without doubling the session's stream.
/// - `trajectory` — an in-process
///   [`tact_trajectory::KernelTrajectoryRecorder`], so `trajectory.*` answers
///   instead of claiming a history that was never written (we deliberately do
///   not start the durable SQLite writer here).
/// - `permission` — [`crate::permission::serving_permission`] in
///   [`PermissionMode::Auto`]. It is the *host's* control plane
///   (`HostControlPlanePermission` allows `chat.submit` / `chat.compact` /
///   `runs.start` / `runs.cancel`), independent of the agent's own tool
///   permission mode, so one policy serves `Auto` / `Default` / `Plan` agents
///   alike. Only starting a turn is governed here; the agent keeps its own
///   `PermissionManager` for tools.
/// - `storage` — a second SQLite handle on the same session database, opened
///   inside [`crate::session_bootstrap::build_serving_context`].
///
/// The database lives under the agent's own temp workspace (`work_dir`), so
/// parallel tests never share a file.
async fn build_test_serving_context(work_dir: &Path) -> tact::RuntimeContext {
    let db_path = work_dir.join(".tact").join("tact.db");
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent).expect("create serving-context db dir");
    }
    let store = open_sqlite_session_store(&db_path)
        .await
        .expect("open serving-context session store");
    let transport = tact::EventTransport::new(64);
    let settings = tact_extensions::permission::settings::PermissionSettings::load_from(
        &work_dir.join("settings.json"),
        None,
    );
    let policy = crate::permission::serving_permission(PermissionMode::Auto, settings)
        .expect("serving policy");
    crate::session_bootstrap::build_serving_context(
        &db_path,
        &store,
        &transport,
        std::sync::Arc::new(tact_trajectory::KernelTrajectoryRecorder::default()),
        policy,
        &crate::session_bootstrap::Notices::Stderr,
    )
    .await
}

/// Attaches a serving context to `agent`, whose temp workspace is `work_dir`.
///
/// This is what makes every shared test agent exercisable through the
/// production path: with a serving context present the driver registers the
/// host extensions and routes a submitted turn through `chat.submit` →
/// `runs.start`, instead of its direct `agent_loop` fallback.
fn attach_serving_context(agent: Agent, work_dir: &Path) -> Agent {
    let serving = block_on(build_test_serving_context(work_dir));
    agent.with_serving_context(serving)
}

fn default_test_config() -> tact_extensions::config::ResolvedConfig {
    tact_extensions::config::ResolvedConfig {
        llm: tact_extensions::config::LlmSettings {
            provider: ProviderKind::OpenAi,
            protocol: tact_llm::OpenAiProtocol::default(),
            reasoning_effort: None,
            api_key: String::new(),
            base_url: String::new(),
            model: "mock-model".to_string(),
            models: Vec::new(),
            model_profiles: Default::default(),
            responses_compact_threshold: None,
        },
        agent: tact_extensions::config::AgentSettings {
            model: "mock-model".to_string(),
            reasoning_effort: None,
            model_context_window: 500_000,
            max_tokens: 8192,
            thinking_budget: 0,
            snapshot_max_items: 80,
            notifications_enabled: false,
            max_token_usage_bodies: tact_extensions::store::session_store::MAX_TOKEN_USAGE_BODIES,
            micro_compact_enabled: true,
            memory_enabled: true,
            skill_body_auto_inject: false,
            skill_dirs: Vec::new(),
            instruction_sources: tact_extensions::config::InstructionSources::default(),
            subagent: None,
        },
        ui: tact_extensions::config::UiSettings {
            theme: "retro".to_string(),
            language: "en".to_string(),
            vision_image: tact_extensions::config::VisionImageSettings {
                compress: tact_extensions::config::VisionImageSettings::DEFAULT_COMPRESS,
                max_edge: tact_extensions::config::VisionImageSettings::DEFAULT_MAX_EDGE,
                jpeg_quality: tact_extensions::config::VisionImageSettings::DEFAULT_JPEG_QUALITY,
            },
            hook_output: tact_extensions::config::UiSettings::DEFAULT_HOOK_OUTPUT,
        },
        tools: tact_extensions::config::ToolSettings {
            bash_timeout_secs: tact_extensions::config::ToolSettings::DEFAULT_BASH_TIMEOUT_SECS,
            bash_nice: tact_extensions::config::ToolSettings::DEFAULT_BASH_NICE,
            rtk_filter: false,
            sandbox: false,
        },
        voice: tact_extensions::config::VoiceSettings::disabled_defaults(),
        mcp: tact_extensions::config::McpSettings::default(),
        permission_mode: None,
        tokio_console: false,
        config_path: None,
    }
}

/// Install minimal `tact_extensions::config` settings required by non-agent code paths.
///
/// Safe to call multiple times in the same process; later calls override the
/// previous configuration. Agent-loop settings should be passed via
/// [`build_test_agent_with_config`] / [`Agent::with_agent_settings`].
pub fn install_test_config() {
    tact_extensions::config::install_or_override(default_test_config());
}

/// Install a custom test configuration.
///
/// Use this when a test needs non-default values (e.g. a tiny context limit to
/// force compaction).
pub fn install_test_config_with(config: tact_extensions::config::ResolvedConfig) {
    tact_extensions::config::install_or_override(config);
}

/// Build an agent wired to a mock LLM and optional in-process event channel.
pub fn build_test_agent(
    mock: MockClient,
    ui_tx: Option<UnboundedSender<RuntimeEvent>>,
) -> (Agent, std::path::PathBuf) {
    build_test_agent_with_mode(mock, ui_tx, PermissionMode::Auto)
}

/// Like [`build_test_agent`], but selects the permission mode (Plan / Auto / Default).
pub fn build_test_agent_with_mode(
    mock: MockClient,
    ui_tx: Option<UnboundedSender<RuntimeEvent>>,
    permission_mode: PermissionMode,
) -> (Agent, std::path::PathBuf) {
    let config = default_test_config();
    build_test_agent_with_config(mock, ui_tx, permission_mode, &config)
}

/// Build an agent with an explicit configuration snapshot for the agent loop.
///
/// Installs `config` for global readers (UI/permissions) and attaches
/// `config.agent` to the returned agent so parallel tests do not race.
pub fn build_test_agent_with_config(
    mock: MockClient,
    ui_tx: Option<UnboundedSender<RuntimeEvent>>,
    permission_mode: PermissionMode,
    config: &tact_extensions::config::ResolvedConfig,
) -> (Agent, std::path::PathBuf) {
    build_test_agent_with_provider(LlmProvider::Mock(mock), ui_tx, permission_mode, config)
}

/// Build an agent with an explicit LLM provider and configuration snapshot.
///
/// Installs `config` for global readers (UI/permissions) and attaches
/// `config.agent` to the returned agent so parallel tests do not race.
/// Used by driver tests that need a real protocol adapter (e.g. OpenAI
/// Responses) pointed at a local wiremock server.
///
/// The agent is attached to a serving context (see
/// [`attach_serving_context`]), so a driver built on it takes the production
/// routed path (`chat.submit` → `runs.start`). Tests that must pin the
/// no-serving-context compatibility path use [`build_test_agent_without_serving`].
pub fn build_test_agent_with_provider(
    client: LlmProvider,
    ui_tx: Option<UnboundedSender<RuntimeEvent>>,
    permission_mode: PermissionMode,
    config: &tact_extensions::config::ResolvedConfig,
) -> (Agent, std::path::PathBuf) {
    let (agent, work_dir) = build_test_agent_raw(client, ui_tx, permission_mode, config);
    let agent = attach_serving_context(agent, &work_dir);
    (agent, work_dir)
}

/// Build an agent wired to a mock LLM **without** a serving context — the
/// driver's direct fallback path.
///
/// Identical to [`build_test_agent`] minus the serving context, so a driver
/// built on this agent cannot register the host extensions and instead runs a
/// submitted turn through `agent_loop` directly (the `run_submit` /
/// `TurnEntry::Shared` branch). Kept so the compatibility path does not rot
/// while every other builder's agent exercises the routed one.
pub fn build_test_agent_without_serving(
    mock: MockClient,
    ui_tx: Option<UnboundedSender<RuntimeEvent>>,
    permission_mode: PermissionMode,
) -> (Agent, std::path::PathBuf) {
    let config = default_test_config();
    build_test_agent_raw(LlmProvider::Mock(mock), ui_tx, permission_mode, &config)
}

/// The un-attached agent: the body every builder shares, with no serving
/// context. [`build_test_agent_with_provider`] attaches one; the fallback
/// builder above deliberately does not.
fn build_test_agent_raw(
    client: LlmProvider,
    ui_tx: Option<UnboundedSender<RuntimeEvent>>,
    permission_mode: PermissionMode,
    config: &tact_extensions::config::ResolvedConfig,
) -> (Agent, std::path::PathBuf) {
    tact_extensions::config::install_or_override(config.clone());
    let agent_settings = config.agent.clone();
    let context = test_context(&unique_workspace_name("tact-ui-integration"));
    let work_dir = context.work_dir.clone();

    let mut tool_context = context;
    tool_context.ui_tx = ui_tx.clone();

    let mut agent = Agent::new(
        client,
        tool_context,
        toolset(),
        MCPToolRouter::new(),
        PermissionManager::try_new(permission_mode).expect("permission mode"),
        AgentSystemPrompt::Static("You are a test agent.".to_string()),
    )
    .with_agent_settings(agent_settings);
    if let Some(tx) = ui_tx {
        agent = agent.with_ui_channel(tx);
    }

    (agent, work_dir)
}

/// Build an agent wired to the OpenAI Responses protocol adapter pointed at
/// `base_url` (normally a local wiremock server).
pub fn build_responses_test_agent(
    base_url: &str,
    ui_tx: Option<UnboundedSender<RuntimeEvent>>,
) -> (Agent, std::path::PathBuf) {
    let config = default_test_config();
    build_test_agent_with_provider(
        LlmProvider::OpenAiResponses(tact_llm::openai::responses::OpenAiResponsesAdapter::new(
            "test-key", base_url, None,
        )),
        ui_tx,
        PermissionMode::Auto,
        &config,
    )
}

/// Build an agent with a custom MCP router.
///
/// This is useful when integration tests need to exercise `mcp__` prefixed tools
/// without spawning real MCP server child processes.
pub fn build_test_agent_with_mcp(
    mock: MockClient,
    ui_tx: Option<UnboundedSender<RuntimeEvent>>,
    permission_mode: PermissionMode,
    mcp_router: MCPToolRouter,
) -> (Agent, std::path::PathBuf) {
    let config = default_test_config();
    tact_extensions::config::install_or_override(config.clone());
    let agent_settings = config.agent.clone();
    let context = test_context(&unique_workspace_name("tact-ui-mcp"));
    let work_dir = context.work_dir.clone();

    let mut tool_context = context;
    tool_context.ui_tx = ui_tx.clone();

    let mut agent = Agent::new(
        LlmProvider::Mock(mock),
        tool_context,
        toolset(),
        mcp_router,
        PermissionManager::try_new(permission_mode).expect("permission mode"),
        AgentSystemPrompt::Static("You are a test agent.".to_string()),
    )
    .with_agent_settings(agent_settings);
    if let Some(tx) = ui_tx {
        agent = agent.with_ui_channel(tx);
    }
    let agent = attach_serving_context(agent, &work_dir);

    (agent, work_dir)
}

/// Like [`build_test_agent`], but attaches an in-memory SQLite session store.
pub async fn build_test_agent_with_session(
    mock: MockClient,
    ui_tx: Option<UnboundedSender<RuntimeEvent>>,
) -> (Agent, PathBuf, DynSessionStore, String) {
    let config = default_test_config();
    tact_extensions::config::install_or_override(config.clone());
    let agent_settings = config.agent.clone();
    let context = test_context(&unique_workspace_name("tact-ui-session"));
    let work_dir = context.work_dir.clone();
    let db_path = work_dir.join(".tact").join("tact.db");
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent).expect("create session db dir");
    }
    let session_store = open_sqlite_session_store(&db_path)
        .await
        .expect("open test session store");
    let session_id = "integration-session".to_string();
    let root_dir = work_dir.display().to_string();
    session_store
        .ensure_session_row(&session_id, &root_dir, "")
        .await
        .expect("ensure session row");

    let mut tool_context = context;
    tool_context.ui_tx = ui_tx.clone();

    let mut agent = Agent::new(
        LlmProvider::Mock(mock),
        tool_context,
        toolset(),
        MCPToolRouter::new(),
        PermissionManager::try_new(PermissionMode::Auto).expect("auto permission mode"),
        AgentSystemPrompt::Static("You are a test agent.".to_string()),
    )
    .with_agent_settings(agent_settings)
    .with_session(session_id.clone(), session_store.clone());
    if let Some(tx) = ui_tx {
        agent = agent.with_ui_channel(tx);
    }
    let agent = agent.with_serving_context(build_test_serving_context(&work_dir).await);

    (agent, work_dir, session_store, session_id)
}

/// `(sender, receiver)` pair for driving `run_command_loop` in tests.
pub fn user_command_channels() -> (
    UnboundedSender<tact_view::UserCommand>,
    UnboundedReceiver<tact_view::UserCommand>,
) {
    unbounded_channel()
}

/// Drain all pending events from the agent channel (non-blocking after idle).
pub async fn collect_updates(rx: &mut UnboundedReceiver<RuntimeEvent>) -> Vec<RuntimeEvent> {
    let mut updates = Vec::new();
    while let Ok(update) = rx.try_recv() {
        updates.push(update);
    }
    updates
}

/// Drain events until idle, waiting briefly for in-flight agent work.
pub async fn collect_updates_after(mut rx: UnboundedReceiver<RuntimeEvent>) -> Vec<RuntimeEvent> {
    let mut updates = Vec::new();
    loop {
        match tokio::time::timeout(std::time::Duration::from_millis(50), rx.recv()).await {
            Ok(Some(update)) => updates.push(update),
            Ok(None) => break,
            Err(_) => break,
        }
    }
    while let Ok(update) = rx.try_recv() {
        updates.push(update);
    }
    updates
}
