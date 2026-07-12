use super::cli::CliArgs;
use super::types::{
    AgentSettings, LlmSettings, ResolvedConfig, TactTomlConfig, ToolSettings, UiSettings,
    VisionImageSettings,
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

fn default_base_url(provider: &str) -> Option<String> {
    match provider {
        "openai" => Some("https://api.openai.com/v1".to_string()),
        "deepseek" => Some("https://api.deepseek.com".to_string()),
        "kimi" => Some("https://api.moonshot.cn/v1".to_string()),
        _ => None,
    }
}

fn default_model(_provider: &str) -> Option<String> {
    None
}

fn resolve_provider(args: &CliArgs, toml_cfg: &TactTomlConfig) -> anyhow::Result<String> {
    if let Some(ref p) = args.provider {
        return Ok(p.clone());
    }
    if let Some(ref p) = toml_cfg.llm.provider {
        return Ok(p.clone());
    }
    anyhow::bail!(
        "LLM provider not configured. Set llm.provider in config.toml or pass --provider anthropic|openai|deepseek|kimi"
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
        .ok_or_else(|| anyhow::anyhow!("base_url not configured for provider '{provider}'"))?;

    let model = args
        .model
        .clone()
        .or_else(|| toml_cfg.llm.model.clone())
        .or_else(|| default_model(&provider))
        .filter(|m| !m.trim().is_empty())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "model not configured for provider '{provider}'. Set llm.model in config.toml or pass --model"
            )
        })?;

    Ok(LlmSettings {
        provider,
        api_key,
        base_url,
        model,
    })
}

pub(super) fn resolve_non_llm_settings(
    args: &CliArgs,
    toml_cfg: &TactTomlConfig,
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
            provider: String::new(),
            api_key: String::new(),
            base_url: String::new(),
            model: String::new(),
        },
        agent: AgentSettings {
            max_tokens: 8_000,
            thinking_budget: 32_000,
            context_limit_chars: 500_000,
            notifications_enabled,
            snapshot_max_items,
            micro_compact_enabled,
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
    }
}

pub(super) fn resolve_config(
    args: &CliArgs,
    toml_cfg: &TactTomlConfig,
) -> anyhow::Result<ResolvedConfig> {
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
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::cli::CliArgs;
    use crate::config::types::TactTomlConfig;

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
        };
        let resolved = resolve_config(&args, &toml_cfg).unwrap();
        assert_eq!(resolved.llm.provider, "openai");
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
        let args = CliArgs {
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
        };
        let resolved = resolve_non_llm_settings(&args, &toml_cfg);
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
        let args = CliArgs {
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
        };
        let resolved = resolve_non_llm_settings(&args, &toml_cfg);
        assert_eq!(resolved.ui.vision_image.max_edge, 4096);
        assert_eq!(resolved.ui.vision_image.jpeg_quality, 1);
    }

    #[test]
    fn resolve_deepseek_config_from_toml() {
        let toml_cfg: TactTomlConfig = toml::from_str(
            r#"
[llm]
provider = "deepseek"
api_key = "sk-test"
model = "deepseek-chat"
"#,
        )
        .unwrap();
        let args = CliArgs {
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
        };
        let resolved = resolve_config(&args, &toml_cfg).unwrap();
        assert_eq!(resolved.llm.provider, "deepseek");
        assert_eq!(resolved.llm.api_key, "sk-test");
        assert_eq!(resolved.llm.model, "deepseek-chat");
        assert_eq!(resolved.llm.base_url, "https://api.deepseek.com");
    }

    #[test]
    fn resolve_kimi_config_from_toml() {
        let toml_cfg: TactTomlConfig = toml::from_str(
            r#"
[llm]
provider = "kimi"
api_key = "mk-test"
model = "kimi-k2.5"
"#,
        )
        .unwrap();
        let args = CliArgs {
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
        };
        let resolved = resolve_config(&args, &toml_cfg).unwrap();
        assert_eq!(resolved.llm.provider, "kimi");
        assert_eq!(resolved.llm.api_key, "mk-test");
        assert_eq!(resolved.llm.model, "kimi-k2.5");
        assert_eq!(resolved.llm.base_url, "https://api.moonshot.cn/v1");
    }

    #[test]
    fn resolve_config_requires_model() {
        let toml_cfg: TactTomlConfig = toml::from_str(
            r#"
[llm]
provider = "openai"
api_key = "sk-test"
"#,
        )
        .unwrap();
        let args = CliArgs {
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
        };
        let err = resolve_config(&args, &toml_cfg).unwrap_err().to_string();
        assert!(err.contains("model not configured"));
    }

    #[test]
    fn list_sessions_does_not_require_llm() {
        let args = CliArgs {
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
            list_sessions: true,
            notifications: None,
            context_limit_chars: None,
            theme: Some("nord".to_string()),
            snapshot_max_items: None,
            no_micro_compact: false,
            no_notifications: false,
            brave_search_api_key: None,
            tokio_console: false,
        };
        let resolved = resolve_non_llm_settings(&args, &TactTomlConfig::default());
        assert_eq!(resolved.ui.theme, "nord");
        assert!(resolved.llm.api_key.is_empty());
    }
}
