use super::cli::CliArgs;
use super::types::{
    AgentSettings, LlmSettings, ResolvedConfig, TactTomlConfig, ToolSettings, UiSettings,
    VisionImageSettings,
};
use tact_llm::ProviderKind;

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

    let model = args
        .model
        .clone()
        .or_else(|| entry.model.clone())
        .filter(|m| !m.trim().is_empty())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "model not configured for provider '{provider}'. Set llm.providers.{provider}.model or pass --model"
            )
        })?;

    Ok(LlmSettings {
        provider,
        api_key,
        base_url,
        model,
        models: entry.models.clone(),
    })
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
        toml_cfg.agent.micro_compact_enabled.unwrap_or(true)
    };

    let skill_body_auto_inject = args.skill_body_auto_inject
        || toml_cfg.agent.skill_body_auto_inject.unwrap_or(false);

    let theme = args
        .theme
        .clone()
        .or_else(|| toml_cfg.ui.theme.clone())
        .unwrap_or_else(|| "retro".to_string());

    let vision_image = resolve_vision_image(toml_cfg);

    let brave_search_api_key = args
        .brave_search_api_key
        .clone()
        .or_else(|| toml_cfg.tools.brave_search_api_key.clone())
        .filter(|k| !k.is_empty());

    let permission_mode = args
        .permission_mode
        .clone()
        .or_else(|| toml_cfg.permission.mode.clone());

    ResolvedConfig {
        llm: LlmSettings {
            provider: ProviderKind::OpenAi,
            api_key: String::new(),
            base_url: String::new(),
            model: String::new(),
            models: Vec::new(),
        },
        agent: AgentSettings {
            max_tokens: 8_000,
            thinking_budget: 32_000,
            context_limit_chars: 500_000,
            notifications_enabled,
            snapshot_max_items,
            micro_compact_enabled,
            skill_body_auto_inject,
        },
        ui: UiSettings {
            theme,
            vision_image,
        },
        tools: ToolSettings {
            brave_search_api_key,
        },
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
        toml_cfg.agent.micro_compact_enabled.unwrap_or(true)
    };

    let skill_body_auto_inject = args.skill_body_auto_inject
        || toml_cfg.agent.skill_body_auto_inject.unwrap_or(false);

    let theme = args
        .theme
        .clone()
        .or_else(|| toml_cfg.ui.theme.clone())
        .unwrap_or_else(|| "retro".to_string());

    let vision_image = resolve_vision_image(toml_cfg);

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
            skill_body_auto_inject,
        },
        ui: UiSettings {
            theme,
            vision_image,
        },
        tools: ToolSettings {
            brave_search_api_key,
        },
        permission_mode,
        tokio_console: args.tokio_console,
        config_path,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::cli::CliArgs;
    use crate::config::types::TactTomlConfig;

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
            context_limit_chars: None,
            theme: None,
            snapshot_max_items: None,
            no_micro_compact: false,
            no_notifications: false,
            brave_search_api_key: None,
            tokio_console: false,
            skill_body_auto_inject: false,
        }
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
        assert_eq!(resolved.llm.api_key, "sk-test");
        assert_eq!(resolved.llm.base_url, "https://api.openai.com/v1");
        assert_eq!(resolved.agent.max_tokens, 8000);
        assert_eq!(resolved.ui.theme, "retro");
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
        assert!(resolved.agent.micro_compact_enabled);
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
}
