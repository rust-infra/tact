//! Configuration management for tact.
//!
//! Merges configuration from three sources (priority: high to low):
//! 1. CLI arguments
//! 2. Environment variables
//! 3. TOML config file (`.tact/config.toml` or `$TACT_CONFIG`)
//!
//! After merging, environment variables are set so the rest of the code
//! (which reads from env vars) continues to work unchanged.

use clap::Parser;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

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
    #[arg(short, long, env = "TACT_CONFIG")]
    pub config: Option<PathBuf>,

    /// LLM provider: "anthropic" or "openai"
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

    /// Permission mode: "default", "plan", or "auto" (tact CLI only)
    #[arg(short = 'm', long, env = "TACT_PERMISSION_MODE")]
    pub permission_mode: Option<String>,
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
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct LlmTomlConfig {
    /// Provider name: "anthropic" or "openai"
    pub provider: Option<String>,

    /// Model name
    pub model: Option<String>,

    /// API key (can also be set via env var)
    pub api_key: Option<String>,

    /// API base URL
    pub base_url: Option<String>,
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
}

// ---------------------------------------------------------------------------
// Config loading
// ---------------------------------------------------------------------------

/// Search paths for config file, in order.
fn config_search_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();

    // .tact/config.toml — repo-level config (relative to cwd)
    let cwd = std::env::current_dir().unwrap_or_default();
    paths.push(cwd.join(".tact").join("config.toml"));
    paths.push(cwd.join("tact.toml"));

    // ~/.tact/config.toml — user-level config
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

/// Load TOML config from the given path, or auto-discover.
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

    // Auto-discover
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

/// Merge CLI args, environment variables, and TOML config.
///
/// This function:
/// 1. Parses CLI args via clap
/// 2. Loads TOML config from the specified path or auto-discovery
/// 3. Merges with priority: CLI > env > toml
/// 4. Sets environment variables so existing code works unchanged
///
/// Returns the parsed CLI args.
pub fn init_config() -> CliArgs {
    // SAFETY: set_var is called in single-threaded context at program start,
    // before any other threads are spawned or any env access occurs.
    fn set_env(key: &str, val: &str) {
        unsafe { std::env::set_var(key, val); }
    }

    let args = CliArgs::parse();
    let toml_cfg = load_toml_config(args.config.as_ref());

    // ---- Provider ----
    let provider = args
        .provider
        .clone()
        .or_else(|| std::env::var("TACT_PROVIDER").ok())
        .or_else(|| toml_cfg.llm.provider.clone());
    if let Some(ref p) = provider {
        set_env("TACT_PROVIDER", p);
    }

    // Helper to get the right env var name for the current provider
    let provider_env = |var_base: &str| -> String {
        let prov = provider.as_deref().unwrap_or("openai");
        match prov {
            "anthropic" => format!("ANTHROPIC_{var_base}"),
            _ => format!("OPENAI_{var_base}"),
        }
    };

    // ---- API Key ----
    {
        let env_name = provider_env("API_KEY");
        let api_key = args
            .api_key
            .clone()
            .or_else(|| std::env::var(&env_name).ok())
            .or_else(|| toml_cfg.llm.api_key.clone());
        if let Some(ref key) = api_key {
            set_env(&env_name, key);
        }
    }

    // ---- Model ----
    {
        let env_name = provider_env("MODEL");
        let model = args
            .model
            .clone()
            .or_else(|| std::env::var(&env_name).ok())
            .or_else(|| toml_cfg.llm.model.clone());
        if let Some(ref m) = model {
            set_env(&env_name, m);
        }
    }

    // ---- Base URL ----
    {
        let env_name = provider_env("BASE_URL");
        let base_url = args
            .base_url
            .clone()
            .or_else(|| std::env::var(&env_name).ok())
            .or_else(|| toml_cfg.llm.base_url.clone());
        if let Some(ref url) = base_url {
            set_env(&env_name, url);
        }
    }

    // ---- Permission mode ----
    let perm_mode = args
        .permission_mode
        .clone()
        .or_else(|| std::env::var("TACT_PERMISSION_MODE").ok())
        .or_else(|| toml_cfg.permission.mode.clone());
    if let Some(ref mode) = perm_mode {
        set_env("TACT_PERMISSION_MODE", mode);
    }

    args
}

/// Convenience: initialize config and return CLI args.
/// Call this at the very start of `main()`.
pub fn init() -> CliArgs {
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

[permission]
mode = "auto"

[agent]
context_limit_chars = 500000
"#;
        let cfg: TactTomlConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.llm.provider.as_deref(), Some("openai"));
        assert_eq!(cfg.llm.model.as_deref(), Some("gpt-4o"));
        assert_eq!(cfg.llm.api_key.as_deref(), Some("sk-test"));
        assert!(cfg.llm.base_url.is_some());
        assert_eq!(cfg.permission.mode.as_deref(), Some("auto"));
        assert_eq!(cfg.agent.context_limit_chars, Some(500000));
    }

    #[test]
    fn parse_empty_config() {
        let cfg: TactTomlConfig = toml::from_str("").unwrap();
        assert_eq!(cfg.llm.provider, None);
        assert_eq!(cfg.permission.mode.as_deref(), Some("default"));
        assert_eq!(cfg.agent.context_limit_chars, None);
    }
}
