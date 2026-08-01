use tact_llm::{OpenAiProtocol, ProviderInfo, ProviderKind};

use super::{
    cli::CliArgs,
    instruction_sources::InstructionSources,
    types::{
        AgentSettings, LlmSettings, ResolvedConfig, SubagentSettings, TactTomlConfig, ToolSettings,
        UiSettings, VisionImageSettings, VoiceProvider, VoiceSettings,
    },
};

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
        });
    let model = toml_cfg
        .voice
        .model
        .clone()
        .filter(|m| !m.trim().is_empty())
        .unwrap_or_else(|| match provider {
            VoiceProvider::WhisperCpp => String::new(),
            VoiceProvider::OpenAi => VoiceSettings::DEFAULT_MODEL.to_string(),
        });
    let language = match toml_cfg.voice.language.clone() {
        Some(language) if language.trim().is_empty() => None,
        Some(language) => Some(language),
        None => Some(VoiceSettings::DEFAULT_LANGUAGE.to_string()),
    };
    let max_duration_secs = toml_cfg
        .voice
        .max_duration_secs
        .unwrap_or(VoiceSettings::DEFAULT_MAX_DURATION_SECS);
    if !(1..=600).contains(&max_duration_secs) {
        anyhow::bail!(
            "voice.max_duration_secs must be between 1 and 600 (got {max_duration_secs})"
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
                "LLM provider not configured. Set llm.provider in config.toml or pass --provider anthropic|openai|deepseek|kimi"
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
fn resolve_responses_compact_threshold(
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
                    "invalid token limits: responses_compact_threshold ({configured}) must leave room for llm.max_tokens ({max_tokens}) and 10% headroom within agent.model_context_window ({model_context_window})"
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

    for key in toml_cfg.llm.providers.keys() {
        key.parse::<ProviderKind>().map_err(anyhow::Error::msg)?;
    }

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
    if protocol == OpenAiProtocol::Responses && provider != ProviderKind::OpenAi {
        anyhow::bail!("protocol 'responses' is only supported for provider 'openai'");
    }
    let reasoning_effort = entry.reasoning_effort;
    if reasoning_effort.is_some() && provider != ProviderKind::OpenAi {
        anyhow::bail!("reasoning_effort is only supported for provider 'openai'");
    }

    Ok(LlmSettings {
        provider,
        protocol,
        reasoning_effort,
        api_key,
        base_url,
        model,
        models: entry.models.clone(),
        // Filled in by `resolve_config` once max_tokens and the model context
        // window are resolved (needs both for validation/derivation).
        responses_compact_threshold: None,
    })
}

/// Resolve the optional subagent provider configuration from TOML.
///
/// Reads `[agent.subagent]` and validates that the referenced `provider` key
/// exists in `[llm.providers.*]`. Returns `None` when the subagent section is
/// absent (backward compatibility).
///
/// `main_max_tokens` and `main_thinking_budget` are the main agent's resolved
/// defaults, used as fallback when the subagent does not override them.
fn resolve_subagent(
    toml_cfg: &TactTomlConfig,
    main_max_tokens: u32,
    main_thinking_budget: usize,
) -> anyhow::Result<Option<SubagentSettings>> {
    let Some(subagent_cfg) = &toml_cfg.agent.subagent else {
        return Ok(None);
    };
    let Some(provider_name) = &subagent_cfg.provider else {
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

    // Resolve reasoning_effort from the referenced provider entry
    let reasoning_effort = entry.reasoning_effort;

    let max_tokens = subagent_cfg
        .max_tokens
        .or(entry.max_tokens)
        .unwrap_or_else(|| {
            if provider_kind == ProviderKind::Kimi
                && (model.contains("kimi-k2")
                    || model.contains("k2.")
                    || model.contains("k2-")
                    || model == "kimi-for-coding")
            {
                32_000
            } else {
                main_max_tokens
            }
        });
    let thinking_budget = subagent_cfg
        .thinking_budget
        .or(entry.thinking_budget)
        .unwrap_or(main_thinking_budget);

    if thinking_budget > 0 && usize::try_from(max_tokens).is_ok_and(|mt| thinking_budget >= mt) {
        anyhow::bail!(
            "subagent thinking_budget ({thinking_budget}) must be less than max_tokens ({max_tokens})"
        );
    }

    Ok(Some(SubagentSettings {
        provider: ProviderInfo {
            api_key,
            base_url,
            model,
            provider: provider_kind,
            protocol,
            reasoning_effort,
            responses_compact_threshold: entry.responses_compact_threshold,
        },
        max_tokens,
        thinking_budget,
        models: entry.models.clone(),
    }))
}

pub(super) fn resolve_non_llm_settings(
    args: &CliArgs,
    toml_cfg: &TactTomlConfig,
    config_path: Option<std::path::PathBuf>,
) -> ResolvedConfig {
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

    let skill_body_auto_inject =
        args.skill_body_auto_inject || toml_cfg.agent.skill_body_auto_inject.unwrap_or(false);

    let skill_dirs = toml_cfg.agent.skill_dirs.clone();

    let instruction_sources =
        InstructionSources::from_config(toml_cfg.agent.instruction_sources.clone())
            .expect("invalid instruction_sources in config");

    let theme = args
        .theme
        .clone()
        .or_else(|| toml_cfg.ui.theme.clone())
        .unwrap_or_else(|| "ink".to_string());

    let vision_image = resolve_vision_image(toml_cfg);

    let bash_timeout_secs = toml_cfg
        .tools
        .bash_timeout_secs
        .unwrap_or(ToolSettings::DEFAULT_BASH_TIMEOUT_SECS);

    let bash_nice = toml_cfg
        .tools
        .bash_nice
        .unwrap_or(ToolSettings::DEFAULT_BASH_NICE);

    let permission_mode = args
        .permission_mode
        .clone()
        .or_else(|| toml_cfg.permission.mode.clone());

    let voice = resolve_voice(toml_cfg).unwrap_or_else(|err| {
        tracing::warn!(error = %err, "invalid voice configuration; voice input disabled");
        VoiceSettings::disabled_defaults()
    });

    ResolvedConfig {
        llm: LlmSettings {
            provider: ProviderKind::OpenAi,
            protocol: OpenAiProtocol::default(),
            reasoning_effort: None,
            api_key: String::new(),
            base_url: String::new(),
            model: String::new(),
            models: Vec::new(),
            responses_compact_threshold: None,
        },
        agent: AgentSettings {
            max_tokens: 8_000,
            thinking_budget: 0,
            model_context_window: 200_000,
            notifications_enabled,
            snapshot_max_items,
            micro_compact_enabled,
            skill_body_auto_inject,
            skill_dirs,
            instruction_sources,
            subagent: None,
        },
        ui: UiSettings {
            theme,
            vision_image,
        },
        tools: ToolSettings {
            bash_timeout_secs,
            bash_nice,
        },
        voice,
        permission_mode,
        tokio_console: args.tokio_console,
        config_path,
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

    let max_tokens = args
        .max_tokens
        .or_else(|| entry.and_then(|e| e.max_tokens))
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
        .or_else(|| entry.and_then(|e| e.thinking_budget))
        .or(toml_cfg.llm.thinking_budget)
        .unwrap_or(0);

    if thinking_budget > 0 && usize::try_from(max_tokens).is_ok_and(|mt| thinking_budget >= mt) {
        anyhow::bail!(
            "thinking_budget ({thinking_budget}) must be less than max_tokens ({max_tokens})"
        );
    }

    let model_context_window = args
        .model_context_window
        .or(toml_cfg.agent.model_context_window)
        .unwrap_or(200_000);

    if model_context_window != 0
        && !usize::try_from(max_tokens).is_ok_and(|max_tokens| max_tokens < model_context_window)
    {
        anyhow::bail!(
            "invalid token limits: llm.max_tokens ({max_tokens}) must be less than agent.model_context_window ({model_context_window})"
        );
    }

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

    let skill_body_auto_inject =
        args.skill_body_auto_inject || toml_cfg.agent.skill_body_auto_inject.unwrap_or(false);

    let skill_dirs = toml_cfg.agent.skill_dirs.clone();

    let instruction_sources =
        InstructionSources::from_config(toml_cfg.agent.instruction_sources.clone())
            .map_err(|e| anyhow::anyhow!("{e}"))?;

    let theme = args
        .theme
        .clone()
        .or_else(|| toml_cfg.ui.theme.clone())
        .unwrap_or_else(|| "ink".to_string());

    let vision_image = resolve_vision_image(toml_cfg);

    let bash_timeout_secs = toml_cfg
        .tools
        .bash_timeout_secs
        .unwrap_or(ToolSettings::DEFAULT_BASH_TIMEOUT_SECS);

    let bash_nice = toml_cfg
        .tools
        .bash_nice
        .unwrap_or(ToolSettings::DEFAULT_BASH_NICE);

    let permission_mode = args
        .permission_mode
        .clone()
        .or_else(|| toml_cfg.permission.mode.clone());

    let subagent = resolve_subagent(toml_cfg, max_tokens, thinking_budget)?;

    let responses_compact_threshold = resolve_responses_compact_threshold(
        entry.and_then(|e| e.responses_compact_threshold),
        llm.protocol,
        model_context_window,
        max_tokens,
    )?;

    let voice = resolve_voice(toml_cfg)?;

    Ok(ResolvedConfig {
        llm: LlmSettings {
            responses_compact_threshold,
            ..llm
        },
        agent: AgentSettings {
            max_tokens,
            thinking_budget,
            model_context_window,
            notifications_enabled,
            snapshot_max_items,
            micro_compact_enabled,
            skill_body_auto_inject,
            skill_dirs,
            instruction_sources,
            subagent,
        },
        ui: UiSettings {
            theme,
            vision_image,
        },
        tools: ToolSettings {
            bash_timeout_secs,
            bash_nice,
        },
        voice,
        permission_mode,
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
        });
        let cfg = resolve_config(&empty_cli_args(), &toml_cfg, None).unwrap();
        let sa = cfg.agent.subagent.unwrap();
        assert_eq!(sa.thinking_budget, 0);
        assert_eq!(sa.max_tokens, 8_000);
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

    #[test]
    fn subagent_explicit_overrides_beat_inherited_defaults() {
        let mut toml_cfg = openai_toml_config();
        toml_cfg.agent.subagent = Some(SubagentTomlConfig {
            provider: Some("openai".to_string()),
            model: Some("gpt-4o-mini".to_string()),
            max_tokens: Some(16_000),
            thinking_budget: Some(8_000),
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
        });
        let err = resolve_config(&empty_cli_args(), &toml_cfg, None).unwrap_err();
        assert!(err.to_string().contains("thinking_budget"));
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
        assert_eq!(
            resolved.agent.instruction_sources,
            InstructionSources::default()
        );
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
    fn reject_reasoning_effort_for_non_openai_provider() {
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

        let error = resolve_config(&empty_cli_args(), &toml_cfg, None)
            .unwrap_err()
            .to_string();
        assert!(error.contains("reasoning_effort is only supported for provider 'openai'"));
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
    fn reject_responses_protocol_for_non_openai_provider() {
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
        assert!(error.contains("only supported for provider 'openai'"));
    }

    #[test]
    fn resolve_instruction_sources_from_toml() {
        let toml_cfg: TactTomlConfig = toml::from_str(
            r#"
[agent]
instruction_sources = ["agents_md", "claude_md_project"]
"#,
        )
        .unwrap();
        let resolved = resolve_non_llm_settings(&empty_cli_args(), &toml_cfg, None);
        assert!(resolved.agent.instruction_sources.agents_md);
        assert!(!resolved.agent.instruction_sources.claude_user);
        assert!(resolved.agent.instruction_sources.claude_project);
        assert!(!resolved.agent.instruction_sources.claude_subdir);
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
        let resolved = resolve_non_llm_settings(&empty_cli_args(), &toml_cfg, None);
        assert_eq!(
            resolved.agent.skill_dirs,
            vec!["~/shared-skills".to_string(), "./vendor/skills".to_string()]
        );
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
        let resolved = resolve_non_llm_settings(&empty_cli_args(), &toml_cfg, None);
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
        let resolved = resolve_non_llm_settings(&empty_cli_args(), &toml_cfg, None);
        assert_eq!(resolved.ui.vision_image.max_edge, 4096);
        assert_eq!(resolved.ui.vision_image.jpeg_quality, 1);
    }

    #[test]
    fn bash_timeout_defaults_to_thirty_minutes_and_zero_is_preserved() {
        let default = resolve_non_llm_settings(&empty_cli_args(), &TactTomlConfig::default(), None);
        assert_eq!(default.tools.bash_timeout_secs, 1_800);

        let cfg: TactTomlConfig = toml::from_str("[tools]\nbash_timeout_secs = 0\n").unwrap();
        let disabled = resolve_non_llm_settings(&empty_cli_args(), &cfg, None);
        assert_eq!(disabled.tools.bash_timeout_secs, 0);
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
max_tokens = 8000

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
        assert_eq!(resolved.agent.max_tokens, 8000);
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
    fn per_provider_max_tokens_overrides_global() {
        let toml_cfg: TactTomlConfig = toml::from_str(
            r#"
[llm]
provider = "openai"
max_tokens = 8000

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
    fn cli_max_tokens_overrides_entry_and_global() {
        let toml_cfg: TactTomlConfig = toml::from_str(
            r#"
[llm]
provider = "openai"
max_tokens = 8000

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
max_tokens = 128000
thinking_budget = 32000

[llm.providers.openai]
api_key = "sk-test"
model = "gpt-4o"
thinking_budget = 64000
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
    fn invalid_provider_map_key_errors() {
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
        let err = resolve_config(&empty_cli_args(), &toml_cfg, None)
            .unwrap_err()
            .to_string();
        assert!(err.contains("unknown provider"));
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
    fn unknown_provider_name_errors() {
        let toml_cfg: TactTomlConfig = toml::from_str(
            r#"
[llm]
provider = "foo"

[llm.providers.foo]
api_key = "sk-test"
model = "gpt-4o"
"#,
        )
        .unwrap();
        let err = resolve_config(&empty_cli_args(), &toml_cfg, None)
            .unwrap_err()
            .to_string();
        assert!(err.contains("unknown provider"));
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
        let resolved = resolve_non_llm_settings(&args, &TactTomlConfig::default(), None);
        assert_eq!(resolved.ui.theme, "nord");
        assert!(resolved.llm.api_key.is_empty());
    }

    #[test]
    fn resolve_model_context_window_defaults_to_200k() {
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
model = "gpt-4o"

[agent]
model_context_window = 128000
"#,
        )
        .unwrap();
        let resolved = resolve_config(&empty_cli_args(), &toml_cfg, None).unwrap();
        assert_eq!(resolved.agent.model_context_window, 128_000);
    }

    #[test]
    fn max_tokens_equal_to_model_context_window_errors() {
        let toml_cfg: TactTomlConfig = toml::from_str(
            r#"
[llm]
provider = "openai"
max_tokens = 8000

[llm.providers.openai]
api_key = "sk-test"
model = "gpt-4o"

[agent]
model_context_window = 8000
"#,
        )
        .unwrap();

        let err = resolve_config(&empty_cli_args(), &toml_cfg, None)
            .unwrap_err()
            .to_string();
        assert_eq!(
            err,
            "invalid token limits: llm.max_tokens (8000) must be less than agent.model_context_window (8000)"
        );
    }

    #[test]
    fn resolved_cli_max_tokens_above_model_context_window_errors() {
        let toml_cfg: TactTomlConfig = toml::from_str(
            r#"
[llm]
provider = "openai"
max_tokens = 1000

[llm.providers.openai]
api_key = "sk-test"
model = "gpt-4o"

[agent]
model_context_window = 8000
"#,
        )
        .unwrap();
        let mut args = empty_cli_args();
        args.max_tokens = Some(9000);

        let err = resolve_config(&args, &toml_cfg, None)
            .unwrap_err()
            .to_string();
        assert!(err.contains("llm.max_tokens (9000)"));
        assert!(err.contains("agent.model_context_window (8000)"));
    }

    #[test]
    fn max_tokens_below_model_context_window_is_valid() {
        let toml_cfg: TactTomlConfig = toml::from_str(
            r#"
[llm]
provider = "openai"
max_tokens = 7999

[llm.providers.openai]
api_key = "sk-test"
model = "gpt-4o"

[agent]
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
max_tokens = 32000

[llm.providers.openai]
api_key = "sk-test"
model = "gpt-4o"

[agent]
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
max_tokens = 8000

[llm.providers.openai]
api_key = "sk-test"
model = "gpt-5"
protocol = "responses"

[agent]
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
max_tokens = 8000

[llm.providers.openai]
api_key = "sk-test"
model = "gpt-5"
protocol = "responses"
responses_compact_threshold = 160000

[agent]
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
max_tokens = 8000

[llm.providers.openai]
api_key = "sk-test"
model = "gpt-5"
protocol = "responses"
responses_compact_threshold = 0

[agent]
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
max_tokens = 8000

[llm.providers.openai]
api_key = "sk-test"
model = "gpt-5"
protocol = "responses"
responses_compact_threshold = 180000

[agent]
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
    fn responses_threshold_is_none_when_window_is_zero() {
        let toml_cfg: TactTomlConfig = toml::from_str(
            r#"
[llm]
provider = "openai"
max_tokens = 8000

[llm.providers.openai]
api_key = "sk-test"
model = "gpt-5"
protocol = "responses"

[agent]
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
max_tokens = 8000

[llm.providers.openai]
api_key = "sk-test"
model = "gpt-4o"

[agent]
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
