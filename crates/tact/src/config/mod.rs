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
    VisionImageSettings, VisionImageTomlConfig,
};

use std::sync::RwLock;

use clap::Parser;

static SETTINGS: RwLock<Option<types::ResolvedConfig>> = RwLock::new(None);

/// Install resolved settings for the process. Must be called once at startup.
pub fn install(config: types::ResolvedConfig) {
    tact_llm::init_provider(config.llm.provider_info());
    let mut guard = SETTINGS.write().expect("tact config lock poisoned");
    assert!(
        guard.is_none(),
        "tact config must be installed exactly once"
    );
    *guard = Some(config);
}

/// Install non-LLM settings for commands that never call the model (e.g. `--list-sessions`).
pub fn install_without_llm(config: types::ResolvedConfig) {
    let mut guard = SETTINGS.write().expect("tact config lock poisoned");
    assert!(
        guard.is_none(),
        "tact config must be installed exactly once"
    );
    *guard = Some(config);
}

/// Access the installed runtime settings.
pub fn settings() -> types::ResolvedConfig {
    SETTINGS
        .read()
        .expect("tact config lock poisoned")
        .as_ref()
        .expect("tact config not installed; call tact::config::init() first")
        .clone()
}

/// Install or replace the runtime settings.
///
/// This is only available under the `test-support` feature. It allows tests to
/// use different configurations within the same process for code paths that still
/// read global settings (UI, permissions, tools). Agent-loop settings are snapshotted
/// on each [`crate::Agent`] via [`crate::Agent::with_agent_settings`]; parallel tests
/// must pass per-agent settings rather than relying on this alone.
#[cfg(feature = "test-support")]
pub fn install_or_override(config: types::ResolvedConfig) {
    let mut guard = SETTINGS.write().expect("tact config lock poisoned");
    if guard.is_none() {
        tact_llm::init_provider(config.llm.provider_info());
    }
    *guard = Some(config);
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
