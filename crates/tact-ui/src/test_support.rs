//! Helpers for tact-ui integration tests (mock LLM + channel harness).

use std::path::PathBuf;
use std::sync::Once;

use tact::{
    Agent, AgentSystemPrompt,
    mcp::MCPToolRouter,
    permission::{PermissionManager, PermissionMode},
    store::{DynSessionStore, open_sqlite_session_store},
    tool::{test_support::test_context, toolset},
};
use tact_llm::{LlmProvider, MockClient};
use tact_protocol::AgentUpdate;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

static INIT_CONFIG: Once = Once::new();

/// Install minimal `tact::config` settings required by `agent_loop`.
pub fn install_test_config() {
    INIT_CONFIG.call_once(|| {
        let config = tact::config::ResolvedConfig {
            llm: tact::config::LlmSettings {
                provider: "mock".to_string(),
                api_key: String::new(),
                base_url: String::new(),
                model: "mock-model".to_string(),
            },
            agent: tact::config::AgentSettings {
                context_limit_chars: 500_000,
                max_tokens: 8192,
                thinking_budget: 0,
                snapshot_max_items: 80,
                notifications_enabled: false,
                micro_compact_enabled: true,
            },
            ui: tact::config::UiSettings {
                theme: "retro".to_string(),
            },
            tools: tact::config::ToolSettings {
                brave_search_api_key: None,
            },
            permission_mode: None,
            tokio_console: false,
        };
        tact::config::install(config);
    });
}

/// Build an agent wired to a mock LLM and optional UI update channel.
pub fn build_test_agent(
    mock: MockClient,
    ui_tx: Option<UnboundedSender<AgentUpdate>>,
) -> (Agent, std::path::PathBuf) {
    install_test_config();
    let context = test_context("tact-ui-integration");
    let work_dir = context.work_dir.clone();

    let mut tool_context = context;
    tool_context.ui_tx = ui_tx.clone();

    let mut agent = Agent::new(
        LlmProvider::Mock(mock),
        tool_context,
        toolset(),
        MCPToolRouter::new(),
        PermissionManager::try_new(PermissionMode::Auto).expect("auto permission mode"),
        AgentSystemPrompt::Static("You are a test agent.".to_string()),
    );
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
    install_test_config();
    let context = test_context("tact-ui-session");
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
        .ensure_session_row(&session_id, &root_dir)
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
    .with_session(Some(session_id.clone()), session_store.clone());
    if let Some(tx) = ui_tx {
        agent = agent.with_ui_channel(tx);
    }

    (agent, work_dir, session_store, session_id)
}

/// `(sender, receiver)` pair for driving `run_command_loop` in tests.
pub fn user_command_channels() -> (
    UnboundedSender<tact_protocol::UserCommand>,
    UnboundedReceiver<tact_protocol::UserCommand>,
) {
    unbounded_channel()
}

/// Drain all pending updates from the agent channel (non-blocking after idle).
pub async fn collect_updates(
    rx: &mut UnboundedReceiver<AgentUpdate>,
) -> Vec<AgentUpdate> {
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
