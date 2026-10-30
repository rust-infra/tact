use std::collections::HashMap;

use tact_llm::{OpenAiProtocol, ProviderInfo, ProviderKind};

use super::{
    cli::CliArgs,
    instruction_sources::InstructionSources,
    types::{
        AgentSettings, GoalSettings, LlmSettings, McpSettings, ResolvedConfig, SubagentSettings,
        TactTomlConfig, ToolSettings, UiSettings, VisionImageSettings, VoiceProvider,
        VoiceSettings,
    },
};
use crate::permission::PermissionMode;

/// Resolves `[goal]`, rejecting explicit zero budgets: a zero would make every
/// activation an immediate no-op, which is a config mistake rather than an
/// intent (the tests set zero on `GoalLimits` directly when they need it).
fn resolve_goal(toml_cfg: &TactTomlConfig) -> anyhow::Result<GoalSettings> {
    let defaults = GoalSettings::default();
    if toml_cfg.goal.max_turns == Some(0) {
        anyhow::bail!("[goal].max_turns must be greater than 0");
    }
    if toml_cfg.goal.max_elapsed_secs == Some(0) {
        anyhow::bail!("[goal].max_elapsed_secs must be greater than 0");
    }
    Ok(GoalSettings {
        max_turns: toml_cfg.goal.max_turns.unwrap_or(defaults.max_turns),
        max_elapsed_secs: toml_cfg
            .goal
            .max_elapsed_secs
            .unwrap_or(defaults.max_elapsed_secs),
    })
}

fn resolve_vision_image(toml_cfg: &TactTomlConfig) -> VisionImageSettings {
    let compress = toml_cfg
        .ui
        .vision_image
        .compress
        .unwrap_or(VisionImageSettings::DEFAULT_COMPRESS);
    let max_edge = toml_cfg
        .ui
        .vision_image
        .max_edge
        .unwrap_or(VisionImageSettings::DEFAULT_MAX_EDGE)
        .clamp(256, 4096);
    let jpeg_quality = toml_cfg
        .ui
        .vision_image
        .jpeg_quality
        .unwrap_or(VisionImageSettings::DEFAULT_JPEG_QUALITY)
        .clamp(1, 100);
    VisionImageSettings {
        compress,
        max_edge,
        jpeg_quality,
    }
}

fn validate_voice_keybind(raw: &str) -> anyhow::Result<()> {
    let lower = raw.trim().to_lowercase();
    if lower.is_empty() {
        anyhow::bail!("voice.voice_keybind must not be empty");
    }
    let parts: Vec<&str> = lower.split('+').collect();
    match parts.len() {
        1 => {
            // Single-key shortcut (no modifier), e.g. "f5", "tab"
            // For now, we only support ctrl+<char> format.
            anyhow::bail!(
                "unsupported voice_keybind '{}': expected format 'ctrl+<char>', e.g. 'ctrl+g'",
                raw
            );
        }
        2 => {
            if parts[0] != "ctrl" {
                anyhow::bail!(
                    "voice_keybind modifier '{}' not supported: only 'ctrl' is allowed, e.g. 'ctrl+g'",
                    parts[0]
                );
            }
            let key = parts[1];
            if key.len() != 1 {
                anyhow::bail!(
                    "voice_keybind key '{}' must be a single character, e.g. 'ctrl+g'",
                    key
                );
            }
            Ok(())
        }
        _ => anyhow::bail!(
            "invalid voice_keybind '{}': expected format 'ctrl+<char>', e.g. 'ctrl+g'",
            raw
        ),
    }
}

/// Resolves `[mcp]` settings.
///
/// An empty or whitespace-only `oauth_client_name` falls back to the default
/// rather than sending a blank name, which every provider would reject.
fn resolve_mcp(toml_cfg: &TactTomlConfig) -> McpSettings {
    let configured = toml_cfg
        .mcp
        .oauth_client_name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty());

    let oauth_client_name = match configured {
        Some(name) => name.to_string(),
        None => McpSettings::DEFAULT_OAUTH_CLIENT_NAME.to_string(),
    };
    if oauth_client_name != McpSettings::TACT_OAUTH_CLIENT_NAME {
        // Worth a line in the log: providers see this name on their consent
        // screens and in their audit trails, so "who did Tact claim to be?" is
        // a question the log should answer.
        tracing::debug!(
            oauth_client_name = %oauth_client_name,
            "MCP OAuth registration identity"
        );
    }
    // The resource tools are Tact's own, so their risk has no server entry to
    // live in. Same vocabulary as `tools.<name>.risk`; an unknown value is
    // warned about and ignored, so one typo cannot fail the whole config.
    let resource_list_risk = parse_mcp_tool_risk(
        toml_cfg.mcp.resource_list_risk.as_deref(),
        "mcp.resource_list_risk",
    );
    let resource_read_risk = parse_mcp_tool_risk(
        toml_cfg.mcp.resource_read_risk.as_deref(),
        "mcp.resource_read_risk",
    );
    let prompt_list_risk = parse_mcp_tool_risk(
        toml_cfg.mcp.prompt_list_risk.as_deref(),
        "mcp.prompt_list_risk",
    );
    let prompt_get_risk = parse_mcp_tool_risk(
        toml_cfg.mcp.prompt_get_risk.as_deref(),
        "mcp.prompt_get_risk",
    );

    McpSettings {
        oauth_client_name,
        resource_list_risk,
        resource_read_risk,
        prompt_list_risk,
        prompt_get_risk,
    }
}

/// Parses one of `[mcp]`'s resource-tool risk keys.
///
/// `None` for absent *or* unrecognised: the caller treats both as "keep the
/// default", and the default is the restrictive one.
fn parse_mcp_tool_risk(
    value: Option<&str>,
    field: &str,
) -> Option<crate::permission::CapabilityRisk> {
    let value = value?;
    match crate::mcp::ToolRisk::parse(value) {
        Some(risk) => Some(risk.to_capability()),
        None => {
            tracing::warn!(
                field,
                value,
                "unknown MCP resource-tool risk (expected read|write|high); ignoring it"
            );
            None
        }
    }
}

fn resolve_voice(toml_cfg: &TactTomlConfig) -> anyhow::Result<VoiceSettings> {
    let enabled = toml_cfg.voice.enabled.unwrap_or(false);
    let provider = toml_cfg.voice.provider.unwrap_or_default();
    let api_key = toml_cfg.voice.api_key.clone().filter(|k| !k.is_empty());
    let base_url = toml_cfg
        .voice
        .base_url
        .clone()
        .filter(|u| !u.trim().is_empty())
        .unwrap_or_else(|| match provider {
            VoiceProvider::WhisperCpp => VoiceSettings::DEFAULT_WHISPER_CPP_BASE_URL.to_string(),
            VoiceProvider::OpenAi => VoiceSettings::DEFAULT_BASE_URL.to_string(),
            VoiceProvider::Google => VoiceSettings::DEFAULT_GOOGLE_BASE_URL.to_string(),
        });
    let model = toml_cfg
        .voice
        .model
        .clone()
        .filter(|m| !m.trim().is_empty())
        .unwrap_or_else(|| match provider {
            VoiceProvider::WhisperCpp => String::new(),
            VoiceProvider::OpenAi => VoiceSettings::DEFAULT_MODEL.to_string(),
            VoiceProvider::Google => VoiceSettings::DEFAULT_GOOGLE_MODEL.to_string(),
        });
    let language = match toml_cfg.voice.language.clone() {
        Some(language) if language.trim().is_empty() => None,
        Some(language) => Some(language),
        None => Some(VoiceSettings::DEFAULT_LANGUAGE.to_string()),
    };
    let max_duration_secs = toml_cfg.voice.max_duration_secs.unwrap_or(match provider {
        VoiceProvider::Google => VoiceSettings::DEFAULT_GOOGLE_MAX_DURATION_SECS,
        VoiceProvider::OpenAi | VoiceProvider::WhisperCpp => {
            VoiceSettings::DEFAULT_MAX_DURATION_SECS
        }
    });
    let max_allowed = match provider {
        VoiceProvider::Google => 60,
        VoiceProvider::OpenAi | VoiceProvider::WhisperCpp => 600,
    };
    if !(1..=max_allowed).contains(&max_duration_secs) {
        anyhow::bail!(
            "voice.max_duration_secs for {provider:?} must be between 1 and {max_allowed} (got {max_duration_secs})"
        );
    }
    let voice_keybind = toml_cfg.voice.voice_keybind.clone();
    if let Some(ref kb) = voice_keybind {
        validate_voice_keybind(kb)?;
    }
    Ok(VoiceSettings {
        enabled,
        provider,
        api_key,
        base_url,
        model,
        language,
        max_duration_secs,
        voice_keybind,
    })
}

fn resolve_provider_kind(
    args: &CliArgs,
    toml_cfg: &TactTomlConfig,
) -> anyhow::Result<ProviderKind> {
    let raw = args
        .provider
        .clone()
        .or_else(|| toml_cfg.llm.provider.clone())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "LLM provider not configured. Set llm.provider in config.toml or pass --provider"
            )
        })?;
    raw.parse::<ProviderKind>().map_err(anyhow::Error::msg)
}

/// Resolve the optional OpenAI Responses `context_management` compaction
/// threshold (tokens).
///
/// The setting is Responses-specific: non-Responses providers always resolve
/// to `None` and their config is unchanged. A configured value must be
/// positive and must leave room for `max_tokens` and 10% safety headroom
/// within the model context window. When omitted for the Responses protocol,
/// the threshold is derived from the window, `max_tokens`, and headroom;
/// a zero window (disabled) resolves to `None`.
pub(crate) fn resolve_responses_compact_threshold(
    configured: Option<u32>,
    protocol: OpenAiProtocol,
    model_context_window: usize,
    max_tokens: u32,
) -> anyhow::Result<Option<u32>> {
    if protocol != OpenAiProtocol::Responses {
        return Ok(None);
    }
    if let Some(configured) = configured {
        if configured == 0 {
            anyhow::bail!("responses_compact_threshold must be positive (got {configured})");
        }
        if model_context_window != 0 {
            let headroom = model_context_window.saturating_mul(10).div_ceil(100);
            let required = u128::from(configured) + u128::from(max_tokens) + headroom as u128;
            if required > model_context_window as u128 {
                anyhow::bail!(
                    "invalid token limits: responses_compact_threshold ({configured}) must leave room for max_tokens ({max_tokens}) and 10% headroom within agent.model_context_window ({model_context_window})"
                );
            }
        }
        return Ok(Some(configured));
    }
    if model_context_window == 0 {
        return Ok(None);
    }
    let headroom = model_context_window.saturating_mul(10).div_ceil(100);
    let threshold = model_context_window
        .saturating_sub(max_tokens as usize)
        .saturating_sub(headroom)
        .max(1);
    u32::try_from(threshold).map(Some).map_err(|_| {
        anyhow::anyhow!("derived responses_compact_threshold ({threshold}) does not fit u32")
    })
}

fn resolve_llm(args: &CliArgs, toml_cfg: &TactTomlConfig) -> anyhow::Result<LlmSettings> {
    let provider = resolve_provider_kind(args, toml_cfg)?;

    let entry = toml_cfg
        .llm
        .providers
        .get(provider.as_str())
        .ok_or_else(|| {
            let have: Vec<_> = toml_cfg.llm.providers.keys().cloned().collect();
            anyhow::anyhow!(
                "provider '{provider}' not found in llm.providers (have: {})",
                if have.is_empty() {
                    "<none>".into()
                } else {
                    have.join(", ")
                }
            )
        })?;

    let api_key = args
        .api_key
        .clone()
        .or_else(|| entry.api_key.clone())
        .filter(|k| !k.is_empty())
        .ok_or_else(|| anyhow::anyhow!("api_key not configured for provider '{provider}'"))?;

    let base_url = args
        .base_url
        .clone()
        .or_else(|| entry.base_url.clone())
        .or_else(|| provider.default_base_url().map(str::to_string))
        .filter(|u| !u.is_empty())
        .ok_or_else(|| anyhow::anyhow!("base_url not configured for provider '{provider}'"))?;

    let model =
        args.model.clone().or_else(|| entry.model.clone()).filter(|m| !m.trim().is_empty()).ok_or_else(|| {
            anyhow::anyhow!(
                "model not configured for provider '{provider}'. Set llm.providers.{provider}.model or pass --model"
            )
        })?;

    let protocol = entry
        .protocol
        .as_deref()
        .unwrap_or(OpenAiProtocol::default().as_str())
        .parse::<OpenAiProtocol>()
        .map_err(anyhow::Error::msg)?;
    if protocol == OpenAiProtocol::Responses
        && !matches!(
            provider,
            ProviderKind::OpenAi | ProviderKind::DeepSeek | ProviderKind::Custom(_)
        )
    {
        anyhow::bail!(
            "protocol 'responses' is only supported for provider 'openai', 'deepseek' or custom OpenAI-compatible providers"
        );
    }

    if protocol == OpenAiProtocol::Responses && provider == ProviderKind::DeepSeek {
        anyhow::bail!(
            "DeepSeek Responses protocol support is incomplete and not yet production-ready.\n\
             Please set protocol = \"openai\" (Chat Completions) for the deepseek provider in config.toml:\n\
               [llm.providers.deepseek]\n\
               protocol = \"openai\"\n\
             Or remove the protocol line (Chat Completions is the default)."
        );
    }
    let reasoning_effort = entry.reasoning_effort;
    if reasoning_effort.is_some() && !provider.is_openai_compatible() {
        anyhow::bail!(
            "reasoning_effort is only supported for provider 'openai', 'deepseek', 'kimi' or custom OpenAI-compatible providers"
        );
    }

    Ok(LlmSettings {
        provider,
        protocol,
        reasoning_effort,
        api_key,
        base_url,
        model,
        models: entry.models.clone(),
        model_profiles: HashMap::new(), // merged by `resolve_config`
        // Filled in by `resolve_config` once max_tokens and the model context
        // window are resolved (needs both for validation/derivation).
        responses_compact_threshold: None,
    })
}

/// Resolve the optional subagent provider configuration from TOML.
///
/// Reads `[agent.subagent]` and validates that the referenced `provider` key
/// exists in `[llm.providers.*]`. Returns `None` when the subagent section is
/// absent (backward compatibility), or when it is present but carries no
/// overrides at all (a leftover header with every key commented out). A section
/// that sets overrides without a `provider` is rejected rather than dropped.
///
/// `main_max_tokens` and `main_thinking_budget` are the main agent's resolved
/// defaults, used as fallback when the subagent does not override them.
fn resolve_subagent(
    toml_cfg: &TactTomlConfig,
    main_max_tokens: u32,
    main_thinking_budget: usize,
    model_context_window: usize,
) -> anyhow::Result<Option<SubagentSettings>> {
    let Some(subagent_cfg) = &toml_cfg.agent.subagent else {
        return Ok(None);
    };
    let Some(provider_name) = &subagent_cfg.provider else {
        // An all-defaults section is a harmless leftover (every key commented
        // out), so it stays a silent no-op. A section that *sets* overrides
        // without a provider is an error: silently dropping them is the
        // "configured but ignored" trap — the user sees a max_tokens or model
        // in the file that never reaches a request.
        if subagent_cfg.model.is_some()
            || subagent_cfg.max_tokens.is_some()
            || subagent_cfg.thinking_budget.is_some()
            || subagent_cfg.reasoning_effort.is_some()
        {
            anyhow::bail!(
                "[agent.subagent] sets overrides but `provider` is missing, so the whole section is ignored.\n\
                 Set `provider` to a key from [llm.providers.*] (e.g. provider = \"deepseek\"), or remove the section."
            );
        }
        return Ok(None);
    };

    // Validate the provider name is a known key in llm.providers
    let provider_kind = provider_name
        .parse::<ProviderKind>()
        .map_err(|e| anyhow::anyhow!("subagent provider '{provider_name}' is not valid: {e}"))?;

    let entry = toml_cfg.llm.providers.get(provider_name).ok_or_else(|| {
        let have: Vec<_> = toml_cfg.llm.providers.keys().cloned().collect();
        anyhow::anyhow!(
            "subagent provider '{provider_name}' not found in llm.providers (have: {})",
            if have.is_empty() {
                "<none>".into()
            } else {
                have.join(", ")
            }
        )
    })?;

    let api_key = entry
        .api_key
        .clone()
        .filter(|k| !k.is_empty())
        .ok_or_else(|| {
            anyhow::anyhow!("api_key missing for subagent provider '{provider_name}'")
        })?;

    let base_url = entry
        .base_url
        .clone()
        .or_else(|| provider_kind.default_base_url().map(str::to_string))
        .filter(|u| !u.is_empty())
        .ok_or_else(|| {
            anyhow::anyhow!("base_url missing for subagent provider '{provider_name}'")
        })?;

    let model = subagent_cfg
        .model
        .clone()
        .or_else(|| entry.model.clone())
        .filter(|m| !m.trim().is_empty())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "model not configured for subagent provider '{provider_name}'. \
                 Set [agent.subagent].model or [llm.providers.{provider_name}].model"
            )
        })?;

    let protocol = entry
        .protocol
        .as_deref()
        .unwrap_or(OpenAiProtocol::default().as_str())
        .parse::<OpenAiProtocol>()
        .map_err(anyhow::Error::msg)?;

    if protocol == OpenAiProtocol::Responses && provider_kind == ProviderKind::DeepSeek {
        anyhow::bail!(
            "subagent: DeepSeek Responses protocol support is incomplete and not yet production-ready.\n\
             Please set protocol = \"openai\" (Chat Completions) for the deepseek provider entry in config.toml:\n\
               [llm.providers.deepseek]\n\
               protocol = \"openai\"\n\
             Or remove the protocol line (Chat Completions is the default)."
        );
    }

    // Resolve reasoning_effort from the referenced provider entry
    let reasoning_effort = entry.reasoning_effort;

    let explicit_max_tokens = subagent_cfg.max_tokens.or(entry.max_tokens);
    let max_tokens = explicit_max_tokens
        .unwrap_or_else(|| inherited_subagent_max_tokens(&provider_kind, &model, main_max_tokens));

    // The subagent's context window follows the subagent's OWN model, not the
    // main agent's: an override like `model = "gpt-4o"` under a 1M main model
    // must not measure against 1M. The same priority is re-applied on a
    // runtime `/model-subagent` switch — see `apply_subagent_model_switch`.
    let subagent_context_window = resolve_subagent_context_window(
        subagent_cfg.model_context_window,
        &model,
        model_context_window,
    );

    // Clamped into the window when inherited, refused when explicit — see
    // `fit_subagent_max_tokens`.
    let max_tokens = fit_subagent_max_tokens(
        max_tokens,
        explicit_max_tokens.is_some(),
        subagent_context_window,
    )?;

    // The budget fits under the POST-clamp `max_tokens` (a clamp must not leave
    // the budget above it), and the budget wins when `max_tokens` is ours to
    // move — see `fit_subagent_thinking_budget`.
    let thinking_budget = subagent_cfg
        .thinking_budget
        .or(entry.thinking_budget)
        .unwrap_or(main_thinking_budget);
    let max_tokens = fit_subagent_thinking_budget(
        max_tokens,
        explicit_max_tokens.is_some(),
        thinking_budget,
        subagent_context_window,
    )?;

    // The subagent reuses the provider entry's configured Responses compact
    // threshold with its own max_tokens and its own context window.
    // A threshold validated against the main agent's max_tokens may not leave
    // room for a larger subagent max_tokens, so the same headroom rule is
    // applied here. When the threshold is omitted, it is derived from the
    // subagent's own max_tokens; non-Responses protocols and zero context
    // windows resolve to `None`.
    let responses_compact_threshold = resolve_responses_compact_threshold(
        entry.responses_compact_threshold,
        protocol,
        subagent_context_window,
        max_tokens,
    )
    .map_err(|error| anyhow::anyhow!("invalid subagent token limits: {error:#}"))?;

    // Subagent effort: subagent's own override wins, then the referenced
    // provider entry's configured effort.
    let subagent_reasoning_effort = subagent_cfg.reasoning_effort.or(reasoning_effort);

    Ok(Some(SubagentSettings {
        provider: ProviderInfo {
            api_key,
            base_url,
            model,
            provider: provider_kind,
            protocol,
            responses_compact_threshold,
        },
        max_tokens,
        max_tokens_override: explicit_max_tokens,
        thinking_budget,
        reasoning_effort: subagent_reasoning_effort,
        model_context_window: subagent_context_window,
        model_context_window_override: subagent_cfg.model_context_window,
        responses_compact_threshold_override: entry.responses_compact_threshold,
        models: entry.models.clone(),
    }))
}

/// Every setting that does not depend on a usable LLM configuration.
///
/// Extracted so the two resolution entry points cannot drift apart:
/// [`resolve_non_llm_settings`] (subcommands that never talk to a model, so an
/// invalid `[llm]` section must not stop them) and [`resolve_config`] (the full
/// path) both take all of these through the same precedence chain —
/// **CLI flag > TOML > built-in default**.
struct NonLlmSettings {
    notifications_enabled: bool,
    snapshot_max_items: usize,
    micro_compact_enabled: bool,
    memory_enabled: bool,
    max_token_usage_bodies: usize,
    skill_body_auto_inject: bool,
    skill_dirs: Vec<String>,
    skill_description_cap: usize,
    skill_disable: Vec<String>,
    instruction_sources: InstructionSources,
    auto_memory_directory: Option<String>,
    theme: String,
    language: String,
    vision_image: VisionImageSettings,
    hook_output: bool,
    bash_timeout_secs: u64,
    bash_nice: i32,
    rtk_filter: bool,
    sandbox: bool,
    permission_mode: PermissionMode,
    mcp: McpSettings,
}

/// Resolves the non-LLM half of the configuration.
///
/// Precedence, per field: an explicit CLI flag wins, then the TOML value, then
/// the built-in default. The two negating flags (`--no-notifications`,
/// `--no-micro-compact`) are absolute: when set they force the feature off and
/// the TOML value is not consulted.
///
/// Returns `Err` for a malformed `[agent].instruction_sources`; callers decide
/// whether that is fatal (it is not for subcommands that never build a prompt).
fn resolve_non_llm(args: &CliArgs, toml_cfg: &TactTomlConfig) -> anyhow::Result<NonLlmSettings> {
    let notifications_enabled = if args.no_notifications {
        false
    } else {
        args.notifications
            .or(toml_cfg.agent.notifications_enabled)
            .unwrap_or(true)
    };

    let snapshot_max_items = args
        .snapshot_max_items
        .or(toml_cfg.agent.snapshot_max_items)
        .unwrap_or(80);

    let micro_compact_enabled = if args.no_micro_compact {
        false
    } else {
        toml_cfg.agent.micro_compact_enabled.unwrap_or(false)
    };

    let memory_enabled = toml_cfg.agent.memory_enabled.unwrap_or(true);

    let max_token_usage_bodies = toml_cfg
        .agent
        .max_token_usage_bodies
        .unwrap_or(crate::store::session_store::MAX_TOKEN_USAGE_BODIES);

    let skill_body_auto_inject =
        args.skill_body_auto_inject || toml_cfg.agent.skill_body_auto_inject.unwrap_or(false);

    let skill_dirs = toml_cfg.agent.skill_dirs.clone();

    let skill_description_cap = toml_cfg
        .agent
        .skill_description_cap
        .unwrap_or(crate::skill::DEFAULT_SKILL_DESCRIPTION_CAP);

    let skill_disable = toml_cfg.agent.skill_disable.clone();

    // Unlike the old `.expect()` here, a bad value is reported to the caller.
    // This is library code reached from `main`, where a panic aborts the
    // process instead of naming the offending config key.
    let instruction_sources =
        InstructionSources::from_config(toml_cfg.agent.instruction_sources.clone())
            .map_err(|e| anyhow::anyhow!("invalid [agent].instruction_sources: {e}"))?;

    // Kept as the raw string: `~` and relative forms need the workdir, which
    // resolution does not have, so expansion happens in `memory::memory_root`.
    let auto_memory_directory = toml_cfg
        .agent
        .auto_memory_directory
        .clone()
        .map(|raw| raw.trim().to_string())
        .filter(|raw| !raw.is_empty());

    let theme = args
        .theme
        .clone()
        .or_else(|| toml_cfg.ui.theme.clone())
        .unwrap_or_else(|| "ink".to_string());

    // No CLI flag: unlike the theme (chosen per terminal, so `--theme` earns
    // its keep) a language belongs to the person, and `/lang` writes it once.
    let language = toml_cfg
        .ui
        .language
        .clone()
        .unwrap_or_else(|| "en".to_string());

    let vision_image = resolve_vision_image(toml_cfg);

    // Display-only switch, so no CLI flag: `/hook-output` writes it, and the
    // file is the only other way in. Absent means shown — a fresh install
    // behaves as it did before the switch existed.
    let hook_output = toml_cfg
        .ui
        .hook_output
        .unwrap_or(UiSettings::DEFAULT_HOOK_OUTPUT);

    let bash_timeout_secs = toml_cfg
        .tools
        .bash_timeout_secs
        .unwrap_or(ToolSettings::DEFAULT_BASH_TIMEOUT_SECS);

    let bash_nice = toml_cfg
        .tools
        .bash_nice
        .unwrap_or(ToolSettings::DEFAULT_BASH_NICE);

    let rtk_filter = toml_cfg.tools.rtk_filter.unwrap_or(false);

    // Opt-in: absent means off, so a default install is unchanged.
    let sandbox = toml_cfg.tools.sandbox.unwrap_or(false);

    let permission_mode = args
        .permission_mode
        .or(toml_cfg.permission.mode)
        .unwrap_or_default();

    Ok(NonLlmSettings {
        notifications_enabled,
        snapshot_max_items,
        micro_compact_enabled,
        memory_enabled,
        max_token_usage_bodies,
        skill_body_auto_inject,
        skill_dirs,
        skill_description_cap,
        skill_disable,
        instruction_sources,
        auto_memory_directory,
        theme,
        language,
        vision_image,
        hook_output,
        bash_timeout_secs,
        bash_nice,
        rtk_filter,
        sandbox,
        permission_mode,
        mcp: resolve_mcp(toml_cfg),
    })
}

pub(super) fn resolve_non_llm_settings(
    args: &CliArgs,
    toml_cfg: &TactTomlConfig,
    config_path: Option<std::path::PathBuf>,
) -> anyhow::Result<ResolvedConfig> {
    let non_llm = resolve_non_llm(args, toml_cfg)?;

    Ok(ResolvedConfig {
        // Placeholder identity: this path exists for subcommands that never
        // build a request (`--list-sessions`, `plugin`, `mcp`, `upgrade`), so
        // an unusable `[llm]` section must not stop them.
        llm: LlmSettings {
            provider: ProviderKind::OpenAi,
            protocol: OpenAiProtocol::default(),
            reasoning_effort: None,
            api_key: String::new(),
            base_url: String::new(),
            model: String::new(),
            models: Vec::new(),
            model_profiles: HashMap::new(),
            responses_compact_threshold: None,
        },
        agent: AgentSettings {
            model: String::new(),
            reasoning_effort: None,
            max_tokens: 8_000,
            thinking_budget: 0,
            model_context_window: 200_000,
            notifications_enabled: non_llm.notifications_enabled,
            snapshot_max_items: non_llm.snapshot_max_items,
            micro_compact_enabled: non_llm.micro_compact_enabled,
            memory_enabled: non_llm.memory_enabled,
            max_token_usage_bodies: non_llm.max_token_usage_bodies,
            skill_body_auto_inject: non_llm.skill_body_auto_inject,
            skill_dirs: non_llm.skill_dirs,
            skill_description_cap: non_llm.skill_description_cap,
            skill_disable: non_llm.skill_disable,
            instruction_sources: non_llm.instruction_sources,
            auto_memory_directory: non_llm.auto_memory_directory,
            subagent: None,
        },
        ui: UiSettings {
            theme: non_llm.theme,
            language: non_llm.language,
            vision_image: non_llm.vision_image,
            hook_output: non_llm.hook_output,
        },
        tools: ToolSettings {
            bash_timeout_secs: non_llm.bash_timeout_secs,
            bash_nice: non_llm.bash_nice,
            rtk_filter: non_llm.rtk_filter,
            sandbox: non_llm.sandbox,
        },
        // A malformed `[voice]` warns and degrades to disabled here: this path
        // serves subcommands that never record audio, so it must not fail.
        voice: resolve_voice(toml_cfg).unwrap_or_else(|err| {
            tracing::warn!(error = %err, "invalid voice configuration; voice input disabled");
            VoiceSettings::disabled_defaults()
        }),
        mcp: non_llm.mcp,
        goal: resolve_goal(toml_cfg)?,
        permission_mode: non_llm.permission_mode,
        tokio_console: args.tokio_console,
        config_path,
    })
}

/// The `max_tokens` a subagent gets when it overrides none: the Kimi K2.x
/// family has a documented 32K default, everything else inherits the main
/// agent's resolved budget. Shared with `apply_subagent_model_switch` so a
/// runtime switch re-inherits exactly the way startup does.
fn inherited_subagent_max_tokens(
    provider_kind: &ProviderKind,
    model: &str,
    main_max_tokens: u32,
) -> u32 {
    if *provider_kind == ProviderKind::Kimi
        && (model.contains("kimi-k2")
            || model.contains("k2.")
            || model.contains("k2-")
            || model == "kimi-for-coding")
    {
        32_000
    } else {
        main_max_tokens
    }
}

/// `max_tokens` must stay under the context window. An INHERITED budget is
/// clamped into the window (the user never asked for that pairing, and a main
/// budget larger than a small model's window is routine); a budget the user set
/// explicitly for this subagent is an error — silently changing a number they
/// wrote is worse than refusing.
fn fit_subagent_max_tokens(
    max_tokens: u32,
    explicit: bool,
    model_context_window: usize,
) -> anyhow::Result<u32> {
    if model_context_window == 0
        || usize::try_from(max_tokens).is_ok_and(|mt| mt < model_context_window)
    {
        return Ok(max_tokens);
    }
    if explicit {
        anyhow::bail!(
            "invalid subagent token limits: max_tokens ({max_tokens}) must be less than \
             the subagent context window ({model_context_window})"
        );
    }
    Ok(u32::try_from(model_context_window.saturating_sub(1)).unwrap_or(u32::MAX))
}

/// The thinking budget must fit under `max_tokens`. When `max_tokens` is ours
/// to move (inherited from the main agent or a Kimi K2.x default), the budget
/// wins and grows it — the same `budget + 1` auto-expand the main agent applies
/// when a budget is picked — bounded by the context window: a `budget + 1` that
/// does not fit under the window is an error. An explicit `max_tokens` is
/// refused rather than silently changed.
fn fit_subagent_thinking_budget(
    max_tokens: u32,
    max_tokens_explicit: bool,
    thinking_budget: usize,
    model_context_window: usize,
) -> anyhow::Result<u32> {
    if thinking_budget == 0 || usize::try_from(max_tokens).is_ok_and(|mt| thinking_budget < mt) {
        return Ok(max_tokens);
    }
    if max_tokens_explicit {
        anyhow::bail!(
            "subagent thinking_budget ({thinking_budget}) must be less than max_tokens ({max_tokens})"
        );
    }
    let expanded = u32::try_from(thinking_budget)
        .unwrap_or(u32::MAX)
        .saturating_add(1);
    if model_context_window != 0
        && usize::try_from(expanded).is_ok_and(|e| e >= model_context_window)
    {
        anyhow::bail!(
            "subagent thinking_budget ({thinking_budget}) cannot fit the context window ({model_context_window})"
        );
    }
    Ok(expanded)
}

/// The subagent's context window: an explicit
/// `[agent.subagent].model_context_window` pins it, otherwise the child model's
/// mapping decides, with the main agent's resolved window as the last resort
/// (a model the mapping does not know keeps a sensible value rather than
/// collapsing to the 200,000 default). Shared by the startup resolution and a
/// runtime `/model-subagent` switch so the two priorities cannot drift.
pub(crate) fn resolve_subagent_context_window(
    override_window: Option<usize>,
    model: &str,
    main_window: usize,
) -> usize {
    override_window
        .or_else(|| model_context_window_for_model(model))
        .unwrap_or(main_window)
}

/// Apply a runtime `/model-subagent` switch to resolved settings. Everything
/// derived from the model is re-resolved with the same rules as startup: the
/// context window (an explicit `[agent.subagent].model_context_window` keeps
/// pinning it), the output budget (re-inherited and clamped into the new
/// window; an explicit one that no longer fits refuses the switch), the
/// thinking budget (it grows an inherited `max_tokens` to `budget + 1`; an
/// explicit `max_tokens` it crosses refuses), and the Responses compaction
/// threshold (re-derived or re-validated against the new window). A stale
/// window would drive auto-compaction and the card's `ctx` meter for the rest
/// of the session; a stale threshold would compact against the old window.
///
/// On error nothing is applied: a refused switch leaves the session as it was.
pub(crate) fn apply_subagent_model_switch(
    agent: &mut AgentSettings,
    model: String,
    thinking_budget: usize,
) -> anyhow::Result<()> {
    let main_window = agent.model_context_window;
    let main_max_tokens = agent.max_tokens;
    let Some(subagent) = agent.subagent.as_mut() else {
        return Ok(());
    };

    let window = resolve_subagent_context_window(
        subagent.model_context_window_override,
        &model,
        main_window,
    );
    let max_tokens = match subagent.max_tokens_override {
        Some(explicit) => explicit,
        None => inherited_subagent_max_tokens(&subagent.provider.provider, &model, main_max_tokens),
    };
    let max_tokens =
        fit_subagent_max_tokens(max_tokens, subagent.max_tokens_override.is_some(), window)?;
    let max_tokens = fit_subagent_thinking_budget(
        max_tokens,
        subagent.max_tokens_override.is_some(),
        thinking_budget,
        window,
    )?;
    let responses_compact_threshold = resolve_responses_compact_threshold(
        subagent.responses_compact_threshold_override,
        subagent.provider.protocol,
        window,
        max_tokens,
    )
    .map_err(|error| anyhow::anyhow!("invalid subagent token limits: {error:#}"))?;

    subagent.provider.model = model;
    subagent.thinking_budget = thinking_budget;
    subagent.reasoning_effort = None;
    subagent.model_context_window = window;
    subagent.max_tokens = max_tokens;
    subagent.provider.responses_compact_threshold = responses_compact_threshold;
    Ok(())
}

/// Returns the context window (total input + output tokens) for a known model id.
///
/// This mapping is a **fallback** for models the user did not configure: the
/// resolution order is CLI `--model-context-window` > `[agent]
/// model_context_window` > this mapping > the 200,000 default. An explicit
/// value therefore always wins, so a stale manual window can under-report a
/// long-context model.
///
/// Values follow official docs (2026-08):
/// - OpenAI (developers.openai.com/api/docs/models): GPT-5.6 family and GPT-5.5
///   are 1,050,000; GPT-5.4 is 1,000,000; GPT-5 … GPT-5.3 and GPT-5.4-mini are
///   400,000; GPT-4o family is 128,000.
/// - Anthropic (platform.claude.com/docs/en/about-claude/models/overview):
///   Sonnet 5 / Fable 5 / Opus 5 / Opus 4.6–4.8 / Sonnet 4.6 are 1M; Sonnet 4 /
///   Opus 4 / Haiku 4.5 are 200K. Claude Code serves the same ids with the
///   same 1M windows.
/// - DeepSeek V4 family (api-docs.deepseek.com): 1M default; Kimi `k3-256k`: 256K.
pub(crate) fn model_context_window_for_model(model: &str) -> Option<usize> {
    match model {
        // OpenAI — GPT-5.6 family and GPT-5.5 (1.05M).
        "gpt-5.6" | "gpt-5.6-luna" | "gpt-5.6-terra" | "gpt-5.6-sol" | "gpt-5.5" => Some(1_050_000),
        // OpenAI — GPT-5.4 (up to 1M).
        "gpt-5.4" => Some(1_000_000),
        // OpenAI — 400K family (GPT-5 … GPT-5.3, GPT-5.4-mini).
        "gpt-5" | "gpt-5.1" | "gpt-5.2" | "gpt-5.3" | "gpt-5.3-codex" | "gpt-5.4-mini" => {
            Some(400_000)
        }
        // OpenAI — GPT-4o family (128K).
        "gpt-4o" | "gpt-4o-mini" => Some(128_000),
        // Anthropic — 1M models (API and Claude Code).
        "claude-sonnet-5" | "claude-fable-5" | "claude-opus-5" | "claude-opus-4-8"
        | "claude-opus-4-7" | "claude-opus-4-6" | "claude-sonnet-4-6" => Some(1_000_000),
        // Anthropic — 200K models.
        "claude-opus-4-20250514"
        | "claude-sonnet-4-20250514"
        | "claude-haiku-4-5"
        | "claude-haiku-4-20250514" => Some(200_000),
        // DeepSeek V4 family — 1M default. Ids can carry experiment/vision
        // suffixes (e.g. `deepseek-v4-flash-vision-exp`) and dot-separated minor
        // versions (`deepseek-v4.1-flash`), so match the family prefix rather
        // than a fixed id list. `deepseek-flash` is the unversioned alias
        // OpenAI-compatible gateways expose for the same V4 Flash model;
        // `deepseek-reasoner` is the official reasoning id.
        _ if model.starts_with("deepseek-v4-") || model.starts_with("deepseek-v4.") => {
            Some(1_000_000)
        }
        "deepseek-flash" | "deepseek-reasoner" => Some(1_000_000),
        // Kimi — k3-256k.
        "k3-256k" => Some(256_000),
        _ => None,
    }
}

pub(super) fn resolve_config(
    args: &CliArgs,
    toml_cfg: &TactTomlConfig,
    config_path: Option<std::path::PathBuf>,
) -> anyhow::Result<ResolvedConfig> {
    let llm = resolve_llm(args, toml_cfg)?;
    let provider_info = llm.provider_info();
    let entry = toml_cfg.llm.providers.get(llm.provider.as_str());

    // `[llm].max_tokens` was removed as a level: it sat *below* `[agent]`, so a
    // global value could only ever apply to users who set no `[agent]` key, and
    // setting both silently ignored one of them. A stale key is now a hard
    // error naming the replacement instead of a silent drop — otherwise the
    // request would quietly fall back to the built-in default.
    if toml_cfg.llm.max_tokens.is_some() {
        anyhow::bail!(
            "[llm].max_tokens was removed. Set [agent].max_tokens instead (or \
             [llm.providers.<name>].max_tokens for a per-provider value), or delete the key.\n\
             Resolution order: --max-tokens > [llm.providers.<active>].max_tokens > \
             [agent].max_tokens > default (8000; 32000 for Kimi K2.x)."
        );
    }

    // NOTE on `[agent]`: the struct is `deny_unknown_fields`, so a key that is
    // not a real agent field — a typo, or `thinking_budget` / `reasoning_effort`
    // (runtime agent fields that as TOML keys belong to `[llm]` or a provider
    // entry), or `model` — fails at parse time listing the valid fields. serde
    // used to drop all of these silently, so a session could run for its whole
    // life on default thinking settings while the config looked configured.

    let max_tokens = args
        .max_tokens
        .or_else(|| entry.and_then(|e| e.max_tokens))
        .or(toml_cfg.agent.max_tokens)
        .unwrap_or_else(|| {
            if provider_info.is_kimi_k2x(&provider_info.model) {
                32_000
            } else {
                8_000
            }
        });

    let thinking_budget = args
        .thinking_budget
        .or_else(|| entry.and_then(|e| e.thinking_budget))
        .or(toml_cfg.llm.thinking_budget)
        .unwrap_or(0);

    if thinking_budget > 0 && usize::try_from(max_tokens).is_ok_and(|mt| thinking_budget >= mt) {
        anyhow::bail!(
            "thinking_budget ({thinking_budget}) must be less than max_tokens ({max_tokens})"
        );
    }

    // Explicit config wins over the built-in model→window table: the mapping is
    // a fallback for models the user did not configure. Note the trade-off —
    // a stale manual value for a long-context model can now under-report the
    // real window and trigger premature auto-compaction, which is why the
    // mapping used to win. Resolution order: CLI > `[agent]` > mapping > default.
    let model_context_window = args
        .model_context_window
        .or(toml_cfg.agent.model_context_window)
        .or_else(|| model_context_window_for_model(&provider_info.model))
        .unwrap_or(200_000);

    if model_context_window != 0
        && !usize::try_from(max_tokens).is_ok_and(|max_tokens| max_tokens < model_context_window)
    {
        anyhow::bail!(
            "invalid token limits: max_tokens ({max_tokens}) must be less than agent.model_context_window ({model_context_window})"
        );
    }

    // Identical precedence chain to `resolve_non_llm_settings` — shared so the
    // two paths cannot drift.
    let non_llm = resolve_non_llm(args, toml_cfg)?;

    let subagent = resolve_subagent(toml_cfg, max_tokens, thinking_budget, model_context_window)?;

    let responses_compact_threshold = resolve_responses_compact_threshold(
        entry.and_then(|e| e.responses_compact_threshold),
        llm.protocol,
        model_context_window,
        max_tokens,
    )?;

    let voice = resolve_voice(toml_cfg)?;

    // Merge TOML model_profiles over built-in defaults (per model / per field).
    let mut model_profiles = crate::config::builtin_model_profiles();
    for (model, profile) in &toml_cfg.llm.model_profiles {
        let merged = model_profiles.entry(model.clone()).or_default();
        if !profile.thinking_budgets.is_empty() {
            merged.thinking_budgets = profile.thinking_budgets.clone();
        }
        if !profile.reasoning_efforts.is_empty() {
            merged.reasoning_efforts = profile.reasoning_efforts.clone();
        }
        // `Option` cannot use the "non-empty wins" rule, so an explicit value
        // always overrides (built-in profiles never set it).
        if let Some(explicit) = profile.supports_vision {
            merged.supports_vision = Some(explicit);
        }
    }

    let agent_model = llm.model.clone();
    let agent_effort = llm.reasoning_effort;

    Ok(ResolvedConfig {
        llm: LlmSettings {
            responses_compact_threshold,
            model_profiles,
            ..llm
        },
        agent: AgentSettings {
            model: agent_model,
            reasoning_effort: agent_effort,
            max_tokens,
            thinking_budget,
            model_context_window,
            notifications_enabled: non_llm.notifications_enabled,
            snapshot_max_items: non_llm.snapshot_max_items,
            micro_compact_enabled: non_llm.micro_compact_enabled,
            memory_enabled: non_llm.memory_enabled,
            max_token_usage_bodies: non_llm.max_token_usage_bodies,
            skill_body_auto_inject: non_llm.skill_body_auto_inject,
            skill_dirs: non_llm.skill_dirs,
            skill_description_cap: non_llm.skill_description_cap,
            skill_disable: non_llm.skill_disable,
            instruction_sources: non_llm.instruction_sources,
            auto_memory_directory: non_llm.auto_memory_directory,
            subagent,
        },
        ui: UiSettings {
            theme: non_llm.theme,
            language: non_llm.language,
            vision_image: non_llm.vision_image,
            hook_output: non_llm.hook_output,
        },
        tools: ToolSettings {
            bash_timeout_secs: non_llm.bash_timeout_secs,
            bash_nice: non_llm.bash_nice,
            rtk_filter: non_llm.rtk_filter,
            sandbox: non_llm.sandbox,
        },
        voice,
        mcp: non_llm.mcp,
        goal: resolve_goal(toml_cfg)?,
        permission_mode: non_llm.permission_mode,
        tokio_console: args.tokio_console,
        config_path,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SubagentTomlConfig;
    use crate::config::{cli::CliArgs, types::TactTomlConfig};

    fn empty_cli_args() -> CliArgs {
        CliArgs {
            command: None,
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
            model_context_window: None,
            theme: None,
            snapshot_max_items: None,
            no_micro_compact: false,
            no_notifications: false,
            tokio_console: false,
            skill_body_auto_inject: false,
        }
    }

    fn openai_toml_config() -> TactTomlConfig {
        toml::from_str(
            r#"
[llm]
provider = "openai"

[llm.providers.openai]
api_key = "sk-test"
model = "gpt-4o"
"#,
        )
        .unwrap()
    }

    fn empty_cli_args_with_openai() -> (CliArgs, TactTomlConfig) {
        (empty_cli_args(), openai_toml_config())
    }

    /// Responses config with a main agent and a subagent that reuses the
    /// `openai` provider entry. `threshold` sets the provider entry's
    /// `responses_compact_threshold` (shared by main and subagent);
    /// `subagent_max_tokens` overrides the subagent's output budget and
    /// `model_context_window` sets the shared agent context window.
    fn config_with_responses_main_and_subagent(
        threshold: Option<u32>,
        subagent_max_tokens: u32,
        model_context_window: usize,
    ) -> TactTomlConfig {
        let threshold_line = threshold
            .map(|t| format!("responses_compact_threshold = {t}"))
            .unwrap_or_default();
        toml::from_str(&format!(
            r#"
[llm]
provider = "openai"

[llm.providers.openai]
api_key = "sk-test"
model = "some-unknown-model"
protocol = "responses"
{threshold_line}

[agent]
max_tokens = 8000
model_context_window = {model_context_window}

[agent.subagent]
provider = "openai"
model = "some-unknown-model"
max_tokens = {subagent_max_tokens}
"#
        ))
        .unwrap()
    }

    #[test]
    fn google_voice_defaults_and_duration_limit() {
        let mut toml_cfg = openai_toml_config();
        toml_cfg.voice.provider = Some(VoiceProvider::Google);
        let cfg = resolve_config(&empty_cli_args(), &toml_cfg, None).unwrap();
        assert_eq!(cfg.voice.base_url, "https://speech.googleapis.com/v1");
        assert_eq!(cfg.voice.model, "latest_short");
        assert_eq!(cfg.voice.max_duration_secs, 60);

        toml_cfg.voice.max_duration_secs = Some(60);
        assert!(resolve_config(&empty_cli_args(), &toml_cfg, None).is_ok());

        toml_cfg.voice.max_duration_secs = Some(61);
        let err = resolve_config(&empty_cli_args(), &toml_cfg, None).unwrap_err();
        let message = err.to_string();
        assert!(message.contains("Google"));
        assert!(message.contains("between 1 and 60"));
    }

    #[test]
    fn google_voice_uses_custom_model_language_and_base_url() {
        let mut toml_cfg = openai_toml_config();
        toml_cfg.voice.provider = Some(VoiceProvider::Google);
        toml_cfg.voice.base_url = Some("http://localhost:9000/v1".to_string());
        toml_cfg.voice.model = Some("latest_long".to_string());
        toml_cfg.voice.language = Some("en-US".to_string());
        toml_cfg.voice.max_duration_secs = Some(42);
        let cfg = resolve_config(&empty_cli_args(), &toml_cfg, None).unwrap();
        assert_eq!(cfg.voice.base_url, "http://localhost:9000/v1");
        assert_eq!(cfg.voice.model, "latest_long");
        assert_eq!(cfg.voice.language.as_deref(), Some("en-US"));
        assert_eq!(cfg.voice.max_duration_secs, 42);
    }

    #[test]
    fn resolve_mcp_oauth_client_name_defaults_to_codex_and_is_overridable() {
        // Default: the value providers such as Figma admit.
        let (args, toml_cfg) = empty_cli_args_with_openai();
        let cfg = resolve_config(&args, &toml_cfg, None).unwrap();
        assert_eq!(
            cfg.mcp.oauth_client_name,
            McpSettings::DEFAULT_OAUTH_CLIENT_NAME
        );

        // Configured value wins, and is trimmed.
        let (args, mut toml_cfg) = empty_cli_args_with_openai();
        toml_cfg.mcp.oauth_client_name = Some("  MyAgent  ".to_string());
        let cfg = resolve_config(&args, &toml_cfg, None).unwrap();
        assert_eq!(cfg.mcp.oauth_client_name, "MyAgent");

        // Blank falls back to the default rather than sending an empty name.
        let (args, mut toml_cfg) = empty_cli_args_with_openai();
        toml_cfg.mcp.oauth_client_name = Some("   ".to_string());
        let cfg = resolve_config(&args, &toml_cfg, None).unwrap();
        assert_eq!(
            cfg.mcp.oauth_client_name,
            McpSettings::DEFAULT_OAUTH_CLIENT_NAME
        );

        // `McpSettings::default()` must agree with the resolver, since it is
        // the fallback when config is not installed.
        assert_eq!(
            McpSettings::default().oauth_client_name,
            McpSettings::DEFAULT_OAUTH_CLIENT_NAME
        );
    }

    #[test]
    fn resolve_mcp_resource_tool_risk_defaults_to_high_and_is_overridable() {
        use crate::permission::CapabilityRisk;

        let (args, toml_cfg) = empty_cli_args_with_openai();
        let cfg = resolve_config(&args, &toml_cfg, None).unwrap();
        assert_eq!(cfg.mcp.resource_list_risk, None);
        assert_eq!(cfg.mcp.resource_read_risk, None);

        let (args, mut toml_cfg) = empty_cli_args_with_openai();
        toml_cfg.mcp.resource_list_risk = Some(" write ".to_string());
        toml_cfg.mcp.resource_read_risk = Some("read".to_string());
        let cfg = resolve_config(&args, &toml_cfg, None).unwrap();
        assert_eq!(cfg.mcp.resource_list_risk, Some(CapabilityRisk::Write));
        assert_eq!(cfg.mcp.resource_read_risk, Some(CapabilityRisk::Read));

        // An unknown value is ignored rather than guessed, so the tool keeps the
        // restrictive default — the same rule `tools.<name>.risk` follows.
        let (args, mut toml_cfg) = empty_cli_args_with_openai();
        toml_cfg.mcp.resource_read_risk = Some("harmless".to_string());
        let cfg = resolve_config(&args, &toml_cfg, None).unwrap();
        assert_eq!(cfg.mcp.resource_read_risk, None);
    }

    #[test]
    fn resolve_mcp_prompt_tool_risk_defaults_to_high_and_is_overridable() {
        use crate::permission::CapabilityRisk;

        let (args, toml_cfg) = empty_cli_args_with_openai();
        let cfg = resolve_config(&args, &toml_cfg, None).unwrap();
        assert_eq!(cfg.mcp.prompt_list_risk, None);
        assert_eq!(cfg.mcp.prompt_get_risk, None);

        let (args, mut toml_cfg) = empty_cli_args_with_openai();
        toml_cfg.mcp.prompt_list_risk = Some(" write ".to_string());
        toml_cfg.mcp.prompt_get_risk = Some("read".to_string());
        let cfg = resolve_config(&args, &toml_cfg, None).unwrap();
        assert_eq!(cfg.mcp.prompt_list_risk, Some(CapabilityRisk::Write));
        assert_eq!(cfg.mcp.prompt_get_risk, Some(CapabilityRisk::Read));

        // Same rule as the resource keys: an unknown value is ignored rather
        // than guessed, so the tool keeps the restrictive default.
        let (args, mut toml_cfg) = empty_cli_args_with_openai();
        toml_cfg.mcp.prompt_get_risk = Some("harmless".to_string());
        let cfg = resolve_config(&args, &toml_cfg, None).unwrap();
        assert_eq!(cfg.mcp.prompt_get_risk, None);

        // And the two families stay independent: a prompt key must not move a
        // resource tool.
        let (args, mut toml_cfg) = empty_cli_args_with_openai();
        toml_cfg.mcp.prompt_list_risk = Some("read".to_string());
        let cfg = resolve_config(&args, &toml_cfg, None).unwrap();
        assert_eq!(cfg.mcp.resource_list_risk, None);
    }

    #[test]
    fn resolve_voice_defaults_and_validation() {
        let (args, toml_cfg) = empty_cli_args_with_openai();
        let cfg = resolve_config(&args, &toml_cfg, None).unwrap();
        assert!(!cfg.voice.enabled);
        assert_eq!(cfg.voice.base_url, "https://api.openai.com/v1");
        assert_eq!(cfg.voice.model, "gpt-4o-mini-transcribe");
        assert_eq!(cfg.voice.language.as_deref(), Some("zh"));
        assert_eq!(cfg.voice.max_duration_secs, 300);
    }

    #[test]
    fn explicit_empty_language_disables_language_hint() {
        let (args, mut toml_cfg) = empty_cli_args_with_openai();
        toml_cfg.voice.language = Some(String::new());
        let cfg = resolve_config(&args, &toml_cfg, None).unwrap();
        assert_eq!(cfg.voice.language, None);
    }

    #[test]
    fn reject_voice_duration_outside_safe_range() {
        let mut toml_cfg = openai_toml_config();
        toml_cfg.voice.enabled = Some(true);
        toml_cfg.voice.max_duration_secs = Some(0);
        let err = resolve_config(&empty_cli_args(), &toml_cfg, None).unwrap_err();
        assert!(err.to_string().contains("voice.max_duration_secs"));
    }

    #[test]
    fn voice_keybind_passes_through_when_valid() {
        let mut toml_cfg = openai_toml_config();
        toml_cfg.voice.voice_keybind = Some("ctrl+g".to_string());
        let cfg = resolve_config(&empty_cli_args(), &toml_cfg, None).unwrap();
        assert_eq!(cfg.voice.voice_keybind.as_deref(), Some("ctrl+g"));
    }

    #[test]
    fn voice_keybind_rejects_empty_string() {
        let mut toml_cfg = openai_toml_config();
        toml_cfg.voice.voice_keybind = Some("".to_string());
        let err = resolve_config(&empty_cli_args(), &toml_cfg, None).unwrap_err();
        assert!(err.to_string().contains("voice_keybind must not be empty"));
    }

    #[test]
    fn voice_keybind_rejects_unsupported_modifier() {
        let mut toml_cfg = openai_toml_config();
        toml_cfg.voice.voice_keybind = Some("alt+x".to_string());
        let err = resolve_config(&empty_cli_args(), &toml_cfg, None).unwrap_err();
        assert!(err.to_string().contains("only 'ctrl' is allowed"));
    }

    #[test]
    fn voice_keybind_rejects_multi_char_key() {
        let mut toml_cfg = openai_toml_config();
        toml_cfg.voice.voice_keybind = Some("ctrl+ab".to_string());
        let err = resolve_config(&empty_cli_args(), &toml_cfg, None).unwrap_err();
        assert!(err.to_string().contains("must be a single character"));
    }

    #[test]
    fn voice_keybind_rejects_no_modifier() {
        let mut toml_cfg = openai_toml_config();
        toml_cfg.voice.voice_keybind = Some("g".to_string());
        let err = resolve_config(&empty_cli_args(), &toml_cfg, None).unwrap_err();
        assert!(err.to_string().contains("expected format"));
    }

    #[test]
    fn subagent_inherits_main_agent_thinking_budget() {
        let mut toml_cfg = openai_toml_config();
        toml_cfg.agent.subagent = Some(SubagentTomlConfig {
            provider: Some("openai".to_string()),
            model: Some("gpt-4o-mini".to_string()),
            max_tokens: None,
            thinking_budget: None,
            reasoning_effort: None,
            model_context_window: None,
        });
        let cfg = resolve_config(&empty_cli_args(), &toml_cfg, None).unwrap();
        let sa = cfg.agent.subagent.unwrap();
        assert_eq!(sa.thinking_budget, 0);
        assert_eq!(sa.max_tokens, 8_000);
    }

    /// The subagent's window follows the SUBAGENT's model, not the main
    /// agent's: measuring a `gpt-4o` child against a 1M main window would make
    /// auto-compaction fire far too late, and the card's `ctx` meter lie.
    #[test]
    fn subagent_window_follows_its_own_model_not_the_main_agents() {
        let mut toml_cfg = openai_toml_config();
        // Main agent: explicit 1M window.
        toml_cfg.agent.model_context_window = Some(1_000_000);
        toml_cfg.agent.subagent = Some(SubagentTomlConfig {
            provider: Some("openai".to_string()),
            // gpt-4o is mapped to 128K, so the child must resolve to 128K.
            model: Some("gpt-4o".to_string()),
            max_tokens: None,
            thinking_budget: None,
            reasoning_effort: None,
            model_context_window: None,
        });
        let cfg = resolve_config(&empty_cli_args(), &toml_cfg, None).unwrap();
        assert_eq!(cfg.agent.model_context_window, 1_000_000);
        assert_eq!(cfg.agent.subagent.unwrap().model_context_window, 128_000);
    }

    /// An explicit `[agent.subagent].model_context_window` outranks the mapping.
    #[test]
    fn subagent_explicit_window_beats_the_model_mapping() {
        let mut toml_cfg = openai_toml_config();
        toml_cfg.agent.subagent = Some(SubagentTomlConfig {
            provider: Some("openai".to_string()),
            model: Some("gpt-4o".to_string()),
            max_tokens: None,
            thinking_budget: None,
            reasoning_effort: None,
            model_context_window: Some(300_000),
        });
        let cfg = resolve_config(&empty_cli_args(), &toml_cfg, None).unwrap();
        assert_eq!(cfg.agent.subagent.unwrap().model_context_window, 300_000);
    }

    /// A runtime `/model-subagent` switch re-resolves the window with the same
    /// priority as startup: an explicit `[agent.subagent].model_context_window`
    /// keeps pinning it. The regression this pins: the switch used to drop the
    /// pin and, for a model the mapping does not know, silently fall back to
    /// the MAIN agent's window (1M here).
    #[test]
    fn subagent_model_switch_keeps_an_explicit_window_override() {
        let mut toml_cfg = openai_toml_config();
        toml_cfg.agent.model_context_window = Some(1_000_000);
        toml_cfg.agent.subagent = Some(SubagentTomlConfig {
            provider: Some("openai".to_string()),
            model: Some("gpt-4o".to_string()),
            max_tokens: None,
            thinking_budget: None,
            reasoning_effort: None,
            model_context_window: Some(64_000),
        });
        let mut cfg = resolve_config(&empty_cli_args(), &toml_cfg, None).unwrap();
        assert_eq!(
            cfg.agent.subagent.as_ref().unwrap().model_context_window,
            64_000
        );

        apply_subagent_model_switch(&mut cfg.agent, "some-unmapped-model".to_string(), 0).unwrap();

        let sa = cfg.agent.subagent.unwrap();
        assert_eq!(sa.provider.model, "some-unmapped-model");
        assert_eq!(sa.model_context_window, 64_000);
    }

    /// Without an explicit override the mapping follows the NEW model: the fix
    /// must not freeze the old window, which would trade one stale number for
    /// another.
    #[test]
    fn subagent_model_switch_re_resolves_the_mapping() {
        let mut toml_cfg = openai_toml_config();
        toml_cfg.agent.model_context_window = Some(500_000);
        toml_cfg.agent.subagent = Some(SubagentTomlConfig {
            provider: Some("openai".to_string()),
            model: Some("gpt-4o".to_string()),
            max_tokens: None,
            thinking_budget: None,
            reasoning_effort: None,
            model_context_window: None,
        });
        let mut cfg = resolve_config(&empty_cli_args(), &toml_cfg, None).unwrap();
        assert_eq!(
            cfg.agent.subagent.as_ref().unwrap().model_context_window,
            128_000
        );

        apply_subagent_model_switch(&mut cfg.agent, "gpt-5.6".to_string(), 0).unwrap();

        assert_eq!(cfg.agent.subagent.unwrap().model_context_window, 1_050_000);
    }

    /// A runtime switch re-inherits the budget like startup: an inherited
    /// `max_tokens` follows the new model's window — clamped into it when it
    /// does not fit — instead of freezing at what the old window allowed.
    #[test]
    fn subagent_model_switch_re_inherits_and_clamps_the_budget() {
        let toml_cfg: TactTomlConfig = toml::from_str(
            r#"
[llm]
provider = "openai"

[llm.providers.openai]
api_key = "sk-test"
model = "some-unknown-model"

[agent]
max_tokens = 200000
model_context_window = 1000000

[agent.subagent]
provider = "openai"
model = "gpt-4o"
"#,
        )
        .unwrap();
        let mut cfg = resolve_config(&empty_cli_args(), &toml_cfg, None).unwrap();
        // gpt-4o (128K) cannot take the inherited 200K: clamped at startup.
        assert_eq!(cfg.agent.subagent.as_ref().unwrap().max_tokens, 127_999);

        apply_subagent_model_switch(&mut cfg.agent, "gpt-5.6".to_string(), 0).unwrap();
        // 1.05M can take the inherited 200K again.
        let sa = cfg.agent.subagent.as_ref().unwrap();
        assert_eq!(sa.max_tokens, 200_000);
        assert_eq!(sa.model_context_window, 1_050_000);

        apply_subagent_model_switch(&mut cfg.agent, "gpt-4o".to_string(), 0).unwrap();
        assert_eq!(cfg.agent.subagent.unwrap().max_tokens, 127_999);
    }

    /// An EXPLICIT budget that no longer fits the picked model is refused and
    /// nothing is applied: silently changing a number the user wrote is worse
    /// than refusing the switch.
    #[test]
    fn subagent_model_switch_refuses_an_explicit_budget_over_the_new_window() {
        let toml_cfg: TactTomlConfig = toml::from_str(
            r#"
[llm]
provider = "openai"

[llm.providers.openai]
api_key = "sk-test"
model = "some-unknown-model"

[agent]
max_tokens = 8000
model_context_window = 1000000

[agent.subagent]
provider = "openai"
model = "gpt-5.6"
max_tokens = 200000
"#,
        )
        .unwrap();
        let mut cfg = resolve_config(&empty_cli_args(), &toml_cfg, None).unwrap();
        assert_eq!(cfg.agent.subagent.as_ref().unwrap().max_tokens, 200_000);

        let err = apply_subagent_model_switch(&mut cfg.agent, "gpt-4o".to_string(), 0).unwrap_err();

        assert!(
            err.to_string()
                .contains("must be less than the subagent context window"),
            "got: {err}"
        );
        // Nothing was applied: the session keeps the model it had.
        let sa = cfg.agent.subagent.unwrap();
        assert_eq!(sa.provider.model, "gpt-5.6");
        assert_eq!(sa.max_tokens, 200_000);
        assert_eq!(sa.model_context_window, 1_050_000);
    }

    /// The Responses compaction threshold is derived from the child's window,
    /// so a switch must re-derive it: a threshold measured for the old model
    /// would compact too late — or not at all — for the new one.
    #[test]
    fn subagent_model_switch_re_derives_the_responses_threshold() {
        let toml_cfg: TactTomlConfig = toml::from_str(
            r#"
[llm]
provider = "openai"

[llm.providers.openai]
api_key = "sk-test"
model = "some-unknown-model"
protocol = "responses"

[agent]
max_tokens = 8000
model_context_window = 1000000

[agent.subagent]
provider = "openai"
model = "gpt-4o"
"#,
        )
        .unwrap();
        let mut cfg = resolve_config(&empty_cli_args(), &toml_cfg, None).unwrap();
        // 128000 - 8000 max_tokens - 12800 headroom.
        assert_eq!(
            cfg.agent
                .subagent
                .as_ref()
                .unwrap()
                .provider
                .responses_compact_threshold,
            Some(107_200)
        );

        apply_subagent_model_switch(&mut cfg.agent, "gpt-5.6".to_string(), 0).unwrap();

        let sa = cfg.agent.subagent.unwrap();
        assert_eq!(sa.model_context_window, 1_050_000);
        // 1050000 - 8000 - 105000 headroom.
        assert_eq!(sa.provider.responses_compact_threshold, Some(937_000));
    }

    /// An explicit `responses_compact_threshold` is re-validated against the
    /// new window, with the same refusal semantics as startup.
    #[test]
    fn subagent_model_switch_refuses_a_threshold_that_no_longer_fits() {
        let toml_cfg: TactTomlConfig = toml::from_str(
            r#"
[llm]
provider = "openai"

[llm.providers.openai]
api_key = "sk-test"
model = "some-unknown-model"
protocol = "responses"
responses_compact_threshold = 200000

[agent]
max_tokens = 8000
model_context_window = 1000000

[agent.subagent]
provider = "openai"
model = "gpt-5.6"
"#,
        )
        .unwrap();
        let mut cfg = resolve_config(&empty_cli_args(), &toml_cfg, None).unwrap();
        assert_eq!(
            cfg.agent
                .subagent
                .as_ref()
                .unwrap()
                .provider
                .responses_compact_threshold,
            Some(200_000)
        );

        let err = apply_subagent_model_switch(&mut cfg.agent, "gpt-4o".to_string(), 0).unwrap_err();

        assert!(
            err.to_string().contains("must leave room for max_tokens"),
            "got: {err}"
        );
        let sa = cfg.agent.subagent.unwrap();
        assert_eq!(sa.provider.model, "gpt-5.6");
        assert_eq!(sa.provider.responses_compact_threshold, Some(200_000));
    }

    /// A switch grows an inherited `max_tokens` to fit the budget it was given
    /// (the picker's choice): the same `budget + 1` rule startup applies.
    #[test]
    fn subagent_model_switch_grows_an_inherited_max_tokens_to_fit_the_budget() {
        let mut toml_cfg = openai_toml_config();
        toml_cfg.agent.model_context_window = Some(500_000);
        toml_cfg.agent.subagent = Some(SubagentTomlConfig {
            provider: Some("openai".to_string()),
            model: Some("gpt-4o".to_string()),
            max_tokens: None,
            thinking_budget: None,
            reasoning_effort: None,
            model_context_window: None,
        });
        let mut cfg = resolve_config(&empty_cli_args(), &toml_cfg, None).unwrap();
        assert_eq!(cfg.agent.subagent.as_ref().unwrap().max_tokens, 8_000);

        apply_subagent_model_switch(&mut cfg.agent, "gpt-5.6".to_string(), 32_000).unwrap();

        let sa = cfg.agent.subagent.unwrap();
        assert_eq!(sa.thinking_budget, 32_000);
        assert_eq!(sa.max_tokens, 32_001);
    }

    /// An explicit `max_tokens` is never silently grown: a budget that crosses
    /// it refuses the switch, and nothing is applied.
    #[test]
    fn subagent_model_switch_refuses_a_budget_over_an_explicit_max_tokens() {
        let mut toml_cfg = openai_toml_config();
        toml_cfg.agent.subagent = Some(SubagentTomlConfig {
            provider: Some("openai".to_string()),
            model: Some("gpt-4o".to_string()),
            max_tokens: Some(40_000),
            thinking_budget: None,
            reasoning_effort: None,
            model_context_window: None,
        });
        let mut cfg = resolve_config(&empty_cli_args(), &toml_cfg, None).unwrap();
        assert_eq!(cfg.agent.subagent.as_ref().unwrap().max_tokens, 40_000);

        let err =
            apply_subagent_model_switch(&mut cfg.agent, "gpt-5.6".to_string(), 64_000).unwrap_err();

        assert!(
            err.to_string().contains("must be less than max_tokens"),
            "got: {err}"
        );
        let sa = cfg.agent.subagent.unwrap();
        assert_eq!(sa.provider.model, "gpt-4o");
        assert_eq!(sa.thinking_budget, 0);
    }

    /// The window is not just displayed: it is the auto-compaction threshold's
    /// input, so a child on a small model must derive its threshold from THAT
    /// window. Asserting the derived threshold is what gives the resolve change
    /// teeth (the window value alone can be right for the wrong reason).
    #[test]
    fn subagent_compaction_threshold_follows_the_child_window() {
        let toml_cfg: TactTomlConfig = toml::from_str(
            r#"
[llm]
provider = "openai"

[llm.providers.openai]
api_key = "sk-test"
model = "some-unknown-model"
protocol = "responses"

[agent]
max_tokens = 8000
model_context_window = 1000000

[agent.subagent]
provider = "openai"
model = "gpt-4o"
"#,
        )
        .unwrap();
        let cfg = resolve_config(&empty_cli_args(), &toml_cfg, None).unwrap();
        let sa = cfg.agent.subagent.unwrap();
        // The child runs gpt-4o (128K), so its derived threshold must come from
        // THAT window: 128000 - 8000 max_tokens - 12800 headroom.
        assert_eq!(sa.model_context_window, 128_000);
        assert_eq!(
            sa.provider.responses_compact_threshold,
            Some(107_200),
            "the threshold must be derived from the child's window, not the main one"
        );
    }

    /// An INHERITED `max_tokens` larger than the child's window must be clamped,
    /// not rejected: the user never asked for that pairing, and the main agent's
    /// budget is routinely bigger than a small model's window.
    #[test]
    fn subagent_inherited_max_tokens_is_clamped_to_its_window() {
        let toml_cfg: TactTomlConfig = toml::from_str(
            r#"
[llm]
provider = "openai"

[llm.providers.openai]
api_key = "sk-test"
model = "some-unknown-model"

[agent]
max_tokens = 200000
model_context_window = 1000000

[agent.subagent]
provider = "openai"
model = "gpt-4o"
"#,
        )
        .unwrap();
        let cfg = resolve_config(&empty_cli_args(), &toml_cfg, None).unwrap();
        let sa = cfg.agent.subagent.unwrap();
        assert_eq!(sa.model_context_window, 128_000);
        assert_eq!(sa.max_tokens, 127_999, "clamped below the window");
    }

    /// The budget check runs against the POST-clamp `max_tokens`: a clamped
    /// window that no longer leaves room for the budget is an error, not the
    /// silently broken pairing the old pre-clamp check let through.
    #[test]
    fn subagent_budget_that_cannot_fit_the_child_window_errors() {
        let toml_cfg: TactTomlConfig = toml::from_str(
            r#"
[llm]
provider = "openai"

[llm.providers.openai]
api_key = "sk-test"
model = "some-unknown-model"

[agent]
max_tokens = 200000
model_context_window = 1000000

[agent.subagent]
provider = "openai"
model = "gpt-4o"
thinking_budget = 150000
"#,
        )
        .unwrap();
        let err = resolve_config(&empty_cli_args(), &toml_cfg, None).unwrap_err();
        assert!(
            err.to_string().contains("cannot fit the context window"),
            "got: {err}"
        );
    }

    /// An explicit `max_tokens` that does not fit the child window is still a
    /// hard error — that pairing the user did ask for.
    #[test]
    fn subagent_explicit_max_tokens_over_its_window_still_errors() {
        let toml_cfg: TactTomlConfig = toml::from_str(
            r#"
[llm]
provider = "openai"

[llm.providers.openai]
api_key = "sk-test"
model = "some-unknown-model"

[agent]
max_tokens = 8000
model_context_window = 1000000

[agent.subagent]
provider = "openai"
model = "gpt-4o"
max_tokens = 200000
"#,
        )
        .unwrap();
        let err = resolve_config(&empty_cli_args(), &toml_cfg, None).unwrap_err();
        assert!(
            err.to_string().contains("subagent context window"),
            "got: {err}"
        );
    }

    /// A model the mapping does not know keeps the main agent's window rather
    /// than collapsing to the 200K default.
    #[test]
    fn subagent_unknown_model_falls_back_to_the_main_window() {
        let mut toml_cfg = openai_toml_config();
        toml_cfg.agent.model_context_window = Some(750_000);
        toml_cfg.agent.subagent = Some(SubagentTomlConfig {
            provider: Some("openai".to_string()),
            model: Some("some-unmapped-model".to_string()),
            max_tokens: None,
            thinking_budget: None,
            reasoning_effort: None,
            model_context_window: None,
        });
        let cfg = resolve_config(&empty_cli_args(), &toml_cfg, None).unwrap();
        assert_eq!(cfg.agent.subagent.unwrap().model_context_window, 750_000);
    }

    /// A section that sets overrides without `provider` used to be dropped
    /// silently, so `max_tokens` in the file never reached a request.
    #[test]
    fn subagent_overrides_without_provider_errors() {
        let toml_cfg: TactTomlConfig = toml::from_str(
            r#"
[llm]
provider = "openai"

[llm.providers.openai]
api_key = "sk-test"
model = "gpt-4o"

[agent.subagent]
max_tokens = 64000
"#,
        )
        .unwrap();
        let err = resolve_config(&empty_cli_args(), &toml_cfg, None)
            .unwrap_err()
            .to_string();
        assert!(err.contains("[agent.subagent]"), "got: {err}");
        assert!(err.contains("`provider` is missing"), "got: {err}");
    }

    /// The same guard must not fire for a leftover header whose keys are all
    /// commented out — that section is a no-op, not a mistake.
    #[test]
    fn subagent_empty_section_without_provider_is_ignored() {
        let toml_cfg: TactTomlConfig = toml::from_str(
            r#"
[llm]
provider = "openai"

[llm.providers.openai]
api_key = "sk-test"
model = "gpt-4o"

[agent.subagent]
"#,
        )
        .unwrap();
        let resolved = resolve_config(&empty_cli_args(), &toml_cfg, None).unwrap();
        assert!(resolved.agent.subagent.is_none());
    }

    #[test]
    fn subagent_uses_kimi_default_max_tokens() {
        let toml_cfg: TactTomlConfig = toml::from_str(
            r#"
[llm]
provider = "openai"

[llm.providers.openai]
api_key = "sk-test"
model = "gpt-4o"

[llm.providers.kimi]
api_key = "sk-kimi"
model = "kimi-k2.5"

[agent.subagent]
provider = "kimi"
model = "kimi-k2.5"
"#,
        )
        .unwrap();
        let cfg = resolve_config(&empty_cli_args(), &toml_cfg, None).unwrap();
        let sa = cfg.agent.subagent.unwrap();
        assert_eq!(sa.max_tokens, 32_000);
    }

    /// A budget equal to the Kimi default must not brick the next startup: the
    /// picker offers the profile's largest budget (32K for `kimi-for-coding`,
    /// the same number the Kimi K2.x default gives `max_tokens`), and
    /// persisting that pair used to make the strict `budget < max_tokens` check
    /// fail on the NEXT launch. The inherited `max_tokens` grows to
    /// `budget + 1` instead, like the main agent's auto-expand.
    #[test]
    fn subagent_budget_equal_to_the_kimi_default_grows_max_tokens() {
        let toml_cfg: TactTomlConfig = toml::from_str(
            r#"
[llm]
provider = "openai"

[llm.providers.openai]
api_key = "sk-test"
model = "gpt-4o"

[llm.providers.kimi]
api_key = "sk-kimi"
model = "kimi-k2.5"

[agent.subagent]
provider = "kimi"
model = "kimi-for-coding"
thinking_budget = 32000
"#,
        )
        .unwrap();
        let cfg = resolve_config(&empty_cli_args(), &toml_cfg, None).unwrap();
        let sa = cfg.agent.subagent.unwrap();
        assert_eq!(sa.thinking_budget, 32_000);
        assert_eq!(sa.max_tokens, 32_001, "the inherited default grows to fit");
    }

    #[test]
    fn subagent_explicit_overrides_beat_inherited_defaults() {
        let mut toml_cfg = openai_toml_config();
        toml_cfg.agent.subagent = Some(SubagentTomlConfig {
            provider: Some("openai".to_string()),
            model: Some("gpt-4o-mini".to_string()),
            max_tokens: Some(16_000),
            thinking_budget: Some(8_000),
            reasoning_effort: None,
            model_context_window: None,
        });
        let cfg = resolve_config(&empty_cli_args(), &toml_cfg, None).unwrap();
        let sa = cfg.agent.subagent.unwrap();
        assert_eq!(sa.max_tokens, 16_000);
        assert_eq!(sa.thinking_budget, 8_000);
    }

    #[test]
    fn thinking_budget_not_less_than_max_tokens_errors() {
        let mut toml_cfg = openai_toml_config();
        toml_cfg.llm.thinking_budget = Some(16_000);
        // max_tokens defaults to 8_000, so thinking_budget(16_000) >= max_tokens(8_000)
        let err = resolve_config(&empty_cli_args(), &toml_cfg, None).unwrap_err();
        assert!(err.to_string().contains("thinking_budget"));
    }

    #[test]
    fn subagent_thinking_budget_not_less_than_max_tokens_errors() {
        let mut toml_cfg = openai_toml_config();
        toml_cfg.agent.subagent = Some(SubagentTomlConfig {
            provider: Some("openai".to_string()),
            model: Some("gpt-4o-mini".to_string()),
            max_tokens: Some(4_000),
            thinking_budget: Some(8_000),
            reasoning_effort: None,
            model_context_window: None,
        });
        let err = resolve_config(&empty_cli_args(), &toml_cfg, None).unwrap_err();
        assert!(err.to_string().contains("thinking_budget"));
    }

    #[test]
    fn goal_budgets_default_to_500_turns_and_resolve_from_toml() {
        let resolved = resolve_config(&empty_cli_args(), &openai_toml_config(), None).unwrap();
        assert_eq!(resolved.goal, GoalSettings::default());
        assert_eq!(resolved.goal.max_turns, 500);

        let toml_cfg: TactTomlConfig = toml::from_str(
            r#"
[llm]
provider = "openai"

[llm.providers.openai]
api_key = "sk-test"
model = "gpt-4o"

[goal]
max_turns = 1200
max_elapsed_secs = 1800
"#,
        )
        .unwrap();
        let resolved = resolve_config(&empty_cli_args(), &toml_cfg, None).unwrap();
        assert_eq!(resolved.goal.max_turns, 1200);
        assert_eq!(resolved.goal.max_elapsed_secs, 1800);
    }

    #[test]
    fn goal_budget_zero_is_rejected() {
        let toml_cfg: TactTomlConfig = toml::from_str(
            r#"
[llm]
provider = "openai"

[llm.providers.openai]
api_key = "sk-test"
model = "gpt-4o"

[goal]
max_turns = 0
"#,
        )
        .unwrap();
        let err = resolve_config(&empty_cli_args(), &toml_cfg, None).unwrap_err();
        assert!(err.to_string().contains("[goal].max_turns"), "{err}");
    }

    #[test]
    fn resolve_config_from_toml() {
        let toml_cfg: TactTomlConfig = toml::from_str(
            r#"
[llm]
provider = "openai"

[llm.providers.openai]
api_key = "sk-test"
model = "gpt-4o"
"#,
        )
        .unwrap();
        let resolved = resolve_config(&empty_cli_args(), &toml_cfg, None).unwrap();
        assert_eq!(resolved.llm.provider, ProviderKind::OpenAi);
        assert_eq!(
            resolved.llm.protocol,
            tact_llm::OpenAiProtocol::ChatCompletions
        );
        assert_eq!(resolved.llm.api_key, "sk-test");
        assert_eq!(resolved.llm.base_url, "https://api.openai.com/v1");
        assert_eq!(resolved.agent.max_tokens, 8000);
        assert_eq!(resolved.ui.theme, "ink");
        // Absent means shown: the switch exists to silence a noisy hook, so a
        // config that predates it keeps the behaviour it had.
        assert_eq!(resolved.ui.hook_output, UiSettings::DEFAULT_HOOK_OUTPUT);
        assert!(resolved.ui.hook_output);
        assert_eq!(
            resolved.ui.vision_image.compress,
            VisionImageSettings::DEFAULT_COMPRESS
        );
        assert_eq!(
            resolved.ui.vision_image.max_edge,
            VisionImageSettings::DEFAULT_MAX_EDGE
        );
        assert_eq!(
            resolved.ui.vision_image.jpeg_quality,
            VisionImageSettings::DEFAULT_JPEG_QUALITY
        );
        assert!(!resolved.agent.micro_compact_enabled);
        assert!(resolved.agent.memory_enabled);
        assert_eq!(
            resolved.agent.instruction_sources,
            InstructionSources::default()
        );
    }

    #[test]
    fn memory_can_be_disabled_in_toml() {
        let mut toml_cfg = openai_toml_config();
        toml_cfg.agent.memory_enabled = Some(false);

        let resolved = resolve_config(&empty_cli_args(), &toml_cfg, None).unwrap();
        assert!(!resolved.agent.memory_enabled);
    }

    #[test]
    fn resolve_openai_responses_protocol() {
        let toml_cfg: TactTomlConfig = toml::from_str(
            r#"
[llm]
provider = "openai"

[llm.providers.openai]
api_key = "sk-test"
model = "gpt-5"
protocol = "responses"
"#,
        )
        .unwrap();

        let resolved = resolve_config(&empty_cli_args(), &toml_cfg, None).unwrap();
        assert_eq!(resolved.llm.protocol, tact_llm::OpenAiProtocol::Responses);
    }

    #[test]
    fn resolve_openai_reasoning_effort() {
        let toml_cfg: TactTomlConfig = toml::from_str(
            r#"
[llm]
provider = "openai"

[llm.providers.openai]
api_key = "sk-test"
model = "gpt-5"
protocol = "responses"
reasoning_effort = "max"
"#,
        )
        .unwrap();

        let resolved = resolve_config(&empty_cli_args(), &toml_cfg, None).unwrap();
        assert_eq!(
            resolved.llm.reasoning_effort,
            Some(tact_llm::OpenAiReasoningEffort::Max)
        );
    }

    #[test]
    fn reject_reasoning_effort_for_anthropic_provider() {
        let toml_cfg: TactTomlConfig = toml::from_str(
            r#"
[llm]
provider = "anthropic"

[llm.providers.anthropic]
api_key = "sk-test"
model = "claude-sonnet-4-20250514"
base_url = "https://api.anthropic.com"
reasoning_effort = "max"
"#,
        )
        .unwrap();

        let error = resolve_config(&empty_cli_args(), &toml_cfg, None)
            .unwrap_err()
            .to_string();
        assert!(error.contains("reasoning_effort is only supported for provider"));
    }

    #[test]
    fn accepts_reasoning_effort_for_deepseek() {
        let toml_cfg: TactTomlConfig = toml::from_str(
            r#"
[llm]
provider = "deepseek"

[llm.providers.deepseek]
api_key = "sk-test"
model = "deepseek-chat"
reasoning_effort = "max"
"#,
        )
        .unwrap();

        let resolved = resolve_config(&empty_cli_args(), &toml_cfg, None).unwrap();
        assert_eq!(
            resolved.llm.reasoning_effort,
            Some(tact_llm::OpenAiReasoningEffort::Max)
        );
    }

    #[test]
    fn reject_unknown_openai_reasoning_effort() {
        let error = toml::from_str::<TactTomlConfig>(
            r#"
[llm]
provider = "openai"

[llm.providers.openai]
api_key = "sk-test"
model = "gpt-5"
reasoning_effort = "extreme"
"#,
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("unknown variant `extreme`"));
    }

    #[test]
    fn reject_deepseek_responses_protocol() {
        let toml_cfg: TactTomlConfig = toml::from_str(
            r#"
[llm]
provider = "deepseek"

[llm.providers.deepseek]
api_key = "sk-test"
model = "deepseek-chat"
protocol = "responses"
"#,
        )
        .unwrap();

        let error = resolve_config(&empty_cli_args(), &toml_cfg, None)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("DeepSeek Responses"),
            "expected DeepSeek Responses rejection, got: {error}"
        );
        assert!(
            error.contains("protocol = \"openai\""),
            "expected hint to switch to Chat Completions, got: {error}"
        );
    }

    #[test]
    fn reject_subagent_deepseek_responses_protocol() {
        let toml_cfg: TactTomlConfig = toml::from_str(
            r#"
[llm]
provider = "openai"

[llm.providers.openai]
api_key = "sk-test"
model = "gpt-5"

[llm.providers.deepseek]
api_key = "sk-test"
model = "deepseek-chat"
protocol = "responses"

[agent.subagent]
provider = "deepseek"
model = "deepseek-chat"
"#,
        )
        .unwrap();

        let error = resolve_config(&empty_cli_args(), &toml_cfg, None)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("subagent") && error.contains("DeepSeek Responses"),
            "expected subagent DeepSeek Responses rejection, got: {error}"
        );
        assert!(
            error.contains("protocol = \"openai\""),
            "expected hint to switch to Chat Completions, got: {error}"
        );
    }

    #[test]
    fn reject_responses_protocol_for_anthropic() {
        let toml_cfg: TactTomlConfig = toml::from_str(
            r#"
[llm]
provider = "anthropic"

[llm.providers.anthropic]
api_key = "sk-test"
model = "claude-sonnet-4-20250514"
base_url = "https://api.anthropic.com"
protocol = "responses"
"#,
        )
        .unwrap();

        let error = resolve_config(&empty_cli_args(), &toml_cfg, None)
            .unwrap_err()
            .to_string();
        assert!(error.contains(
            "only supported for provider 'openai', 'deepseek' or custom OpenAI-compatible providers"
        ));
    }

    #[test]
    fn reject_responses_protocol_for_kimi() {
        let toml_cfg: TactTomlConfig = toml::from_str(
            r#"
[llm]
provider = "kimi"

[llm.providers.kimi]
api_key = "sk-test"
model = "kimi-for-coding"
protocol = "responses"
"#,
        )
        .unwrap();

        let error = resolve_config(&empty_cli_args(), &toml_cfg, None)
            .unwrap_err()
            .to_string();
        assert!(error.contains(
            "only supported for provider 'openai', 'deepseek' or custom OpenAI-compatible providers"
        ));
    }

    #[test]
    fn resolve_instruction_sources_from_toml() {
        let toml_cfg: TactTomlConfig = toml::from_str(
            r#"
[agent]
instruction_sources = ["agents_md"]
"#,
        )
        .unwrap();
        let resolved = resolve_non_llm_settings(&empty_cli_args(), &toml_cfg, None).unwrap();
        assert!(resolved.agent.instruction_sources.agents_md);
    }

    #[test]
    fn resolve_skill_dirs_from_toml() {
        let toml_cfg: TactTomlConfig = toml::from_str(
            r#"
[agent]
skill_dirs = ["~/shared-skills", "./vendor/skills"]
"#,
        )
        .unwrap();
        let resolved = resolve_non_llm_settings(&empty_cli_args(), &toml_cfg, None).unwrap();
        assert_eq!(
            resolved.agent.skill_dirs,
            vec!["~/shared-skills".to_string(), "./vendor/skills".to_string()]
        );
    }

    #[test]
    fn resolve_skill_description_cap_defaults_to_200() {
        let toml_cfg: TactTomlConfig = toml::from_str("[agent]\n").unwrap();
        let resolved = resolve_non_llm_settings(&empty_cli_args(), &toml_cfg, None).unwrap();
        assert_eq!(
            resolved.agent.skill_description_cap,
            crate::skill::DEFAULT_SKILL_DESCRIPTION_CAP
        );
        assert!(resolved.agent.skill_disable.is_empty());
    }

    #[test]
    fn resolve_skill_catalog_keys_from_toml() {
        let toml_cfg: TactTomlConfig = toml::from_str(
            r#"
[agent]
skill_description_cap = 0
skill_disable = ["cut-the-curve", "plugin:brainstorming"]
"#,
        )
        .unwrap();
        let resolved = resolve_non_llm_settings(&empty_cli_args(), &toml_cfg, None).unwrap();
        assert_eq!(resolved.agent.skill_description_cap, 0);
        assert_eq!(
            resolved.agent.skill_disable,
            vec![
                "cut-the-curve".to_string(),
                "plugin:brainstorming".to_string()
            ]
        );
    }

    /// The override stays an unresolved string: `~` and relative forms need the
    /// workdir, which resolution does not have. Expansion is covered in
    /// `crate::memory`.
    #[test]
    fn resolve_auto_memory_directory_from_toml() {
        let toml_cfg: TactTomlConfig = toml::from_str(
            r#"
[agent]
auto_memory_directory = "~/my-memories"
"#,
        )
        .unwrap();
        let resolved = resolve_non_llm_settings(&empty_cli_args(), &toml_cfg, None).unwrap();
        assert_eq!(
            resolved.agent.auto_memory_directory.as_deref(),
            Some("~/my-memories")
        );

        // Absent and blank both mean "derive the per-repository directory".
        let resolved =
            resolve_non_llm_settings(&empty_cli_args(), &TactTomlConfig::default(), None).unwrap();
        assert_eq!(resolved.agent.auto_memory_directory, None);

        let toml_cfg: TactTomlConfig = toml::from_str(
            r#"
[agent]
auto_memory_directory = "   "
"#,
        )
        .unwrap();
        let resolved = resolve_non_llm_settings(&empty_cli_args(), &toml_cfg, None).unwrap();
        assert_eq!(resolved.agent.auto_memory_directory, None);
    }

    #[test]
    fn resolve_vision_image_from_toml() {
        let toml_cfg: TactTomlConfig = toml::from_str(
            r#"
[ui.vision_image]
compress = false
max_edge = 1024
jpeg_quality = 70
"#,
        )
        .unwrap();
        let resolved = resolve_non_llm_settings(&empty_cli_args(), &toml_cfg, None).unwrap();
        assert!(!resolved.ui.vision_image.compress);
        assert_eq!(resolved.ui.vision_image.max_edge, 1024);
        assert_eq!(resolved.ui.vision_image.jpeg_quality, 70);
    }

    #[test]
    fn resolve_vision_image_clamps_out_of_range() {
        let toml_cfg: TactTomlConfig = toml::from_str(
            r#"
[ui.vision_image]
max_edge = 99999
jpeg_quality = 0
"#,
        )
        .unwrap();
        let resolved = resolve_non_llm_settings(&empty_cli_args(), &toml_cfg, None).unwrap();
        assert_eq!(resolved.ui.vision_image.max_edge, 4096);
        assert_eq!(resolved.ui.vision_image.jpeg_quality, 1);
    }

    #[test]
    fn bash_timeout_defaults_to_thirty_minutes_and_zero_is_preserved() {
        let default =
            resolve_non_llm_settings(&empty_cli_args(), &TactTomlConfig::default(), None).unwrap();
        assert_eq!(default.tools.bash_timeout_secs, 1_800);

        let cfg: TactTomlConfig = toml::from_str("[tools]\nbash_timeout_secs = 0\n").unwrap();
        let disabled = resolve_non_llm_settings(&empty_cli_args(), &cfg, None).unwrap();
        assert_eq!(disabled.tools.bash_timeout_secs, 0);
    }

    #[test]
    fn sandbox_is_an_opt_in_switch() {
        // Absent key: the sandbox is off, so a default install is unchanged.
        let default =
            resolve_non_llm_settings(&empty_cli_args(), &TactTomlConfig::default(), None).unwrap();
        assert!(!default.tools.sandbox);

        let cfg: TactTomlConfig = toml::from_str("[tools]\nsandbox = true\n").unwrap();
        let opted_in = resolve_non_llm_settings(&empty_cli_args(), &cfg, None).unwrap();
        assert!(opted_in.tools.sandbox);

        let cfg: TactTomlConfig = toml::from_str("[tools]\nsandbox = false\n").unwrap();
        let opted_out = resolve_non_llm_settings(&empty_cli_args(), &cfg, None).unwrap();
        assert!(!opted_out.tools.sandbox);

        // Backend names are no longer part of the config surface; the old
        // spelling must be a parse error rather than a silently ignored key.
        for bad in [
            "[tools]\nsandbox = \"bwrap\"\n",
            "[tools]\nsandbox = \"none\"\n",
        ] {
            assert!(
                toml::from_str::<TactTomlConfig>(bad).is_err(),
                "{bad:?} should not parse: the switch is a boolean"
            );
        }
    }

    #[test]
    fn resolve_deepseek_config_from_toml() {
        let toml_cfg: TactTomlConfig = toml::from_str(
            r#"
[llm]
provider = "deepseek"

[llm.providers.deepseek]
api_key = "sk-test"
model = "deepseek-chat"
"#,
        )
        .unwrap();
        let resolved = resolve_config(&empty_cli_args(), &toml_cfg, None).unwrap();
        assert_eq!(resolved.llm.provider, ProviderKind::DeepSeek);
        assert_eq!(resolved.llm.api_key, "sk-test");
        assert_eq!(resolved.llm.model, "deepseek-chat");
        assert_eq!(resolved.llm.base_url, "https://api.deepseek.com");
    }

    #[test]
    fn resolve_kimi_from_providers_map() {
        let toml_cfg: TactTomlConfig = toml::from_str(
            r#"
[llm]
provider = "kimi"

[llm.providers.kimi]
api_key = "mk-test"
model = "kimi-k2.5"
"#,
        )
        .unwrap();
        let resolved = resolve_config(&empty_cli_args(), &toml_cfg, None).unwrap();
        assert_eq!(resolved.llm.provider, ProviderKind::Kimi);
        assert_eq!(resolved.llm.api_key, "mk-test");
        assert_eq!(resolved.llm.model, "kimi-k2.5");
        assert_eq!(resolved.llm.base_url, "https://api.moonshot.cn/v1");
        // No `max_tokens` anywhere → the Kimi K2.x default.
        assert_eq!(resolved.agent.max_tokens, 32_000);
    }

    #[test]
    fn resolve_copies_model_profile_supports_vision() {
        let toml_cfg: TactTomlConfig = toml::from_str(
            r#"
[llm]
provider = "openai"

[llm.providers.openai]
api_key = "sk-test"
base_url = "https://opencode.ai/zen/go/v1"
model = "deepseek-flash"

[llm.model_profiles."deepseek-flash"]
supports_vision = true

[llm.model_profiles."deepseek-v4-flash"]
supports_vision = false
"#,
        )
        .unwrap();
        let resolved = resolve_config(&empty_cli_args(), &toml_cfg, None).unwrap();
        assert_eq!(
            resolved.llm.model_profiles["deepseek-flash"].supports_vision,
            Some(true)
        );
        assert_eq!(
            resolved.llm.model_profiles["deepseek-v4-flash"].supports_vision,
            Some(false)
        );
        // No TOML entry for this model → no override; the endpoint heuristic
        // keeps deciding.
        assert_eq!(
            resolved.llm.model_profiles["deepseek-v4-pro"].supports_vision,
            None
        );
    }

    #[test]
    fn model_profile_vision_override_keeps_builtin_tiers() {
        use tact_llm::OpenAiReasoningEffort as E;

        let toml_cfg: TactTomlConfig = toml::from_str(
            r#"
[llm]
provider = "openai"

[llm.providers.openai]
api_key = "sk-test"
model = "deepseek-v4-flash"

[llm.model_profiles."deepseek-v4-flash"]
supports_vision = false
"#,
        )
        .unwrap();
        let resolved = resolve_config(&empty_cli_args(), &toml_cfg, None).unwrap();
        let profile = &resolved.llm.model_profiles["deepseek-v4-flash"];
        // The override lands, and the built-in effort tiers survive the merge.
        assert_eq!(profile.supports_vision, Some(false));
        assert_eq!(
            profile.reasoning_efforts,
            vec![E::Low, E::High, E::Max],
            "a vision-only TOML entry must not wipe the built-in tiers"
        );
    }

    #[test]
    fn resolve_copies_provider_models_list() {
        let toml_cfg: TactTomlConfig = toml::from_str(
            r#"
[llm]
provider = "kimi"

[llm.providers.kimi]
api_key = "mk-test"
model = "kimi-k2.5"
models = ["kimi-k2.5", "kimi-for-coding"]
"#,
        )
        .unwrap();
        let resolved = resolve_config(&empty_cli_args(), &toml_cfg, None).unwrap();
        assert_eq!(
            resolved.llm.models,
            vec!["kimi-k2.5".to_string(), "kimi-for-coding".to_string()]
        );
    }

    #[test]
    fn cli_provider_switches_entry() {
        let toml_cfg: TactTomlConfig = toml::from_str(
            r#"
[llm]
provider = "kimi"

[llm.providers.kimi]
api_key = "mk-test"
model = "kimi-k2.5"

[llm.providers.openai]
api_key = "sk-test"
model = "gpt-4o"
"#,
        )
        .unwrap();
        let mut args = empty_cli_args();
        args.provider = Some("openai".to_string());
        let resolved = resolve_config(&args, &toml_cfg, None).unwrap();
        assert_eq!(resolved.llm.provider, ProviderKind::OpenAi);
        assert_eq!(resolved.llm.api_key, "sk-test");
        assert_eq!(resolved.llm.model, "gpt-4o");
    }

    #[test]
    fn per_provider_max_tokens_overrides_default() {
        let toml_cfg: TactTomlConfig = toml::from_str(
            r#"
[llm]
provider = "openai"

[llm.providers.openai]
api_key = "sk-test"
model = "gpt-4o"
max_tokens = 32000
"#,
        )
        .unwrap();
        let resolved = resolve_config(&empty_cli_args(), &toml_cfg, None).unwrap();
        assert_eq!(resolved.agent.max_tokens, 32000);
    }

    #[test]
    fn cli_max_tokens_overrides_entry() {
        let toml_cfg: TactTomlConfig = toml::from_str(
            r#"
[llm]
provider = "openai"

[llm.providers.openai]
api_key = "sk-test"
model = "gpt-4o"
max_tokens = 32000
"#,
        )
        .unwrap();
        let mut args = empty_cli_args();
        args.max_tokens = Some(1000);
        let resolved = resolve_config(&args, &toml_cfg, None).unwrap();
        assert_eq!(resolved.agent.max_tokens, 1000);
    }

    /// `[agent].max_tokens` is the fallback when the provider entry has none:
    /// it outranks the built-in default.
    #[test]
    fn agent_max_tokens_overrides_default() {
        let toml_cfg: TactTomlConfig = toml::from_str(
            r#"
[llm]
provider = "openai"

[llm.providers.openai]
api_key = "sk-test"
model = "gpt-4o"

[agent]
max_tokens = 64000
"#,
        )
        .unwrap();
        let resolved = resolve_config(&empty_cli_args(), &toml_cfg, None).unwrap();
        assert_eq!(resolved.agent.max_tokens, 64_000);
    }

    /// The removed `[llm].max_tokens` key is a hard error, not a silent drop.
    ///
    /// A dropped key would let the request fall back to the built-in default
    /// (8000 here) with nothing in the output to say so — the same
    /// "configured but ignored" trap the key's removal is meant to end.
    #[test]
    fn llm_max_tokens_is_rejected() {
        let toml_cfg: TactTomlConfig = toml::from_str(
            r#"
[llm]
provider = "openai"
max_tokens = 64000

[llm.providers.openai]
api_key = "sk-test"
model = "gpt-4o"
"#,
        )
        .unwrap();
        let err = resolve_config(&empty_cli_args(), &toml_cfg, None)
            .unwrap_err()
            .to_string();
        assert!(err.contains("[llm].max_tokens was removed"), "got: {err}");
        assert!(err.contains("[agent].max_tokens"), "got: {err}");
    }

    /// A config that never sets the removed key still resolves.
    #[test]
    fn absent_llm_max_tokens_still_resolves() {
        let toml_cfg = openai_toml_config();
        let resolved = resolve_config(&empty_cli_args(), &toml_cfg, None).unwrap();
        assert_eq!(resolved.agent.max_tokens, 8_000);
    }

    /// `[agent]` carries no thinking keys: writing them there used to be a
    /// silent no-op, so the session ran at default thinking settings with
    /// nothing in the output to say so.
    ///
    /// The report is serde's unknown-field error. Note it lists only the *real*
    /// agent fields — `thinking_budget` / `reasoning_effort` are absent, which is
    /// what points the reader at `[llm]` (see Ch 21 §3).
    #[test]
    fn agent_thinking_keys_are_rejected() {
        for key in [
            "thinking_budget = 8000",
            // Wrong type on purpose: the report is "unknown field", not a type
            // error, because the key itself does not exist here.
            "thinking_budget = \"abc\"",
            "reasoning_effort = \"low\"",
            "reasoning_effort = 123",
        ] {
            let err = toml::from_str::<TactTomlConfig>(&format!(
                r#"
[llm]
provider = "openai"

[llm.providers.openai]
api_key = "sk-test"
model = "gpt-4o"

[agent]
{key}
"#
            ))
            .unwrap_err()
            .to_string();
            let name = key.split_whitespace().next().unwrap();
            assert!(err.contains("unknown field"), "{key}: {err}");
            assert!(err.contains(name), "{key}: {err}");
            // The valid-field list must not advertise a key that is rejected.
            let list = err.split("expected one of").nth(1).unwrap_or_default();
            assert!(!list.contains(name), "{key}: {err}");
        }
    }

    /// Any other unknown `[agent]` key is rejected by serde, not dropped.
    #[test]
    fn agent_unknown_key_is_rejected() {
        // `model` is a runtime agent field, not a TOML one — the same shape of
        // mistake as `thinking_budget` above, caught one layer earlier.
        let err = toml::from_str::<TactTomlConfig>(
            r#"
[llm]
provider = "openai"

[llm.providers.openai]
api_key = "sk-test"
model = "gpt-4o"

[agent]
model = "gpt-4o"
"#,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("unknown field"), "got: {err}");
        assert!(err.contains("model"), "got: {err}");
    }

    /// The same guard applies inside `[agent.subagent]`.
    #[test]
    fn subagent_unknown_key_is_rejected() {
        let err = toml::from_str::<TactTomlConfig>(
            r#"
[llm]
provider = "openai"

[llm.providers.openai]
api_key = "sk-test"
model = "gpt-4o"

[agent.subagent]
provider = "openai"
max_token = 64000
"#,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("unknown field"), "got: {err}");
        assert!(err.contains("max_token"), "got: {err}");
    }

    /// The provider entry still wins over `[agent].max_tokens`.
    #[test]
    fn per_provider_max_tokens_overrides_agent() {
        let toml_cfg: TactTomlConfig = toml::from_str(
            r#"
[llm]
provider = "openai"

[llm.providers.openai]
api_key = "sk-test"
model = "gpt-4o"
max_tokens = 32000

[agent]
max_tokens = 64000
"#,
        )
        .unwrap();
        let resolved = resolve_config(&empty_cli_args(), &toml_cfg, None).unwrap();
        assert_eq!(resolved.agent.max_tokens, 32_000);
    }

    /// `--max-tokens` still wins over everything, `[agent]` included.
    #[test]
    fn cli_max_tokens_overrides_agent() {
        let toml_cfg: TactTomlConfig = toml::from_str(
            r#"
[llm]
provider = "openai"

[llm.providers.openai]
api_key = "sk-test"
model = "gpt-4o"

[agent]
max_tokens = 64000
"#,
        )
        .unwrap();
        let mut args = empty_cli_args();
        args.max_tokens = Some(1000);
        let resolved = resolve_config(&args, &toml_cfg, None).unwrap();
        assert_eq!(resolved.agent.max_tokens, 1000);
    }

    /// A subagent without its own `max_tokens` inherits the resolved main value,
    /// which now includes the `[agent]` level.
    #[test]
    fn subagent_inherits_agent_max_tokens() {
        let toml_cfg: TactTomlConfig = toml::from_str(
            r#"
[llm]
provider = "openai"

[llm.providers.openai]
api_key = "sk-test"
model = "gpt-4o"

[agent]
max_tokens = 64000

[agent.subagent]
provider = "openai"
"#,
        )
        .unwrap();
        let resolved = resolve_config(&empty_cli_args(), &toml_cfg, None).unwrap();
        assert_eq!(resolved.agent.max_tokens, 64_000);
        assert_eq!(resolved.agent.subagent.unwrap().max_tokens, 64_000);
    }

    #[test]
    fn anthropic_without_base_url_errors() {
        let toml_cfg: TactTomlConfig = toml::from_str(
            r#"
[llm]
provider = "anthropic"

[llm.providers.anthropic]
api_key = "sk-ant-test"
model = "claude-sonnet-4-20250514"
"#,
        )
        .unwrap();
        let err = resolve_config(&empty_cli_args(), &toml_cfg, None)
            .unwrap_err()
            .to_string();
        assert!(err.contains("base_url"));
    }

    #[test]
    fn missing_llm_provider_errors() {
        let toml_cfg: TactTomlConfig = toml::from_str(
            r#"
[llm.providers.openai]
api_key = "sk-test"
model = "gpt-4o"
"#,
        )
        .unwrap();
        let err = resolve_config(&empty_cli_args(), &toml_cfg, None)
            .unwrap_err()
            .to_string();
        assert!(err.contains("LLM provider not configured"));
    }

    #[test]
    fn per_provider_thinking_budget_overrides_global() {
        let toml_cfg: TactTomlConfig = toml::from_str(
            r#"
[llm]
provider = "openai"
thinking_budget = 32000

[llm.providers.openai]
api_key = "sk-test"
model = "some-unknown-model"
thinking_budget = 64000

[agent]
max_tokens = 128000
"#,
        )
        .unwrap();
        let resolved = resolve_config(&empty_cli_args(), &toml_cfg, None).unwrap();
        assert_eq!(resolved.agent.thinking_budget, 64000);
    }

    #[test]
    fn missing_api_key_on_active_entry_errors() {
        let toml_cfg: TactTomlConfig = toml::from_str(
            r#"
[llm]
provider = "openai"

[llm.providers.openai]
model = "gpt-4o"
"#,
        )
        .unwrap();
        let err = resolve_config(&empty_cli_args(), &toml_cfg, None)
            .unwrap_err()
            .to_string();
        assert!(err.contains("api_key"));
    }

    #[test]
    fn custom_provider_in_map_resolves() {
        let toml_cfg: TactTomlConfig = toml::from_str(
            r#"
[llm]
provider = "openai"

[llm.providers.openai]
api_key = "sk-test"
model = "gpt-4o"

[llm.providers.moonshot]
api_key = "mk-test"
model = "kimi-k2.5"
"#,
        )
        .unwrap();
        let resolved = resolve_config(&empty_cli_args(), &toml_cfg, None).unwrap();
        assert_eq!(resolved.llm.provider, ProviderKind::OpenAi);
    }

    #[test]
    fn missing_provider_entry_errors() {
        let toml_cfg: TactTomlConfig = toml::from_str(
            r#"
[llm]
provider = "deepseek"

[llm.providers.kimi]
api_key = "mk-test"
model = "kimi-k2.5"
"#,
        )
        .unwrap();
        let err = resolve_config(&empty_cli_args(), &toml_cfg, None)
            .unwrap_err()
            .to_string();
        assert!(err.contains("not found in llm.providers"));
    }

    #[test]
    fn custom_provider_resolves_with_openai_protocol() {
        let toml_cfg: TactTomlConfig = toml::from_str(
            r#"
[llm]
provider = "foo"

[llm.providers.foo]
api_key = "sk-test"
base_url = "https://foo.example.com/v1"
model = "foo-1"
"#,
        )
        .unwrap();
        let resolved = resolve_config(&empty_cli_args(), &toml_cfg, None).unwrap();
        assert_eq!(
            resolved.llm.provider,
            ProviderKind::Custom("foo".to_string())
        );
        assert_eq!(resolved.llm.protocol, OpenAiProtocol::ChatCompletions);
        assert_eq!(resolved.llm.base_url, "https://foo.example.com/v1");
    }

    #[test]
    fn custom_provider_without_base_url_errors() {
        let toml_cfg: TactTomlConfig = toml::from_str(
            r#"
[llm]
provider = "foo"

[llm.providers.foo]
api_key = "sk-test"
model = "foo-1"
"#,
        )
        .unwrap();
        let err = resolve_config(&empty_cli_args(), &toml_cfg, None)
            .unwrap_err()
            .to_string();
        assert!(err.contains("base_url not configured"));
    }

    #[test]
    fn resolve_config_requires_model() {
        let toml_cfg: TactTomlConfig = toml::from_str(
            r#"
[llm]
provider = "openai"

[llm.providers.openai]
api_key = "sk-test"
"#,
        )
        .unwrap();
        let err = resolve_config(&empty_cli_args(), &toml_cfg, None)
            .unwrap_err()
            .to_string();
        assert!(err.contains("model not configured"));
    }

    #[test]
    fn list_sessions_does_not_require_llm() {
        let mut args = empty_cli_args();
        args.list_sessions = true;
        args.theme = Some("nord".to_string());
        let resolved = resolve_non_llm_settings(&args, &TactTomlConfig::default(), None).unwrap();
        assert_eq!(resolved.ui.theme, "nord");
        assert!(resolved.llm.api_key.is_empty());
    }

    /// `[agent] max_token_usage_bodies` defaults to the store fallback (1), so a
    /// session keeps exactly one request body unless the user asks for more.
    #[test]
    fn resolve_token_usage_bodies_defaults_to_one_and_is_overridable() {
        let toml_cfg: TactTomlConfig = toml::from_str(
            r#"
[llm]
provider = "openai"

[llm.providers.openai]
api_key = "sk-test"
model = "some-unknown-model"
"#,
        )
        .unwrap();
        let resolved = resolve_config(&empty_cli_args(), &toml_cfg, None).unwrap();
        assert_eq!(resolved.agent.max_token_usage_bodies, 1);
        assert_eq!(
            resolved.agent.max_token_usage_bodies,
            crate::store::session_store::MAX_TOKEN_USAGE_BODIES
        );

        let toml_cfg: TactTomlConfig = toml::from_str(
            r#"
[llm]
provider = "openai"

[llm.providers.openai]
api_key = "sk-test"
model = "some-unknown-model"

[agent]
max_token_usage_bodies = 25
"#,
        )
        .unwrap();
        let resolved = resolve_config(&empty_cli_args(), &toml_cfg, None).unwrap();
        assert_eq!(resolved.agent.max_token_usage_bodies, 25);
    }

    #[test]
    fn resolve_model_context_window_defaults_to_200k() {
        let toml_cfg: TactTomlConfig = toml::from_str(
            r#"
[llm]
provider = "openai"

[llm.providers.openai]
api_key = "sk-test"
model = "some-unknown-model"
"#,
        )
        .unwrap();
        let resolved = resolve_config(&empty_cli_args(), &toml_cfg, None).unwrap();
        assert_eq!(resolved.agent.model_context_window, 200_000);
    }

    #[test]
    fn resolve_model_context_window_from_toml() {
        let toml_cfg: TactTomlConfig = toml::from_str(
            r#"
[llm]
provider = "openai"

[llm.providers.openai]
api_key = "sk-test"
model = "some-unknown-model"

[agent]
model_context_window = 128000
"#,
        )
        .unwrap();
        let resolved = resolve_config(&empty_cli_args(), &toml_cfg, None).unwrap();
        assert_eq!(resolved.agent.model_context_window, 128_000);
    }

    #[test]
    fn resolve_model_context_window_toml_overrides_mapping() {
        // deepseek-v4-pro has a well-known 1M window, but an explicit config
        // value wins: the mapping is only a fallback.
        let toml_cfg: TactTomlConfig = toml::from_str(
            r#"
[llm]
provider = "openai"

[llm.providers.openai]
api_key = "sk-test"
model = "deepseek-v4-pro"

[agent]
model_context_window = 128000
"#,
        )
        .unwrap();
        let resolved = resolve_config(&empty_cli_args(), &toml_cfg, None).unwrap();
        assert_eq!(resolved.agent.model_context_window, 128_000);
    }

    #[test]
    fn resolve_model_context_window_cli_overrides_toml_and_mapping() {
        let toml_cfg: TactTomlConfig = toml::from_str(
            r#"
[llm]
provider = "openai"

[llm.providers.openai]
api_key = "sk-test"
model = "deepseek-v4-pro"

[agent]
model_context_window = 128000
"#,
        )
        .unwrap();
        let mut args = empty_cli_args();
        args.model_context_window = Some(64_000);
        let resolved = resolve_config(&args, &toml_cfg, None).unwrap();
        assert_eq!(resolved.agent.model_context_window, 64_000);
    }

    #[test]
    fn resolve_model_context_window_mapping_is_the_fallback() {
        // No CLI flag, no `[agent]` key → the built-in table still applies.
        let toml_cfg: TactTomlConfig = toml::from_str(
            r#"
[llm]
provider = "openai"

[llm.providers.openai]
api_key = "sk-test"
model = "deepseek-v4-pro"
"#,
        )
        .unwrap();
        let resolved = resolve_config(&empty_cli_args(), &toml_cfg, None).unwrap();
        assert_eq!(resolved.agent.model_context_window, 1_000_000);
    }

    #[test]
    fn resolve_model_context_window_maps_deepseek_v4_variants() {
        // Experiment / vision suffixes and dot-separated minor versions must
        // not fall through to the 200K default: every `deepseek-v4-*` /
        // `deepseek-v4.*` id is a 1M-window model.
        for model in [
            "deepseek-v4-flash-version-exp",
            "deepseek-v4-flash-vision-exp",
            "deepseek-v4-pro-2026",
            "deepseek-v4.1-flash",
            "deepseek-flash",
        ] {
            let toml_cfg: TactTomlConfig = toml::from_str(&format!(
                r#"
[llm]
provider = "openai"

[llm.providers.openai]
api_key = "sk-test"
model = "{model}"
"#
            ))
            .unwrap();
            let resolved = resolve_config(&empty_cli_args(), &toml_cfg, None).unwrap();
            assert_eq!(
                resolved.agent.model_context_window, 1_000_000,
                "expected 1M window for {model}"
            );
        }
    }

    #[test]
    fn resolve_model_context_window_maps_k3_256k() {
        let toml_cfg: TactTomlConfig = toml::from_str(
            r#"
[llm]
provider = "openai"

[llm.providers.openai]
api_key = "sk-test"
model = "k3-256k"
"#,
        )
        .unwrap();
        let resolved = resolve_config(&empty_cli_args(), &toml_cfg, None).unwrap();
        assert_eq!(resolved.agent.model_context_window, 256_000);
    }

    #[test]
    fn resolve_model_context_window_maps_openai_models() {
        // Official (developers.openai.com/api/docs/models): GPT-5.6 family and
        // GPT-5.5 are 1.05M; GPT-5.4 is 1M; GPT-5 … GPT-5.3 + GPT-5.4-mini are
        // 400K; GPT-4o family is 128K.
        let cases = [
            ("gpt-5.6", 1_050_000),
            ("gpt-5.6-luna", 1_050_000),
            ("gpt-5.6-terra", 1_050_000),
            ("gpt-5.6-sol", 1_050_000),
            ("gpt-5.5", 1_050_000),
            ("gpt-5.4", 1_000_000),
            ("gpt-5", 400_000),
            ("gpt-5.1", 400_000),
            ("gpt-5.2", 400_000),
            ("gpt-5.3", 400_000),
            ("gpt-5.3-codex", 400_000),
            ("gpt-5.4-mini", 400_000),
            ("gpt-4o", 128_000),
            ("gpt-4o-mini", 128_000),
        ];
        for (model, expected) in cases {
            let toml_cfg: TactTomlConfig = toml::from_str(&format!(
                r#"
[llm]
provider = "openai"

[llm.providers.openai]
api_key = "sk-test"
model = "{model}"
"#
            ))
            .unwrap();
            let resolved = resolve_config(&empty_cli_args(), &toml_cfg, None).unwrap();
            assert_eq!(
                resolved.agent.model_context_window, expected,
                "model {model}"
            );
        }
    }

    #[test]
    fn resolve_model_context_window_maps_claude_models() {
        // Official (platform.claude.com docs models overview + help center):
        // Sonnet 5 / Fable 5 / Opus 5 / Opus 4.6–4.8 / Sonnet 4.6 are 1M
        // (API and Claude Code); Sonnet 4 / Opus 4 / Haiku 4.5 are 200K.
        let cases = [
            ("claude-sonnet-5", 1_000_000),
            ("claude-fable-5", 1_000_000),
            ("claude-opus-5", 1_000_000),
            ("claude-opus-4-8", 1_000_000),
            ("claude-opus-4-7", 1_000_000),
            ("claude-opus-4-6", 1_000_000),
            ("claude-sonnet-4-6", 1_000_000),
            ("claude-sonnet-4-20250514", 200_000),
            ("claude-opus-4-20250514", 200_000),
            ("claude-haiku-4-5", 200_000),
            ("claude-haiku-4-20250514", 200_000),
        ];
        for (model, expected) in cases {
            let toml_cfg: TactTomlConfig = toml::from_str(&format!(
                r#"
[llm]
provider = "anthropic"

[llm.providers.anthropic]
api_key = "sk-test"
model = "{model}"
base_url = "https://api.anthropic.com"
"#
            ))
            .unwrap();
            let resolved = resolve_config(&empty_cli_args(), &toml_cfg, None).unwrap();
            assert_eq!(
                resolved.agent.model_context_window, expected,
                "model {model}"
            );
        }
    }

    #[test]
    fn resolve_model_context_window_unknown_model_falls_through_to_toml() {
        let toml_cfg: TactTomlConfig = toml::from_str(
            r#"
[llm]
provider = "openai"

[llm.providers.openai]
api_key = "sk-test"
model = "some-unknown-model"

[agent]
model_context_window = 777_000
"#,
        )
        .unwrap();
        let resolved = resolve_config(&empty_cli_args(), &toml_cfg, None).unwrap();
        assert_eq!(resolved.agent.model_context_window, 777_000);
    }

    #[test]
    fn max_tokens_equal_to_model_context_window_errors() {
        let toml_cfg: TactTomlConfig = toml::from_str(
            r#"
[llm]
provider = "openai"

[llm.providers.openai]
api_key = "sk-test"
model = "some-unknown-model"

[agent]
max_tokens = 8000
model_context_window = 8000
"#,
        )
        .unwrap();

        let err = resolve_config(&empty_cli_args(), &toml_cfg, None)
            .unwrap_err()
            .to_string();
        assert_eq!(
            err,
            "invalid token limits: max_tokens (8000) must be less than agent.model_context_window (8000)"
        );
    }

    #[test]
    fn resolved_cli_max_tokens_above_model_context_window_errors() {
        let toml_cfg: TactTomlConfig = toml::from_str(
            r#"
[llm]
provider = "openai"

[llm.providers.openai]
api_key = "sk-test"
model = "some-unknown-model"

[agent]
max_tokens = 1000
model_context_window = 8000
"#,
        )
        .unwrap();
        let mut args = empty_cli_args();
        args.max_tokens = Some(9000);

        let err = resolve_config(&args, &toml_cfg, None)
            .unwrap_err()
            .to_string();
        assert!(err.contains("max_tokens (9000)"));
        assert!(err.contains("agent.model_context_window (8000)"));
    }

    #[test]
    fn max_tokens_below_model_context_window_is_valid() {
        let toml_cfg: TactTomlConfig = toml::from_str(
            r#"
[llm]
provider = "openai"

[llm.providers.openai]
api_key = "sk-test"
model = "some-unknown-model"

[agent]
max_tokens = 7999
model_context_window = 8000
"#,
        )
        .unwrap();

        let resolved = resolve_config(&empty_cli_args(), &toml_cfg, None).unwrap();
        assert_eq!(resolved.agent.max_tokens, 7999);
        assert_eq!(resolved.agent.model_context_window, 8000);
    }

    #[test]
    fn zero_model_context_window_skips_max_tokens_validation() {
        let toml_cfg: TactTomlConfig = toml::from_str(
            r#"
[llm]
provider = "openai"

[llm.providers.openai]
api_key = "sk-test"
model = "some-unknown-model"

[agent]
max_tokens = 32000
model_context_window = 0
"#,
        )
        .unwrap();

        let resolved = resolve_config(&empty_cli_args(), &toml_cfg, None).unwrap();
        assert_eq!(resolved.agent.max_tokens, 32_000);
        assert_eq!(resolved.agent.model_context_window, 0);
    }

    #[test]
    fn responses_threshold_derived_from_window_max_tokens_and_headroom() {
        let toml_cfg: TactTomlConfig = toml::from_str(
            r#"
[llm]
provider = "openai"

[llm.providers.openai]
api_key = "sk-test"
model = "some-unknown-model"
protocol = "responses"

[agent]
max_tokens = 8000
model_context_window = 200000
"#,
        )
        .unwrap();
        let resolved = resolve_config(&empty_cli_args(), &toml_cfg, None).unwrap();
        // headroom = 200000 * 10% = 20000; threshold = 200000 - 8000 - 20000.
        assert_eq!(resolved.llm.responses_compact_threshold, Some(172_000));
    }

    #[test]
    fn responses_configured_threshold_is_resolved() {
        let toml_cfg: TactTomlConfig = toml::from_str(
            r#"
[llm]
provider = "openai"

[llm.providers.openai]
api_key = "sk-test"
model = "some-unknown-model"
protocol = "responses"
responses_compact_threshold = 160000

[agent]
max_tokens = 8000
model_context_window = 200000
"#,
        )
        .unwrap();
        let resolved = resolve_config(&empty_cli_args(), &toml_cfg, None).unwrap();
        assert_eq!(resolved.llm.responses_compact_threshold, Some(160_000));
    }

    #[test]
    fn responses_zero_configured_threshold_is_rejected() {
        let toml_cfg: TactTomlConfig = toml::from_str(
            r#"
[llm]
provider = "openai"

[llm.providers.openai]
api_key = "sk-test"
model = "some-unknown-model"
protocol = "responses"
responses_compact_threshold = 0

[agent]
max_tokens = 8000
model_context_window = 200000
"#,
        )
        .unwrap();
        let error = resolve_config(&empty_cli_args(), &toml_cfg, None)
            .unwrap_err()
            .to_string();
        assert!(error.contains("responses_compact_threshold must be positive"));
    }

    #[test]
    fn responses_configured_threshold_without_room_is_rejected() {
        let toml_cfg: TactTomlConfig = toml::from_str(
            r#"
[llm]
provider = "openai"

[llm.providers.openai]
api_key = "sk-test"
model = "some-unknown-model"
protocol = "responses"
responses_compact_threshold = 180000

[agent]
max_tokens = 8000
model_context_window = 200000
"#,
        )
        .unwrap();
        // 180000 + 8000 + 20000 = 208000 > 200000 → no room left.
        let error = resolve_config(&empty_cli_args(), &toml_cfg, None)
            .unwrap_err()
            .to_string();
        assert!(error.contains("responses_compact_threshold"));
        assert!(error.contains("leave room"));
    }

    #[test]
    fn subagent_responses_threshold_must_leave_room_for_subagent_max_tokens() {
        // The subagent reuses the provider entry's configured threshold with
        // its own max_tokens; a threshold validated against the main agent's
        // max_tokens may not leave room for a larger subagent max_tokens.
        let toml_cfg: TactTomlConfig = toml::from_str(
            r#"
[llm]
provider = "openai"

[llm.providers.openai]
api_key = "sk-test"
model = "some-unknown-model"
protocol = "responses"
responses_compact_threshold = 160000

[agent]
max_tokens = 8000
model_context_window = 200000

[agent.subagent]
provider = "openai"
model = "some-unknown-model"
max_tokens = 40000
"#,
        )
        .unwrap();
        // 160000 + 40000 + 20000 = 220000 > 200000 → no room left.
        let error = resolve_config(&empty_cli_args(), &toml_cfg, None)
            .unwrap_err()
            .to_string();
        assert!(error.contains("responses_compact_threshold"));
        assert!(error.contains("leave room"));
        assert!(error.contains("subagent"));
    }

    #[test]
    fn subagent_responses_threshold_with_room_is_accepted() {
        let toml_cfg: TactTomlConfig = toml::from_str(
            r#"
[llm]
provider = "openai"

[llm.providers.openai]
api_key = "sk-test"
model = "some-unknown-model"
protocol = "responses"
responses_compact_threshold = 160000

[agent]
max_tokens = 8000
model_context_window = 250000

[agent.subagent]
provider = "openai"
model = "some-unknown-model"
max_tokens = 40000
"#,
        )
        .unwrap();
        // 160000 + 40000 + 25000 = 225000 <= 250000 → valid.
        let resolved = resolve_config(&empty_cli_args(), &toml_cfg, None).unwrap();
        let sa = resolved.agent.subagent.expect("subagent resolved");
        assert_eq!(sa.max_tokens, 40_000);
        assert_eq!(
            sa.provider.responses_compact_threshold,
            Some(160_000),
            "subagent provider carries the validated threshold"
        );
    }

    #[test]
    fn subagent_responses_threshold_is_derived_when_omitted() {
        let config = config_with_responses_main_and_subagent(None, 8_000, 200_000);
        let resolved = resolve_config(&empty_cli_args(), &config, None).unwrap();
        let subagent = resolved.agent.subagent.unwrap();
        // headroom = 200000 * 10% = 20000; 200000 - 8000 - 20000.
        assert_eq!(subagent.provider.responses_compact_threshold, Some(172_000));
    }

    #[test]
    fn subagent_responses_threshold_is_derived_from_subagent_max_tokens() {
        // The subagent overrides max_tokens to 40000 while the main agent
        // keeps 8000; the derived threshold must use the subagent budget.
        let config = config_with_responses_main_and_subagent(None, 40_000, 200_000);
        let resolved = resolve_config(&empty_cli_args(), &config, None).unwrap();
        let subagent = resolved.agent.subagent.unwrap();
        assert_eq!(subagent.max_tokens, 40_000);
        // headroom = 20000; 200000 - 40000 - 20000.
        assert_eq!(subagent.provider.responses_compact_threshold, Some(140_000));
    }

    #[test]
    fn subagent_responses_threshold_is_none_for_non_responses_and_zero_window() {
        // Non-Responses subagent: configured threshold on the provider entry
        // must not leak into the subagent ProviderInfo.
        let mut config = config_with_responses_main_and_subagent(Some(160_000), 8_000, 200_000);
        config.llm.providers.get_mut("openai").unwrap().protocol = None;
        let resolved = resolve_config(&empty_cli_args(), &config, None).unwrap();
        let subagent = resolved.agent.subagent.unwrap();
        assert_eq!(subagent.provider.responses_compact_threshold, None);

        // Responses subagent with a zero context window resolves to None.
        let config = config_with_responses_main_and_subagent(None, 8_000, 0);
        let resolved = resolve_config(&empty_cli_args(), &config, None).unwrap();
        let subagent = resolved.agent.subagent.unwrap();
        assert_eq!(subagent.provider.responses_compact_threshold, None);
    }

    #[test]
    fn responses_threshold_is_none_when_window_is_zero() {
        let toml_cfg: TactTomlConfig = toml::from_str(
            r#"
[llm]
provider = "openai"

[llm.providers.openai]
api_key = "sk-test"
model = "some-unknown-model"
protocol = "responses"

[agent]
max_tokens = 8000
model_context_window = 0
"#,
        )
        .unwrap();
        let resolved = resolve_config(&empty_cli_args(), &toml_cfg, None).unwrap();
        assert_eq!(resolved.llm.responses_compact_threshold, None);
    }

    #[test]
    fn non_responses_provider_never_resolves_a_threshold() {
        let toml_cfg: TactTomlConfig = toml::from_str(
            r#"
[llm]
provider = "openai"

[llm.providers.openai]
api_key = "sk-test"
model = "gpt-4o"

[agent]
max_tokens = 8000
model_context_window = 200000
"#,
        )
        .unwrap();
        let resolved = resolve_config(&empty_cli_args(), &toml_cfg, None).unwrap();
        assert_eq!(
            resolved.llm.protocol,
            tact_llm::OpenAiProtocol::ChatCompletions
        );
        assert_eq!(resolved.llm.responses_compact_threshold, None);
    }
}
