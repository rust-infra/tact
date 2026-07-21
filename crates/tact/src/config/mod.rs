//! Configuration management for tact.
//!
//! Merges configuration from two sources (priority: high to low):
//! 1. CLI arguments
//! 2. TOML config file (`.tact/config.toml`, `tact.toml`, or `--config`)
//!
//! Resolved settings are stored in a process-global [`ResolvedConfig`] via
//! [`install`] and accessed through [`settings`].

mod cli;
mod instruction_sources;
mod load;
mod persist;
mod resolve;
mod types;

use std::sync::RwLock;

use clap::Parser;
pub use cli::{CliArgs, CliCommand, MarketplaceSubcommand, PluginSubcommand};
pub use instruction_sources::{InstructionSource, InstructionSources};
pub use types::{
    AgentSettings, AgentTomlConfig, LlmSettings, LlmTomlConfig, PermissionTomlConfig, ResolvedConfig, TactTomlConfig,
    ToolSettings, ToolsTomlConfig, UiSettings, UiTomlConfig, VisionImageSettings, VisionImageTomlConfig,
};

static SETTINGS: RwLock<Option<types::ResolvedConfig>> = RwLock::new(None);

/// Install resolved settings for the process. Must be called once at startup.
pub fn install(config: types::ResolvedConfig) {
    tact_llm::init_provider(config.llm.provider_info());
    let mut guard = SETTINGS.write().expect("tact config lock poisoned");
    assert!(guard.is_none(), "tact config must be installed exactly once");
    *guard = Some(config);
}

/// Install non-LLM settings for commands that never call the model (e.g. `--list-sessions`).
pub fn install_without_llm(config: types::ResolvedConfig) {
    let mut guard = SETTINGS.write().expect("tact config lock poisoned");
    assert!(guard.is_none(), "tact config must be installed exactly once");
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
    tact_llm::init_provider(config.llm.provider_info());
    let mut guard = SETTINGS.write().expect("tact config lock poisoned");
    *guard = Some(config);
}

/// Access installed settings if present (TUI unit tests may run without `install`).
pub fn try_settings() -> Option<types::ResolvedConfig> {
    SETTINGS.read().ok()?.as_ref().cloned()
}

/// Update the in-memory active model (keeps status/help in sync with `tact_llm::set_model`).
pub fn update_llm_model(model: String) {
    let mut guard = SETTINGS.write().expect("tact config lock poisoned");
    if let Some(cfg) = guard.as_mut() {
        cfg.llm.model = model;
    }
}

/// Persist `model` under the active `[llm.providers.<name>]` in the loaded config file.
pub fn persist_active_provider_model(model: &str) -> anyhow::Result<()> {
    let settings = settings();
    let path = settings
        .config_path
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("no config file to update (session-only model change)"))?;
    persist::update_provider_model_in_toml(path, settings.llm.provider.as_str(), model)
}

/// Parse CLI args, load TOML config, merge with priority CLI > TOML, and install
/// the resolved settings for the process.
pub fn init_config() -> anyhow::Result<CliArgs> {
    let args = CliArgs::parse();
    let (toml_cfg, config_path) = load::load_toml_config(args.config.as_ref())?;

    if args.list_sessions || matches!(args.command, Some(CliCommand::Plugin { .. })) {
        install_without_llm(resolve::resolve_non_llm_settings(&args, &toml_cfg, config_path));
        return Ok(args);
    }

    let resolved = resolve::resolve_config(&args, &toml_cfg, config_path)?;
    install(resolved);
    Ok(args)
}

/// Convenience: initialize config and return CLI args.
/// Call this at the very start of `main()`.
pub fn init() -> anyhow::Result<CliArgs> {
    init_config()
}
