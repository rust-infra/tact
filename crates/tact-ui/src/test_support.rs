//! Helpers for tact-ui integration tests (mock LLM + channel harness).

use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

use tact::{
    Agent, AgentSystemPrompt,
    mcp::MCPToolRouter,
    permission::{PermissionManager, PermissionMode},
    store::{DynSessionStore, open_sqlite_session_store},
    tool::{test_support::test_context, toolset},
};
use tact_llm::{LlmProvider, MockClient, ProviderKind};
use tact_protocol::AgentUpdate;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

static WORKSPACE_COUNTER: AtomicU64 = AtomicU64::new(0);

fn unique_workspace_name(prefix: &str) -> String {
    let n = WORKSPACE_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}-{n}")
}

fn default_test_config() -> tact::config::ResolvedConfig {
    tact::config::ResolvedConfig {
        llm: tact::config::LlmSettings {
            provider: ProviderKind::OpenAi,
            protocol: tact_llm::OpenAiProtocol::default(),
            reasoning_effort: None,
            api_key: String::new(),
            base_url: String::new(),
            model: "mock-model".to_string(),
            models: Vec::new(),
        },
        agent: tact::config::AgentSettings {
            model_context_window: 500_000,
            max_tokens: 8192,
            thinking_budget: 0,
            snapshot_max_items: 80,
            notifications_enabled: false,
            micro_compact_enabled: true,
            skill_body_auto_inject: false,
            instruction_sources: tact::config::InstructionSources::default(),
        },
        ui: tact::config::UiSettings {
            theme: "retro".to_string(),
            vision_image: tact::config::VisionImageSettings {
                compress: tact::config::VisionImageSettings::DEFAULT_COMPRESS,
                max_edge: tact::config::VisionImageSettings::DEFAULT_MAX_EDGE,
                jpeg_quality: tact::config::VisionImageSettings::DEFAULT_JPEG_QUALITY,
            },
        },
        tools: tact::config::ToolSettings {
            brave_search_api_key: None,
            bash_timeout_secs: tact::config::ToolSettings::DEFAULT_BASH_TIMEOUT_SECS,
        },
        permission_mode: None,
        tokio_console: false,
        config_path: None,
    }
}

/// Install minimal `tact::config` settings required by non-agent code paths.
///
/// Safe to call multiple times in the same process; later calls override the
/// previous configuration. Agent-loop settings should be passed via
/// [`build_test_agent_with_config`] / [`Agent::with_agent_settings`].
pub fn install_test_config() {
    tact::config::install_or_override(default_test_config());
}

/// Install a custom test configuration.
///
/// Use this when a test needs non-default values (e.g. a tiny context limit to
/// force compaction).
pub fn install_test_config_with(config: tact::config::ResolvedConfig) {
    tact::config::install_or_override(config);
}

/// Build an agent wired to a mock LLM and optional UI update channel.
pub fn build_test_agent(mock: MockClient, ui_tx: Option<UnboundedSender<AgentUpdate>>) -> (Agent, std::path::PathBuf) {
    build_test_agent_with_mode(mock, ui_tx, PermissionMode::Auto)
}

/// Like [`build_test_agent`], but selects the permission mode (Plan / Auto / Default).
pub fn build_test_agent_with_mode(
    mock: MockClient,
    ui_tx: Option<UnboundedSender<AgentUpdate>>,
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
    ui_tx: Option<UnboundedSender<AgentUpdate>>,
    permission_mode: PermissionMode,
    config: &tact::config::ResolvedConfig,
) -> (Agent, std::path::PathBuf) {
    tact::config::install_or_override(config.clone());
    let agent_settings = config.agent.clone();
    let context = test_context(&unique_workspace_name("tact-ui-integration"));
    let work_dir = context.work_dir.clone();

    let mut tool_context = context;
    tool_context.ui_tx = ui_tx.clone();

    let mut agent = Agent::new(
        LlmProvider::Mock(mock),
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

/// Build an agent with a custom MCP router.
///
/// This is useful when integration tests need to exercise `mcp__` prefixed tools
/// without spawning real MCP server child processes.
pub fn build_test_agent_with_mcp(
    mock: MockClient,
    ui_tx: Option<UnboundedSender<AgentUpdate>>,
    permission_mode: PermissionMode,
    mcp_router: MCPToolRouter,
) -> (Agent, std::path::PathBuf) {
    let config = default_test_config();
    tact::config::install_or_override(config.clone());
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

    (agent, work_dir)
}

/// Like [`build_test_agent`], but attaches an in-memory SQLite session store.
pub async fn build_test_agent_with_session(
    mock: MockClient,
    ui_tx: Option<UnboundedSender<AgentUpdate>>,
) -> (Agent, PathBuf, DynSessionStore, String) {
    let config = default_test_config();
    tact::config::install_or_override(config.clone());
    let agent_settings = config.agent.clone();
    let context = test_context(&unique_workspace_name("tact-ui-session"));
    let work_dir = context.work_dir.clone();
    let db_path = work_dir.join(".tact").join("tact.db");
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent).expect("create session db dir");
    }
    let session_store = open_sqlite_session_store(&db_path).await.expect("open test session store");
    let session_id = "integration-session".to_string();
    let root_dir = work_dir.display().to_string();
    session_store.ensure_session_row(&session_id, &root_dir).await.expect("ensure session row");

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

    (agent, work_dir, session_store, session_id)
}

/// `(sender, receiver)` pair for driving `run_command_loop` in tests.
pub fn user_command_channels()
-> (UnboundedSender<tact_protocol::UserCommand>, UnboundedReceiver<tact_protocol::UserCommand>) {
    unbounded_channel()
}

/// Drain all pending updates from the agent channel (non-blocking after idle).
pub async fn collect_updates(rx: &mut UnboundedReceiver<AgentUpdate>) -> Vec<AgentUpdate> {
    let mut updates = Vec::new();
    while let Ok(update) = rx.try_recv() {
        updates.push(update);
    }
    updates
}

/// Drain updates until idle, waiting briefly for in-flight agent work.
pub async fn collect_updates_after(mut rx: UnboundedReceiver<AgentUpdate>) -> Vec<AgentUpdate> {
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
