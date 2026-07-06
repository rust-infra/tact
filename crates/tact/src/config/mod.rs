//! Configuration management for tact.
//!
//! Merges configuration from two sources (priority: high to low):
//! 1. CLI arguments
//! 2. TOML config file (`.tact/config.toml`, `tact.toml`, or `--config`)
//!
//! Resolved settings are stored in a process-global [`ResolvedConfig`] via
//! [`install`] and accessed through [`settings`].

mod cli;
mod load;
mod resolve;
mod types;

pub use cli::{CliArgs, CliCommand};
pub use types::{
    AgentSettings, AgentTomlConfig, LlmSettings, LlmTomlConfig, PermissionTomlConfig,
    ResolvedConfig, TactTomlConfig, ToolSettings, ToolsTomlConfig, UiSettings, UiTomlConfig,
};

use std::sync::OnceLock;

use clap::Parser;

static SETTINGS: OnceLock<types::ResolvedConfig> = OnceLock::new();

/// Install resolved settings for the process. Must be called once at startup.
pub fn install(config: types::ResolvedConfig) {
    tact_llm::init_provider(config.llm.provider_info());
    SETTINGS
        .set(config)
        .expect("tact config must be installed exactly once");
}

/// Install non-LLM settings for commands that never call the model (e.g. `--list-sessions`).
pub fn install_without_llm(config: types::ResolvedConfig) {
    SETTINGS
        .set(config)
        .expect("tact config must be installed exactly once");
}

/// Access the installed runtime settings.
pub fn settings() -> &'static types::ResolvedConfig {
    SETTINGS
        .get()
        .expect("tact config not installed; call tact::config::init() first")
}

/// Parse CLI args, load TOML config, merge with priority CLI > TOML, and install
/// the resolved settings for the process.
pub fn init_config() -> anyhow::Result<CliArgs> {
    let args = CliArgs::parse();
    let toml_cfg = load::load_toml_config(args.config.as_ref())?;

    if args.list_sessions {
        install_without_llm(resolve::resolve_non_llm_settings(&args, &toml_cfg));
        return Ok(args);
    }

    let resolved = resolve::resolve_config(&args, &toml_cfg)?;
    install(resolved);
    Ok(args)
}

/// Convenience: initialize config and return CLI args.
/// Call this at the very start of `main()`.
pub fn init() -> anyhow::Result<CliArgs> {
    init_config()
}
