use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use tact_llm::{OpenAiProtocol, OpenAiReasoningEffort, ProviderInfo, ProviderKind};

/// Top-level TOML config (`.tact/config.toml` or `config.toml`).
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

    /// Voice-to-text input settings (independent of LLM providers).
    pub voice: VoiceTomlConfig,
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
    /// OpenAI wire protocol (`chat_completions` or `responses`).
    pub protocol: Option<String>,
    /// Optional OpenAI reasoning effort override.
    pub reasoning_effort: Option<OpenAiReasoningEffort>,
    /// Candidate models for the `/model` picker (optional).
    pub models: Vec<String>,
    /// Optional OpenAI Responses `context_management.compact_threshold`
    /// (tokens). Only meaningful for `protocol = "responses"`. When omitted,
    /// the threshold is derived from `agent.model_context_window`,
    /// `llm.max_tokens`, and 10% safety headroom.
    pub responses_compact_threshold: Option<u32>,
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
    /// Model context window in tokens (auto-compaction + TUI usage meter).
    pub model_context_window: Option<usize>,

    /// Enable desktop notifications (default: true)
    pub notifications_enabled: Option<bool>,

    /// Max entries in the system-prompt project structure snapshot.
    pub snapshot_max_items: Option<usize>,

    /// Enable micro-compaction of old tool results (default: true)
    pub micro_compact_enabled: Option<bool>,

    /// Auto-inject full skill body into system prompt (default: false)
    pub skill_body_auto_inject: Option<bool>,

    /// Extra skill root directories (optional). Each should contain `*/SKILL.md`.
    /// Relative paths are resolved against the workdir; `~` expands to `$HOME`.
    #[serde(default)]
    pub skill_dirs: Vec<String>,

    /// Project instruction files to inject into the system prompt (default: `["agents_md"]`).
    ///
    /// Supported values: `agents_md`, `claude_md` (all CLAUDE paths), `claude_md_user`,
    /// `claude_md_project`, `claude_md_subdir`.
    pub instruction_sources: Option<Vec<String>>,

    /// Subagent LLM configuration (optional).
    /// When configured, spawn_subagent uses a separate provider/model.
    pub subagent: Option<SubagentTomlConfig>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SubagentTomlConfig {
    /// References a key from [llm.providers.*] (e.g. "deepseek", "openai").
    pub provider: Option<String>,
    /// Optional model override.
    pub model: Option<String>,
    /// Optional max_tokens override.
    pub max_tokens: Option<u32>,
    /// Optional thinking_budget override.
    pub thinking_budget: Option<usize>,
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

/// Transcription provider backend.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VoiceProvider {
    #[default]
    OpenAi,
    WhisperCpp,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct VoiceTomlConfig {
    pub enabled: Option<bool>,
    pub provider: Option<VoiceProvider>,
    pub api_key: Option<String>,
    pub base_url: Option<String>,
    pub model: Option<String>,
    pub language: Option<String>,
    pub max_duration_secs: Option<u64>,
    /// Keyboard shortcut to start/stop voice recording, e.g. "ctrl+g".
    /// Format: `ctrl+<lowercase_char>` (e.g. "ctrl+g", "ctrl+r", "ctrl+,").
    /// When unset, voice recording is mouse-only via the title-bar button.
    pub voice_keybind: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ToolsTomlConfig {
    /// Bash wall-clock timeout in seconds. Zero disables the timeout.
    pub bash_timeout_secs: Option<u64>,
    /// Nice increment (0–19) applied to the bash sub-process group so TUI stays
    /// responsive during heavy commands (e.g. `cargo test`). 0 disables.
    pub bash_nice: Option<i32>,
}

// ---------------------------------------------------------------------------
// Resolved runtime settings
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct LlmSettings {
    pub provider: ProviderKind,
    pub protocol: OpenAiProtocol,
    pub reasoning_effort: Option<OpenAiReasoningEffort>,
    pub api_key: String,
    pub base_url: String,
    pub model: String,
    /// Candidate models for the `/model` TUI picker.
    pub models: Vec<String>,
    /// Resolved OpenAI Responses `context_management.compact_threshold`
    /// (tokens). `Some` only for `protocol = "responses"`: either the
    /// configured `responses_compact_threshold` (validated against
    /// `max_tokens` + 10% headroom) or derived from
    /// `model_context_window`, `max_tokens`, and headroom. `None` for
    /// non-Responses providers and when the model context window is zero.
    pub responses_compact_threshold: Option<u32>,
}

impl LlmSettings {
    pub fn provider_info(&self) -> ProviderInfo {
        ProviderInfo {
            api_key: self.api_key.clone(),
            base_url: self.base_url.clone(),
            model: self.model.clone(),
            provider: self.provider,
            protocol: self.protocol,
            reasoning_effort: self.reasoning_effort,
            responses_compact_threshold: self.responses_compact_threshold,
        }
    }
}

#[derive(Debug, Clone)]
pub struct AgentSettings {
    pub max_tokens: u32,
    pub thinking_budget: usize,
    pub model_context_window: usize,
    pub notifications_enabled: bool,
    pub snapshot_max_items: usize,
    pub micro_compact_enabled: bool,
    pub skill_body_auto_inject: bool,
    /// Extra skill roots from `[agent].skill_dirs` (unresolved path strings).
    pub skill_dirs: Vec<String>,
    pub instruction_sources: crate::config::InstructionSources,
    /// Optional subagent provider/model configuration.
    pub subagent: Option<SubagentSettings>,
}

#[derive(Debug, Clone)]
pub struct SubagentSettings {
    /// The resolved provider configuration for subagents.
    pub provider: ProviderInfo,
    /// Max output tokens.
    pub max_tokens: u32,
    /// Thinking budget (0 = off).
    pub thinking_budget: usize,
    /// Candidate model ids for the /model-subagent picker.
    pub models: Vec<String>,
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
    pub bash_timeout_secs: u64,
    pub bash_nice: i32,
}

impl ToolSettings {
    pub const DEFAULT_BASH_TIMEOUT_SECS: u64 = 1_800;
    pub const DEFAULT_BASH_NICE: i32 = 10;
}

#[derive(Debug, Clone)]
pub struct VoiceSettings {
    pub enabled: bool,
    pub provider: VoiceProvider,
    pub api_key: Option<String>,
    pub base_url: String,
    pub model: String,
    pub language: Option<String>,
    pub max_duration_secs: u64,
    /// Keyboard shortcut to start/stop voice recording, e.g. "ctrl+g".
    pub voice_keybind: Option<String>,
}

impl VoiceSettings {
    pub const DEFAULT_BASE_URL: &'static str = "https://api.openai.com/v1";
    pub const DEFAULT_WHISPER_CPP_BASE_URL: &'static str = "http://127.0.0.1:8080";
    pub const DEFAULT_MODEL: &'static str = "gpt-4o-mini-transcribe";
    pub const DEFAULT_LANGUAGE: &'static str = "zh";
    pub const DEFAULT_MAX_DURATION_SECS: u64 = 300;

    /// Disabled voice settings used when voice is not configured or invalid.
    pub fn disabled_defaults() -> Self {
        Self {
            enabled: false,
            provider: VoiceProvider::default(),
            api_key: None,
            base_url: Self::DEFAULT_BASE_URL.to_string(),
            model: Self::DEFAULT_MODEL.to_string(),
            language: Some(Self::DEFAULT_LANGUAGE.to_string()),
            max_duration_secs: Self::DEFAULT_MAX_DURATION_SECS,
            voice_keybind: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ResolvedConfig {
    pub llm: LlmSettings,
    pub agent: AgentSettings,
    pub ui: UiSettings,
    pub tools: ToolSettings,
    pub voice: VoiceSettings,
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
model_context_window = 500000
snapshot_max_items = 120
micro_compact_enabled = false

[ui]
theme = "nord"
vision_image.compress = false
vision_image.max_edge = 1024
vision_image.jpeg_quality = 75
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
        assert_eq!(cfg.agent.model_context_window, Some(500000));
        assert_eq!(cfg.agent.snapshot_max_items, Some(120));
        assert_eq!(cfg.agent.micro_compact_enabled, Some(false));
        assert_eq!(cfg.ui.theme.as_deref(), Some("nord"));
        assert_eq!(cfg.ui.vision_image.compress, Some(false));
        assert_eq!(cfg.ui.vision_image.max_edge, Some(1024));
        assert_eq!(cfg.ui.vision_image.jpeg_quality, Some(75));
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

    #[test]
    fn parse_responses_compact_threshold() {
        let toml_str = r#"
[llm]
provider = "openai"

[llm.providers.openai]
api_key = "sk-test"
model = "gpt-5"
protocol = "responses"
responses_compact_threshold = 160000
"#;
        let cfg: TactTomlConfig = toml::from_str(toml_str).unwrap();
        let openai = cfg.llm.providers.get("openai").unwrap();
        assert_eq!(openai.responses_compact_threshold, Some(160_000));

        // Absent key resolves to None, not a deserialization error.
        let absent: TactTomlConfig = toml::from_str(
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
        let openai = absent.llm.providers.get("openai").unwrap();
        assert_eq!(openai.responses_compact_threshold, None);
    }

    #[test]
    fn parse_voice_config_and_defaults() {
        let cfg: TactTomlConfig = toml::from_str(
            r#"
[llm]
provider = "openai"
[llm.providers.openai]
api_key = "sk-test"
model = "gpt-4o"
[voice]
enabled = true
api_key = "voice-test"
base_url = "http://localhost:1234/v1"
model = "gpt-4o-mini-transcribe"
language = "zh"
max_duration_secs = 45
"#,
        )
        .unwrap();
        assert_eq!(cfg.voice.enabled, Some(true));
        assert_eq!(cfg.voice.max_duration_secs, Some(45));
    }

    #[test]
    fn provider_info_carries_responses_compact_threshold() {
        let settings = LlmSettings {
            provider: ProviderKind::OpenAi,
            protocol: tact_llm::OpenAiProtocol::Responses,
            reasoning_effort: None,
            api_key: "sk-test".to_string(),
            base_url: "https://api.openai.com/v1".to_string(),
            model: "gpt-5".to_string(),
            models: Vec::new(),
            responses_compact_threshold: Some(160_000),
        };

        let info = settings.provider_info();
        assert_eq!(info.responses_compact_threshold, Some(160_000));

        let settings_without = LlmSettings {
            responses_compact_threshold: None,
            ..settings
        };
        assert_eq!(
            settings_without.provider_info().responses_compact_threshold,
            None
        );
    }
}
