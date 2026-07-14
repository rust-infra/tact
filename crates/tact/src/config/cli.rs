use clap::{Parser, Subcommand};
use std::path::PathBuf;

/// tact — terminal-first AI coding agent
#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
pub struct CliArgs {
    #[command(subcommand)]
    pub command: Option<CliCommand>,

    /// Path to a TOML config file
    #[arg(short, long)]
    pub config: Option<PathBuf>,

    /// Active LLM provider (`anthropic` | `openai` | `deepseek` | `kimi`); selects `[llm.providers.<name>]`
    #[arg(long)]
    pub provider: Option<String>,

    /// Model name (e.g. "kimi-for-coding", "deepseek-v4-pro", "gpt-4o")
    #[arg(long)]
    pub model: Option<String>,

    /// API key for the provider
    #[arg(long)]
    pub api_key: Option<String>,

    /// Base URL for the provider API
    #[arg(long)]
    pub base_url: Option<String>,

    /// Maximum tokens to generate per LLM call
    #[arg(long)]
    pub max_tokens: Option<u32>,

    /// Budget tokens for extended thinking (Anthropic/Kimi `thinking`)
    #[arg(long)]
    pub thinking_budget: Option<usize>,

    /// Permission mode: "default", "plan", or "auto"
    #[arg(short = 'm', long)]
    pub permission_mode: Option<String>,

    /// Resume a specific session by ID
    #[arg(long = "session")]
    pub session: Option<String>,

    /// Resume the most recent session
    #[arg(long = "resume-last")]
    pub resume_last: bool,

    /// List recent sessions and exit
    #[arg(long = "list-sessions")]
    pub list_sessions: bool,

    /// Enable desktop notifications (macOS only).
    #[arg(long)]
    pub notifications: Option<bool>,

    /// Disable desktop notifications.
    #[arg(long)]
    pub no_notifications: bool,

    /// Soft context limit in characters before auto-compaction is triggered.
    #[arg(long)]
    pub context_limit_chars: Option<usize>,

    /// UI theme name (e.g. "retro", "nord", "dark").
    #[arg(long)]
    pub theme: Option<String>,

    /// Max entries in the system-prompt project structure snapshot.
    #[arg(long)]
    pub snapshot_max_items: Option<usize>,

    /// Disable micro-compaction of old tool results.
    #[arg(long)]
    pub no_micro_compact: bool,

    /// Brave Search API key for the web_search tool.
    #[arg(long)]
    pub brave_search_api_key: Option<String>,

    /// Enable tokio-console debugging subscriber.
    #[arg(long)]
    pub tokio_console: bool,

    /// Auto-inject full skill body into system prompt (default: false).
    #[arg(long)]
    pub skill_body_auto_inject: bool,
}

#[derive(Subcommand, Debug)]
pub enum CliCommand {
    /// Run a single task without the interactive TUI
    Headless {
        /// The task prompt to execute
        prompt: String,
    },
}
