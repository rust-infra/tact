//! Configuration management for tact.
//!
//! Merges configuration from two sources (priority: high to low):
//! 1. CLI arguments
//! 2. TOML config file (`.tact/config.toml`, `tact.toml`, or `--config`)
//!
//! Resolved settings are stored in a process-global [`ResolvedConfig`] via
//! [`install`] and accessed through [`settings`].

use clap::Parser;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::OnceLock;
use tact_llm::ProviderInfo;

static SETTINGS: OnceLock<ResolvedConfig> = OnceLock::new();

// ---------------------------------------------------------------------------
// CLI arguments (shared by all binaries)
// ---------------------------------------------------------------------------

/// tact — terminal-first AI coding agent
#[derive(Parser, Debug)]
#[command(name = "tact", version, about, long_about = None)]
pub struct CliArgs {
    /// The task prompt to execute (non-interactive mode)
    #[arg(default_value = "")]
    pub prompt: String,

    /// Path to a TOML config file
    #[arg(short, long)]
    pub config: Option<PathBuf>,

    /// LLM provider: "anthropic", "openai", or "kimi"
    #[arg(long)]
    pub provider: Option<String>,

    /// Model name (e.g. "claude-sonnet-4-20250514", "gpt-4o")
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

    /// Permission mode: "default", "plan", or "auto" (tact CLI only)
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

    /// Enable desktop notifications (macOS only). Use --no-notifications to disable.
    #[arg(long)]
    pub notifications: Option<bool>,

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
}

// ---------------------------------------------------------------------------
// TOML config structure
// ---------------------------------------------------------------------------

/// Top-level TOML config (`tact.toml` or `.tact/config.toml`).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct TactTomlConfig {
    /// LLM provider configuration
    pub llm: LlmTomlConfig,

    /// Permission settings
    pub permission: PermissionTomlConfig,

    /// Agent settings
    pub agent: AgentTomlConfig,

    /// UI settings
    pub ui: UiTomlConfig,

    /// Tool-specific settings
    pub tools: ToolsTomlConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct LlmTomlConfig {
    /// Provider name: "anthropic", "openai", or "kimi"
    pub provider: Option<String>,

    /// Model name
    pub model: Option<String>,

    /// API key
    pub api_key: Option<String>,

    /// API base URL
    pub base_url: Option<String>,

    /// Maximum tokens to generate per LLM call.
    pub max_tokens: Option<u32>,

    /// Budget tokens for extended thinking (Anthropic-style thinking / Kimi `thinking`).
    pub thinking_budget: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PermissionTomlConfig {
    /// Permission mode: "default", "plan", or "auto"
    pub mode: Option<String>,
}

impl Default for PermissionTomlConfig {
    fn default() -> Self {
        Self {
            mode: Some("default".into()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct AgentTomlConfig {
    /// Maximum context size in characters (for auto-compaction)
    pub context_limit_chars: Option<usize>,

    /// Enable desktop notifications (default: true)
    pub notifications_enabled: Option<bool>,

    /// Max entries in the system-prompt project structure snapshot.
    pub snapshot_max_items: Option<usize>,

    /// Enable micro-compaction of old tool results (default: true)
    pub micro_compact_enabled: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct UiTomlConfig {
    /// Initial TUI theme name (e.g. "retro", "nord", "dark").
    pub theme: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ToolsTomlConfig {
    /// Brave Search API key for the web_search tool.
    pub brave_search_api_key: Option<String>,
}

// ---------------------------------------------------------------------------
// Resolved runtime settings
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct LlmSettings {
    pub provider: String,
    pub api_key: String,
    pub base_url: String,
    pub model: String,
}

impl LlmSettings {
    pub fn provider_info(&self) -> ProviderInfo {
        ProviderInfo {
            api_key: self.api_key.clone(),
            base_url: self.base_url.clone(),
            model: self.model.clone(),
            provider: self.provider.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct AgentSettings {
    pub max_tokens: u32,
    pub thinking_budget: usize,
    pub context_limit_chars: usize,
    pub notifications_enabled: bool,
    pub snapshot_max_items: usize,
    pub micro_compact_enabled: bool,
}

#[derive(Debug, Clone)]
pub struct UiSettings {
    pub theme: String,
}

#[derive(Debug, Clone)]
pub struct ToolSettings {
    pub brave_search_api_key: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ResolvedConfig {
    pub llm: LlmSettings,
    pub agent: AgentSettings,
    pub ui: UiSettings,
    pub tools: ToolSettings,
    pub permission_mode: Option<String>,
    pub tokio_console: bool,
}

/// Install resolved settings for the process. Must be called once at startup.
pub fn install(config: ResolvedConfig) {
    tact_llm::init_provider(config.llm.provider_info());
    SETTINGS
        .set(config)
        .expect("tact config must be installed exactly once");
}

/// Access the installed runtime settings.
pub fn settings() -> &'static ResolvedConfig {
    SETTINGS
        .get()
        .expect("tact config not installed; call tact::config::init() first")
}

// ---------------------------------------------------------------------------
// Config loading
// ---------------------------------------------------------------------------

fn config_search_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();

    let cwd = std::env::current_dir().unwrap_or_default();
    paths.push(cwd.join(".tact").join("config.toml"));
    paths.push(cwd.join("tact.toml"));

    if let Some(home) = dirs_next_home() {
        paths.push(home.join(".tact").join("config.toml"));
    }

    paths
}

fn dirs_next_home() -> Option<PathBuf> {
    std::env::var("HOME")
        .ok()
        .map(PathBuf::from)
        .or_else(|| std::env::var("USERPROFILE").ok().map(PathBuf::from))
}

fn load_toml_config(path: Option<&PathBuf>) -> TactTomlConfig {
    if let Some(p) = path {
        match std::fs::read_to_string(p) {
            Ok(content) => match toml::from_str(&content) {
                Ok(cfg) => {
                    eprintln!("[config] loaded {:?}", p);
                    return cfg;
                }
                Err(e) => {
                    eprintln!("[config] parse error in {:?}: {e}", p);
                }
            },
            Err(e) => {
                eprintln!("[config] cannot read {:?}: {e}", p);
            }
        }
    }

    for p in config_search_paths() {
        if p.exists() {
            if let Ok(content) = std::fs::read_to_string(&p) {
                if let Ok(cfg) = toml::from_str(&content) {
                    eprintln!("[config] loaded {:?}", p);
                    return cfg;
                }
            }
        }
    }

    TactTomlConfig::default()
}

fn default_base_url(provider: &str) -> Option<String> {
    match provider {
        "openai" => Some("https://api.openai.com/v1".to_string()),
        "kimi" => Some("https://api.kimi.com/coding/v1".to_string()),
        _ => None,
    }
}

fn default_model(provider: &str) -> Option<String> {
    match provider {
        "kimi" => Some("kimi-for-coding".to_string()),
        _ => None,
    }
}

fn resolve_provider(args: &CliArgs, toml_cfg: &TactTomlConfig) -> anyhow::Result<String> {
    if let Some(ref p) = args.provider {
        return Ok(p.clone());
    }
    if let Some(ref p) = toml_cfg.llm.provider {
        return Ok(p.clone());
    }
    anyhow::bail!(
        "LLM provider not configured. Set llm.provider in config.toml or pass --provider anthropic|openai|kimi"
    )
}

fn resolve_llm(args: &CliArgs, toml_cfg: &TactTomlConfig) -> anyhow::Result<LlmSettings> {
    let provider = resolve_provider(args, toml_cfg)?;

    let api_key = args
        .api_key
        .clone()
        .or_else(|| toml_cfg.llm.api_key.clone())
        .ok_or_else(|| anyhow::anyhow!("api_key not configured for provider '{provider}'"))?;

    let base_url = args
        .base_url
        .clone()
        .or_else(|| toml_cfg.llm.base_url.clone())
        .or_else(|| default_base_url(&provider))
        .ok_or_else(|| {
            anyhow::anyhow!("base_url not configured for provider '{provider}'")
        })?;

    let model = args
        .model
        .clone()
        .or_else(|| toml_cfg.llm.model.clone())
        .or_else(|| default_model(&provider))
        .unwrap_or_default();

    Ok(LlmSettings {
        provider,
        api_key,
        base_url,
        model,
    })
}

fn resolve_config(args: &CliArgs, toml_cfg: &TactTomlConfig) -> anyhow::Result<ResolvedConfig> {
    let llm = resolve_llm(args, toml_cfg)?;
    let provider_info = llm.provider_info();

    let max_tokens = args
        .max_tokens
        .or(toml_cfg.llm.max_tokens)
        .unwrap_or_else(|| {
            if provider_info.is_kimi_k2x() {
                32_000
            } else {
                8_000
            }
        });

    let thinking_budget = args
        .thinking_budget
        .or(toml_cfg.llm.thinking_budget)
        .unwrap_or(32_000);

    let context_limit_chars = args
        .context_limit_chars
        .or(toml_cfg.agent.context_limit_chars)
        .unwrap_or_else(|| {
            if provider_info.is_kimi_k2x() {
                900_000
            } else {
                500_000
            }
        });

    let notifications_enabled = args
        .notifications
        .or(toml_cfg.agent.notifications_enabled)
        .unwrap_or(true);

    let snapshot_max_items = args
        .snapshot_max_items
        .or(toml_cfg.agent.snapshot_max_items)
        .unwrap_or(80);

    let micro_compact_enabled = if args.no_micro_compact {
        false
    } else {
        toml_cfg.agent.micro_compact_enabled.unwrap_or(true)
    };

    let theme = args
        .theme
        .clone()
        .or_else(|| toml_cfg.ui.theme.clone())
        .unwrap_or_else(|| "retro".to_string());

    let brave_search_api_key = args
        .brave_search_api_key
        .clone()
        .or_else(|| toml_cfg.tools.brave_search_api_key.clone())
        .filter(|k| !k.is_empty());

    let permission_mode = args
        .permission_mode
        .clone()
        .or_else(|| toml_cfg.permission.mode.clone());

    Ok(ResolvedConfig {
        llm,
        agent: AgentSettings {
            max_tokens,
            thinking_budget,
            context_limit_chars,
            notifications_enabled,
            snapshot_max_items,
            micro_compact_enabled,
        },
        ui: UiSettings { theme },
        tools: ToolSettings {
            brave_search_api_key,
        },
        permission_mode,
        tokio_console: args.tokio_console,
    })
}

/// Parse CLI args, load TOML config, merge with priority CLI > TOML, and install
/// the resolved settings for the process.
pub fn init_config() -> anyhow::Result<CliArgs> {
    let args = CliArgs::parse();
    let toml_cfg = load_toml_config(args.config.as_ref());
    let resolved = resolve_config(&args, &toml_cfg)?;
    install(resolved);
    Ok(args)
}

/// Convenience: initialize config and return CLI args.
/// Call this at the very start of `main()`.
pub fn init() -> anyhow::Result<CliArgs> {
    init_config()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_minimal_config() {
        let toml_str = r#"
[llm]
provider = "anthropic"
"#;
        let cfg: TactTomlConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.llm.provider.as_deref(), Some("anthropic"));
        assert_eq!(cfg.llm.model, None);
        assert_eq!(cfg.permission.mode.as_deref(), Some("default"));
    }

    #[test]
    fn parse_full_config() {
        let toml_str = r#"
[llm]
provider = "openai"
model = "gpt-4o"
api_key = "sk-test"
base_url = "https://proxy.example.com/v1"
max_tokens = 16000
thinking_budget = 64000

[permission]
mode = "auto"

[agent]
context_limit_chars = 500000
snapshot_max_items = 120
micro_compact_enabled = false

[ui]
theme = "nord"

[tools]
brave_search_api_key = "bsk-test"
"#;
        let cfg: TactTomlConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.llm.provider.as_deref(), Some("openai"));
        assert_eq!(cfg.llm.model.as_deref(), Some("gpt-4o"));
        assert_eq!(cfg.llm.api_key.as_deref(), Some("sk-test"));
        assert!(cfg.llm.base_url.is_some());
        assert_eq!(cfg.llm.max_tokens, Some(16000));
        assert_eq!(cfg.llm.thinking_budget, Some(64000));
        assert_eq!(cfg.permission.mode.as_deref(), Some("auto"));
        assert_eq!(cfg.agent.context_limit_chars, Some(500000));
        assert_eq!(cfg.agent.snapshot_max_items, Some(120));
        assert_eq!(cfg.agent.micro_compact_enabled, Some(false));
        assert_eq!(cfg.ui.theme.as_deref(), Some("nord"));
        assert_eq!(cfg.tools.brave_search_api_key.as_deref(), Some("bsk-test"));
    }

    #[test]
    fn resolve_config_from_toml() {
        let toml_cfg: TactTomlConfig = toml::from_str(
            r#"
[llm]
provider = "openai"
api_key = "sk-test"
model = "gpt-4o"
"#,
        )
        .unwrap();
        let args = CliArgs {
            prompt: String::new(),
            config: None,
            provider: None,
            model: None,
            api_key: None,
            base_url: None,
            max_tokens: None,
            thinking_budget: None,
            permission_mode: None,
            session: None,
            resume_last: false,
            list_sessions: false,
            notifications: None,
            context_limit_chars: None,
            theme: None,
            snapshot_max_items: None,
            no_micro_compact: false,
            brave_search_api_key: None,
            tokio_console: false,
        };
        let resolved = resolve_config(&args, &toml_cfg).unwrap();
        assert_eq!(resolved.llm.provider, "openai");
        assert_eq!(resolved.llm.api_key, "sk-test");
        assert_eq!(resolved.llm.base_url, "https://api.openai.com/v1");
        assert_eq!(resolved.agent.max_tokens, 8000);
        assert_eq!(resolved.ui.theme, "retro");
        assert!(resolved.agent.micro_compact_enabled);
    }
}
