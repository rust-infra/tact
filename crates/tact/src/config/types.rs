use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use tact_llm::{ProviderInfo, ProviderKind};

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
    /// Active provider (`anthropic` | `openai` | `deepseek` | `kimi`).
    pub provider: Option<String>,

    /// Global default max tokens (overridable per provider entry).
    pub max_tokens: Option<u32>,

    /// Global default thinking budget (overridable per provider entry).
    pub thinking_budget: Option<usize>,

    /// Per-provider credentials and optional overrides.
    pub providers: HashMap<String, ProviderEntryToml>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ProviderEntryToml {
    pub api_key: Option<String>,
    pub model: Option<String>,
    pub base_url: Option<String>,
    pub max_tokens: Option<u32>,
    pub thinking_budget: Option<usize>,
    /// Candidate models for the `/model` picker (optional).
    pub models: Vec<String>,
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

    /// Auto-inject full skill body into system prompt (default: false)
    pub skill_body_auto_inject: Option<bool>,

    /// Project instruction files to inject into the system prompt (default: `["agents_md"]`).
    ///
    /// Supported values: `agents_md`, `claude_md` (all CLAUDE paths), `claude_md_user`,
    /// `claude_md_project`, `claude_md_subdir`.
    pub instruction_sources: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct UiTomlConfig {
    /// Initial TUI theme name (e.g. "retro", "nord", "dark").
    pub theme: Option<String>,

    /// Vision image attachment compression (user `@file` / markdown images).
    pub vision_image: VisionImageTomlConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct VisionImageTomlConfig {
    /// Downscale and JPEG re-encode user-attached images (default: true).
    pub compress: Option<bool>,

    /// Longest edge in pixels before downscaling (default: 1280).
    pub max_edge: Option<u32>,

    /// JPEG quality 1–100 for re-encoded attachments (default: 80).
    pub jpeg_quality: Option<u8>,
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
    pub provider: ProviderKind,
    pub api_key: String,
    pub base_url: String,
    pub model: String,
    /// Candidate models for the `/model` TUI picker.
    pub models: Vec<String>,
}

impl LlmSettings {
    pub fn provider_info(&self) -> ProviderInfo {
        ProviderInfo {
            api_key: self.api_key.clone(),
            base_url: self.base_url.clone(),
            model: self.model.clone(),
            provider: self.provider,
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
    pub skill_body_auto_inject: bool,
    pub instruction_sources: crate::config::InstructionSources,
}

#[derive(Debug, Clone)]
pub struct VisionImageSettings {
    pub compress: bool,
    pub max_edge: u32,
    pub jpeg_quality: u8,
}

impl VisionImageSettings {
    pub const DEFAULT_COMPRESS: bool = true;
    pub const DEFAULT_MAX_EDGE: u32 = 1280;
    pub const DEFAULT_JPEG_QUALITY: u8 = 80;
}

#[derive(Debug, Clone)]
pub struct UiSettings {
    pub theme: String,
    pub vision_image: VisionImageSettings,
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
    /// Path of the TOML file loaded at startup (for optional `/model` persist).
    pub config_path: Option<std::path::PathBuf>,
}
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
        assert!(cfg.llm.providers.is_empty());
        assert_eq!(cfg.permission.mode.as_deref(), Some("default"));
    }

    #[test]
    fn parse_full_config() {
        let toml_str = r#"
[llm]
provider = "openai"
max_tokens = 16000
thinking_budget = 64000

[llm.providers.openai]
model = "gpt-4o"
api_key = "sk-test"
base_url = "https://proxy.example.com/v1"

[permission]
mode = "auto"

[agent]
context_limit_chars = 500000
snapshot_max_items = 120
micro_compact_enabled = false

[ui]
theme = "nord"
vision_image.compress = false
vision_image.max_edge = 1024
vision_image.jpeg_quality = 75

[tools]
brave_search_api_key = "bsk-test"
"#;
        let cfg: TactTomlConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.llm.provider.as_deref(), Some("openai"));
        assert_eq!(cfg.llm.max_tokens, Some(16000));
        assert_eq!(cfg.llm.thinking_budget, Some(64000));
        let openai = cfg.llm.providers.get("openai").unwrap();
        assert_eq!(openai.model.as_deref(), Some("gpt-4o"));
        assert_eq!(openai.api_key.as_deref(), Some("sk-test"));
        assert!(openai.base_url.is_some());
        assert!(openai.models.is_empty());
        assert_eq!(cfg.permission.mode.as_deref(), Some("auto"));
        assert_eq!(cfg.agent.context_limit_chars, Some(500000));
        assert_eq!(cfg.agent.snapshot_max_items, Some(120));
        assert_eq!(cfg.agent.micro_compact_enabled, Some(false));
        assert_eq!(cfg.ui.theme.as_deref(), Some("nord"));
        assert_eq!(cfg.ui.vision_image.compress, Some(false));
        assert_eq!(cfg.ui.vision_image.max_edge, Some(1024));
        assert_eq!(cfg.ui.vision_image.jpeg_quality, Some(75));
        assert_eq!(cfg.tools.brave_search_api_key.as_deref(), Some("bsk-test"));
    }

    #[test]
    fn parse_provider_models_list() {
        let toml_str = r#"
[llm]
provider = "kimi"

[llm.providers.kimi]
api_key = "sk-test"
model = "kimi-k2.5"
models = ["kimi-k2.5", "kimi-for-coding"]
"#;
        let cfg: TactTomlConfig = toml::from_str(toml_str).unwrap();
        let kimi = cfg.llm.providers.get("kimi").unwrap();
        assert_eq!(
            kimi.models,
            vec!["kimi-k2.5".to_string(), "kimi-for-coding".to_string()]
        );
    }
}
