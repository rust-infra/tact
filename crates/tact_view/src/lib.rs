//! View-layer contract.
//!
//! The commands a View adapter submits ([`UserCommand`]) are Rust view-model
//! types, not the cross-language Plugin Protocol — so they live here rather
//! than in `tact_protocol`. The events a View renders are the protocol's own
//! [`tact_protocol::RuntimeEvent`].

use std::fmt;

use serde::{Deserialize, Serialize};
use tact_protocol::RuntimeCommand;

/// Error classification — lets the TUI distinguish fatal errors (displayed as ❌ Error)
/// from non-fatal situations (shown as Info).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", content = "message", rename_all = "snake_case")]
pub enum AgentErrorKind {
    /// Generic error (catch-all)
    Other(String),
}

impl fmt::Display for AgentErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AgentErrorKind::Other(msg) => f.write_str(msg),
        }
    }
}

impl std::error::Error for AgentErrorKind {}

/// User commands sent from the TUI to the Agent.
#[derive(Debug)]
pub enum UserCommand {
    /// Submit a new natural-language task
    SubmitTask(String),
    /// Cancel the current in-flight task by setting `cancel_flag`.
    /// The agent loop exits cooperatively at the next check point and does not
    /// emit `TaskComplete`. The command driver emits [`RuntimeEvent::Cancelled`]
    /// so the TUI can leave the busy state. The next `SubmitTask` clears the flag.
    Cancel,
    /// Compact the session history (triggered by `/compact` slash command).
    /// Runs compaction on the existing context and stops — does not start a
    /// new task.
    Compact,
    /// Query account balance (DeepSeek/Kimi)
    QueryBalance,
    /// Query session statistics (triggered by the /stats command)
    QueryStats,
    /// Query background task status (triggered by the `/background` slash
    /// command). `None` lists all tasks one line per task; `Some(id)` shows a
    /// single task as pretty JSON.
    QueryBackground(Option<String>),
    /// Set the active permission mode.
    /// The TUI sends this after the user picks through the `/permission` popup.
    /// Only affects the in-memory session; config is never written.
    SetPermissionMode(String),
    /// Set the active session's thinking budget for subsequent LLM requests.
    /// The TUI sends this after `/model` budget confirmation; config persistence
    /// is a separate optional local flow.
    SetThinkingBudget(usize),
    /// Set the active agent session's reasoning effort (openai / deepseek / kimi k3).
    /// `Some("low"|"medium"|...)` sets it; `None` clears (wire omits effort).
    /// The TUI sends this after `/model` effort confirmation; config persistence
    /// is a separate optional local flow.
    SetReasoningEffort(Option<String>),
    /// Set the active agent session's model (per-agent, not global).
    /// The TUI sends this after `/model` model confirmation.
    SetModel(String),
    /// A background subagent finished while the parent may be idle. The driver
    /// decides whether to drop it (parent is mid-turn — the result is already
    /// in `pending_subagent_results`) or submit a synthetic wake-up turn.
    SubagentFinishedNotification {
        child_id: String,
        summary: String,
        success: bool,
    },
    /// Cancel a running background subagent (triggered by `/subagent_cancel`
    /// or the TUI tool-card cancel button). The driver flips the child's
    /// cooperative cancel flag and marks its run record Cancelled.
    CancelSubagent { child_id: String },
    /// Run the interactive OAuth authorization flow for a remote MCP server
    /// (triggered by `/mcp auth <server>`). On success the driver reloads the
    /// MCP router so the server becomes usable without restarting.
    McpAuth { server: String },
    /// List the configured MCP servers with their live status (triggered by
    /// `/mcp list`). The driver renders it from the agent's **already-connected**
    /// router, so it never reconnects and cannot disturb in-flight work.
    McpList,
    /// List the prompts connected servers publish (triggered by
    /// `/mcp prompts [server]`). Read-only; the driver answers it from the
    /// agent's **already-connected** router, like [`UserCommand::McpList`].
    McpPrompts { server: Option<String> },
    /// Fetch one MCP prompt and run it (triggered by
    /// `/mcp prompt <server> <name> [key=value …]`). The driver owns the fetch
    /// because only it can see the router, and submits the rendered messages
    /// through the ordinary task path — a prompt is a starting message, not a
    /// new turn shape. An empty `name` is refused by the driver, which is where
    /// "no such prompt" can also be said.
    RunMcpPrompt {
        server: String,
        name: String,
        arguments: std::collections::BTreeMap<String, String>,
    },
    /// List every configured command hook with its review status (triggered by
    /// `/hooks list`). Read-only; the driver answers it because only the Tact
    /// crate can read the hook sources and the review store.
    HooksList,
    /// Approve hooks that are waiting for review (triggered by
    /// `/hooks trust --all` or `/hooks trust --source <label>`). One of the two
    /// is required — approving a hook by accident is the failure the review
    /// step exists to prevent.
    HooksTrust { all: bool, source: Option<String> },
    /// Revoke every hook approval (triggered by `/hooks forget --all`).
    HooksForget,
    /// Client-neutral command emitted by a View adapter during migration.
    Runtime(RuntimeCommand),
}
