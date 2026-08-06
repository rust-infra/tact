//! Configuration management for tact.
//!
//! Merges configuration from two sources (priority: high to low):
//! 1. CLI arguments
//! 2. TOML config file (`.tact/config.toml`, `config.toml`, or `--config`)
//!
//! Resolved settings are stored in a process-global [`ResolvedConfig`] via
//! [`install`] and accessed through [`settings`].

mod cli;
mod instruction_sources;
mod load;
mod persist;
mod resolve;
mod types;

use std::sync::{LazyLock, RwLock};

use clap::Parser;
pub use cli::{CliArgs, CliCommand, MarketplaceSubcommand, PluginSubcommand};
pub use instruction_sources::{InstructionSource, InstructionSources};
pub use types::{
    AgentSettings, AgentTomlConfig, LlmSettings, LlmTomlConfig, ModelProfileToml,
    PermissionTomlConfig, ResolvedConfig, SubagentSettings, SubagentTomlConfig, TactTomlConfig,
    ToolSettings, ToolsTomlConfig, UiSettings, UiTomlConfig, VisionImageSettings,
    VisionImageTomlConfig, VoiceProvider, VoiceSettings, VoiceTomlConfig,
};

use tact_llm::OpenAiReasoningEffort;

static SETTINGS: RwLock<Option<types::ResolvedConfig>> = RwLock::new(None);

/// Built-in model → thinking parameter defaults (fallback when TOML has no
/// entry for a model). TOML `[llm.model_profiles]` entries override these
/// per model / per field (non-empty field wins).
///
/// Tiers follow the official docs:
/// - openai (developers.openai.com/api/docs/guides/reasoning): gpt-5.6 系列,
///   default medium, model-dependent subsets.
/// - deepseek (api-docs.deepseek.com/zh-cn/guides/thinking_mode): low/high/max.
/// - kimi (www.kimi.com/code/docs/kimi-code/models.html): k3/k3-256k low/high/max;
///   coding 系 Thinking:ON fixed (budget tiers kept for the picker UI only).
static BUILTIN_MODEL_PROFILES: LazyLock<std::collections::HashMap<String, ModelProfileToml>> =
    LazyLock::new(|| {
        use OpenAiReasoningEffort as E;
        let mut m = std::collections::HashMap::new();
        m.insert(
            "gpt-5.6".into(),
            ModelProfileToml {
                thinking_budgets: vec![],
                reasoning_efforts: vec![E::Low, E::Medium, E::High],
            },
        );
        m.insert(
            "gpt-5.6-luna".into(),
            ModelProfileToml {
                thinking_budgets: vec![],
                reasoning_efforts: vec![E::Low, E::Medium],
            },
        );
        m.insert(
            "gpt-5.6-terra".into(),
            ModelProfileToml {
                thinking_budgets: vec![],
                reasoning_efforts: vec![E::Low, E::Medium],
            },
        );
        m.insert(
            "gpt-5.6-sol".into(),
            ModelProfileToml {
                thinking_budgets: vec![],
                reasoning_efforts: vec![E::Medium, E::High, E::Max],
            },
        );
        m.insert(
            "gpt-4o".into(),
            ModelProfileToml {
                thinking_budgets: vec![],
                reasoning_efforts: vec![E::Low, E::Medium, E::High],
            },
        );
        m.insert(
            "deepseek-v4-flash".into(),
            ModelProfileToml {
                thinking_budgets: vec![],
                reasoning_efforts: vec![E::Low, E::High, E::Max],
            },
        );
        m.insert(
            "deepseek-v4-pro".into(),
            ModelProfileToml {
                thinking_budgets: vec![],
                reasoning_efforts: vec![E::High, E::Max],
            },
        );
        m.insert(
            "deepseek-reasoner".into(),
            ModelProfileToml {
                thinking_budgets: vec![],
                reasoning_efforts: vec![E::High, E::Max],
            },
        );
        m.insert(
            "k3".into(),
            ModelProfileToml {
                thinking_budgets: vec![],
                reasoning_efforts: vec![E::Low, E::High, E::Max],
            },
        );
        m.insert(
            "k3-256k".into(),
            ModelProfileToml {
                thinking_budgets: vec![],
                reasoning_efforts: vec![E::Low, E::High, E::Max],
            },
        );
        m.insert(
            "claude-sonnet-4-20250514".into(),
            ModelProfileToml {
                thinking_budgets: vec![0, 8_000, 32_000],
                reasoning_efforts: vec![],
            },
        );
        m.insert(
            "kimi-for-coding".into(),
            ModelProfileToml {
                thinking_budgets: vec![0, 8_000, 32_000],
                reasoning_efforts: vec![],
            },
        );
        m.insert(
            "kimi-for-coding-highspeed".into(),
            ModelProfileToml {
                thinking_budgets: vec![0, 8_000, 32_000],
                reasoning_efforts: vec![],
            },
        );
        m
    });

/// Return a copy of the built-in model profiles (fallback base for TOML merge).
pub fn builtin_model_profiles() -> std::collections::HashMap<String, ModelProfileToml> {
    BUILTIN_MODEL_PROFILES.clone()
}

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
    tact_llm::init_provider(config.llm.provider_info());
    let mut guard = SETTINGS.write().expect("tact config lock poisoned");
    *guard = Some(config);
}

/// Access installed settings if present (TUI unit tests may run without `install`).
pub fn try_settings() -> Option<types::ResolvedConfig> {
    SETTINGS.read().ok()?.as_ref().cloned()
}

/// Update the in-memory active model (keeps status/help in sync; the running
/// agent is updated via `UserCommand::SetModel`).
pub fn update_llm_model(model: String) {
    let mut guard = SETTINGS.write().expect("tact config lock poisoned");
    if let Some(cfg) = guard.as_mut() {
        cfg.llm.model = model.clone();
        cfg.agent.model = model;
    }
}

/// Update the in-memory active model and reasoning effort for this session.
///
/// Mirrors [`update_llm_model_and_thinking_budget`] for effort-semantic
/// providers: both fields must move together so the running agent, status bar,
/// and config-level `agent.reasoning_effort` stay consistent. Effort and budget
/// semantics are mutually exclusive, so picking an effort clears any stale
/// `thinking_budget` (otherwise the bottom bar would show `think high(32K)`
/// with a meaningless budget for an effort-semantic model).
pub fn update_llm_model_and_reasoning_effort(model: String, effort: Option<OpenAiReasoningEffort>) {
    let mut guard = SETTINGS.write().expect("tact config lock poisoned");
    if let Some(cfg) = guard.as_mut() {
        cfg.llm.model = model.clone();
        cfg.agent.model = model;
        cfg.agent.reasoning_effort = effort;
        cfg.agent.thinking_budget = 0;
    }
}

/// Update the in-memory active model and thinking budget for this session.
///
/// When `thinking_budget` is active and not strictly smaller than `max_tokens`,
/// expands `max_tokens` to `budget + 1` so the session settings match the agent
/// auto-expand rule used by [`crate::agent::Agent::set_thinking_budget`].
/// Budget and effort semantics are mutually exclusive, so setting a budget
/// clears any stale `reasoning_effort`.
pub fn update_llm_model_and_thinking_budget(model: String, thinking_budget: usize) {
    let mut guard = SETTINGS.write().expect("tact config lock poisoned");
    if let Some(cfg) = guard.as_mut() {
        cfg.llm.model = model.clone();
        cfg.agent.model = model;
        cfg.agent.thinking_budget = thinking_budget;
        cfg.agent.reasoning_effort = None;
        if thinking_budget > 0 && (cfg.agent.max_tokens as usize) <= thinking_budget {
            cfg.agent.max_tokens = u32::try_from(thinking_budget)
                .unwrap_or(u32::MAX)
                .saturating_add(1);
        }
    }
}

/// Update the in-memory subagent model and thinking budget.
pub fn update_subagent_model(model: String, thinking_budget: usize) {
    let mut guard = SETTINGS.write().expect("tact config lock poisoned");
    if let Some(cfg) = guard.as_mut()
        && let Some(sa) = cfg.agent.subagent.as_mut()
    {
        sa.provider.model = model;
        sa.thinking_budget = thinking_budget;
        sa.reasoning_effort = None;
    }
}

/// Update the in-memory subagent reasoning effort (session level).
pub fn update_subagent_reasoning_effort(effort: Option<OpenAiReasoningEffort>) {
    let mut guard = SETTINGS.write().expect("tact config lock poisoned");
    if let Some(cfg) = guard.as_mut()
        && let Some(sa) = cfg.agent.subagent.as_mut()
    {
        sa.reasoning_effort = effort;
        sa.thinking_budget = 0;
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

/// Persist model and thinking budget under the active provider in the loaded config.
pub fn persist_active_provider_model_and_thinking_budget(
    model: &str,
    thinking_budget: usize,
) -> anyhow::Result<()> {
    let settings = settings();
    let path = settings
        .config_path
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("no config file to update (session-only model change)"))?;
    persist::update_provider_model_and_thinking_budget_in_toml(
        path,
        settings.llm.provider.as_str(),
        model,
        thinking_budget,
    )
}

/// Persist subagent model and thinking budget to the loaded config file.
pub fn persist_subagent_model(model: &str, thinking_budget: usize) -> anyhow::Result<()> {
    let settings = settings();
    let path = settings.config_path.as_ref().ok_or_else(|| {
        anyhow::anyhow!("no config file to update (session-only subagent model change)")
    })?;
    persist::update_subagent_model_in_toml(path, model, thinking_budget)
}

/// Persist model + reasoning effort under the active provider in the loaded config.
///
/// Effort-semantic flows (`/model` effort pick) write `reasoning_effort` instead
/// of `thinking_budget`; the model-level mapping (`[llm.model_profiles]`) stays
/// untouched (it is a static option list, not the current value).
pub fn persist_active_provider_model_and_reasoning_effort(
    model: &str,
    effort: &str,
) -> anyhow::Result<()> {
    let settings = settings();
    let path = settings
        .config_path
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("no config file to update (session-only model change)"))?;
    persist::update_provider_model_and_reasoning_effort_in_toml(
        path,
        settings.llm.provider.as_str(),
        model,
        effort,
    )
}

/// Persist subagent model + reasoning effort to the loaded config file.
pub fn persist_subagent_model_and_reasoning_effort(
    model: &str,
    effort: &str,
) -> anyhow::Result<()> {
    let settings = settings();
    let path = settings.config_path.as_ref().ok_or_else(|| {
        anyhow::anyhow!("no config file to update (session-only subagent model change)")
    })?;
    persist::update_subagent_model_and_reasoning_effort_in_toml(path, model, effort)
}

/// Parse CLI args, load TOML config, merge with priority CLI > TOML, and install
/// the resolved settings for the process.
pub fn init_config() -> anyhow::Result<CliArgs> {
    let args = CliArgs::parse();
    let (toml_cfg, config_path) = load::load_toml_config(args.config.as_ref())?;

    if args.list_sessions
        || matches!(args.command, Some(CliCommand::Plugin { .. }))
        || matches!(args.command, Some(CliCommand::Upgrade { .. }))
    {
        install_without_llm(resolve::resolve_non_llm_settings(
            &args,
            &toml_cfg,
            config_path,
        ));
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
