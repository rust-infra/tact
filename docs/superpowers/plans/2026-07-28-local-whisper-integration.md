# Local Whisper.cpp Voice Integration — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a `WhisperCppTranscriber` that talks to a local whisper.cpp server (`POST /inference`) and a `provider` config field to select between OpenAI and whisper.cpp for voice transcription.

**Architecture:** Add a `VoiceProvider` enum (`OpenAi` / `WhisperCpp`), thread it through config types → resolve → worker spawn → transcriber selection. The new `WhisperCppTranscriber` implements the existing `Transcriber` trait and reuses `parse_transcription_response()` for the shared `{"text":"..."}` JSON format.

**Tech Stack:** Rust, reqwest (multipart), serde (toml), wiremock (tests), tokio.

**Spec:** `docs/superpowers/specs/2026-07-28-local-whisper-integration-design.md`

## Global Constraints

- Default provider is `openai` — fully backward compatible; no existing config breaks.
- `WhisperCpp` does not need `api_key` or a `model` field in the request.
- `WhisperCpp` default base URL is `http://127.0.0.1:8080`.
- All existing tests in `transcriber.rs` and `mod.rs` must continue to pass.
- Book chapters (`21_chapter_config.md` + `_zh.md`) stay structurally aligned.

---

### Task 1: Add `VoiceProvider` enum and wire into config types

**Files:**
- Modify: `crates/tact/src/config/types.rs`

**Interfaces:**
- Produces: `VoiceProvider` enum (pub, `Copy`, `Deserialize`), field `VoiceTomlConfig.provider: Option<VoiceProvider>`, field `VoiceSettings.provider: VoiceProvider`, constant `VoiceSettings::DEFAULT_WHISPER_CPP_BASE_URL: &str`

- [ ] **Step 1: Add `VoiceProvider` enum near the top of `types.rs` (before `VoiceTomlConfig`)**

Add after the existing `VoiceTomlConfig` import section, before the struct:

```rust
/// Transcription provider backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VoiceProvider {
    OpenAi,
    WhisperCpp,
}

impl Default for VoiceProvider {
    fn default() -> Self {
        Self::OpenAi
    }
}
```

- [ ] **Step 2: Add `provider` field to `VoiceTomlConfig`**

In the `VoiceTomlConfig` struct (currently around line 148), add `provider` between `enabled` and `api_key`:

```rust
pub struct VoiceTomlConfig {
    pub enabled: Option<bool>,
    pub provider: Option<VoiceProvider>,
    pub api_key: Option<String>,
    pub base_url: Option<String>,
    pub model: Option<String>,
    pub language: Option<String>,
    pub max_duration_secs: Option<u64>,
}
```

- [ ] **Step 3: Add `DEFAULT_WHISPER_CPP_BASE_URL` constant to `VoiceSettings` impl block**

After the existing constants in the `impl VoiceSettings` block (around line 264), add:

```rust
pub const DEFAULT_WHISPER_CPP_BASE_URL: &'static str = "http://127.0.0.1:8080";
```

Full set of constants (existing + new):

```rust
impl VoiceSettings {
    pub const DEFAULT_BASE_URL: &'static str = "https://api.openai.com/v1";
    pub const DEFAULT_WHISPER_CPP_BASE_URL: &'static str = "http://127.0.0.1:8080";
    pub const DEFAULT_MODEL: &'static str = "gpt-4o-mini-transcribe";
    pub const DEFAULT_LANGUAGE: &'static str = "zh";
    pub const DEFAULT_MAX_DURATION_SECS: u64 = 300;
```

- [ ] **Step 4: Add `provider` field to the `VoiceSettings` struct**

In the `VoiceSettings` struct (around line 255), add `provider` before `api_key`:

```rust
pub struct VoiceSettings {
    pub enabled: bool,
    pub provider: VoiceProvider,
    pub api_key: Option<String>,
    pub base_url: String,
    pub model: String,
    pub language: Option<String>,
    pub max_duration_secs: u64,
}
```

- [ ] **Step 5: Update `disabled_defaults()` to include `provider`**

```rust
pub fn disabled_defaults() -> Self {
    Self {
        enabled: false,
        provider: VoiceProvider::default(),
        api_key: None,
        base_url: Self::DEFAULT_BASE_URL.to_string(),
        model: Self::DEFAULT_MODEL.to_string(),
        language: Some(Self::DEFAULT_LANGUAGE.to_string()),
        max_duration_secs: Self::DEFAULT_MAX_DURATION_SECS,
    }
}
```

- [ ] **Step 6: Verify compilation**

Run: `cargo check -p tact 2>&1`
Expected: should fail with missing `provider` in `resolve.rs` and elsewhere — expected at this stage.

- [ ] **Step 7: Commit**

```bash
git add crates/tact/src/config/types.rs
git commit -m "feat(voice): add VoiceProvider enum and provider field to config types"
```

---

### Task 2: Resolve provider in config with conditional defaults

**Files:**
- Modify: `crates/tact/src/config/resolve.rs`
- Modify: `crates/tact/src/config/mod.rs`

**Interfaces:**
- Consumes: `VoiceProvider` from Task 1, `VoiceTomlConfig.provider`
- Produces: `VoiceSettings.provider` resolved with correct per-provider defaults

- [ ] **Step 1: Resolve the `provider` field in `resolve_voice()`**

In `resolve_voice()` in `resolve.rs` (currently around line 37), add provider resolution after `enabled`:

```rust
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
    Ok(VoiceSettings {
        enabled,
        provider,
        api_key,
        base_url,
        model,
        language,
        max_duration_secs,
    })
}
```

- [ ] **Step 2: Update imports in `resolve.rs`**

In the `use` block at the top of `resolve.rs`, add `VoiceProvider` to the import from `types`:

```rust
use super::{
    cli::CliArgs,
    instruction_sources::InstructionSources,
    types::{
        AgentSettings, LlmSettings, ResolvedConfig, SubagentSettings, TactTomlConfig, ToolSettings,
        UiSettings, VisionImageSettings, VoiceProvider, VoiceSettings,
    },
};
```

- [ ] **Step 3: Update the `VoiceSettings` import/export in `config/mod.rs`**

This may be needed if `VoiceProvider` isn't already re-exported. Add `VoiceProvider` to the `pub use` block:

```rust
pub use types::{
    AgentSettings, AgentTomlConfig, LlmSettings, LlmTomlConfig, PermissionTomlConfig,
    ResolvedConfig, SubagentSettings, SubagentTomlConfig, TactTomlConfig, ToolSettings,
    ToolsTomlConfig, UiSettings, UiTomlConfig, VisionImageSettings, VisionImageTomlConfig,
    VoiceProvider, VoiceSettings, VoiceTomlConfig,
};
```

- [ ] **Step 4: Verify compilation**

Run: `cargo check -p tact 2>&1`
Expected: should fail in `voice/mod.rs` because `spawn_worker` and tests use `VoiceSettings` without `provider` — expected.

- [ ] **Step 5: Commit**

```bash
git add crates/tact/src/config/resolve.rs crates/tact/src/config/mod.rs
git commit -m "feat(voice): resolve provider with per-provider defaults in config"
```

---

### Task 3: Implement `WhisperCppTranscriber`

**Files:**
- Modify: `crates/tact/src/voice/transcriber.rs`
- Modify: `crates/tact/src/voice/mod.rs` (re-exports only)

**Interfaces:**
- Consumes: `Transcriber` trait, `VoiceSettings`, `parse_transcription_response`
- Produces: `pub struct WhisperCppTranscriber` implementing `Transcriber`

- [ ] **Step 1: Add `WhisperCppTranscriber` struct and implementation**

Add after the end of `impl Transcriber for OpenAiTranscriber` block and before `pub fn parse_transcription_response`:

```rust
pub struct WhisperCppTranscriber {
    settings: VoiceSettings,
    client: reqwest::Client,
}

impl WhisperCppTranscriber {
    pub fn new(settings: VoiceSettings) -> Self {
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(120))
            .build()
            .expect("reqwest client");
        Self { settings, client }
    }

    fn endpoint(&self) -> String {
        format!(
            "{}/inference",
            self.settings.base_url.trim_end_matches('/')
        )
    }
}

#[async_trait]
impl Transcriber for WhisperCppTranscriber {
    async fn transcribe(&self, wav: Vec<u8>, cancel: CancellationToken) -> anyhow::Result<String> {
        let file_part = Part::bytes(wav)
            .file_name("recording.wav".to_string())
            .mime_str("audio/wav")
            .context("failed to build multipart file part")?;
        let mut form = Form::new()
            .part("file", file_part)
            .text("response_format", "json");
        if let Some(lang) = self
            .settings
            .language
            .as_ref()
            .filter(|l| !l.trim().is_empty())
        {
            form = form.text("language", lang.clone());
        }

        let request = self.client.post(self.endpoint()).multipart(form);

        let response = tokio::select! {
            result = request.send() => result.context("transcription request failed")?,
            () = cancel.cancelled() => bail!("transcription cancelled"),
        };

        let status = response.status();
        let body = response
            .bytes()
            .await
            .context("failed to read transcription response body")?;
        parse_transcription_response(status, &body)
    }
}
```

- [ ] **Step 2: Add wiremock tests for `WhisperCppTranscriber`**

Add inside the existing `#[cfg(test)] mod tests` block at the end of `transcriber.rs`, after the existing `transcriber_missing_api_key_errors_before_request` test:

```rust
    fn whisper_settings(base_url: &str) -> VoiceSettings {
        VoiceSettings {
            enabled: true,
            provider: VoiceProvider::WhisperCpp,
            api_key: None,
            base_url: base_url.to_string(),
            model: String::new(),
            language: Some("zh".to_string()),
            max_duration_secs: 300,
        }
    }

    #[tokio::test]
    async fn whisper_transcriber_sends_expected_multipart_and_parses_text() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/inference"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"text":"你好 Tact"})))
            .mount(&server)
            .await;
        let settings = whisper_settings(&server.uri());
        let text = WhisperCppTranscriber::new(settings)
            .transcribe(vec![1, 2, 3], CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(text, "你好 Tact");
    }

    #[tokio::test]
    async fn whisper_transcriber_rejects_http_error_without_leaking_data() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(500).set_body_string("internal error"))
            .mount(&server)
            .await;
        let err = WhisperCppTranscriber::new(whisper_settings(&server.uri()))
            .transcribe(vec![1, 2, 3], CancellationToken::new())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("500"));
    }

    #[tokio::test]
    async fn whisper_transcriber_cancellation_aborts_request() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(30)))
            .mount(&server)
            .await;
        let cancel = CancellationToken::new();
        let task = tokio::spawn({
            let settings = whisper_settings(&server.uri());
            let cancel = cancel.clone();
            async move {
                WhisperCppTranscriber::new(settings)
                    .transcribe(vec![1, 2, 3], cancel)
                    .await
            }
        });
        cancel.cancel();
        let err = task.await.unwrap().unwrap_err();
        assert!(err.to_string().to_ascii_lowercase().contains("cancel"));
    }

    #[tokio::test]
    async fn whisper_transcriber_omits_language_when_none() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/inference"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"text":"ok"})))
            .mount(&server)
            .await;
        let mut settings = whisper_settings(&server.uri());
        settings.language = None;
        let text = WhisperCppTranscriber::new(settings)
            .transcribe(vec![1, 2, 3], CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(text, "ok");
    }

    #[tokio::test]
    async fn whisper_transcriber_does_not_send_model_or_auth() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/inference"))
            .and(|req: &wiremock::Request| {
                // Verify no authorization header is present.
                !req.headers.contains_key("authorization")
            })
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"text":"no-auth"})))
            .mount(&server)
            .await;
        let text = WhisperCppTranscriber::new(whisper_settings(&server.uri()))
            .transcribe(vec![1, 2, 3], CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(text, "no-auth");
    }
```

Note: the `whisper_settings` helper needs access to `VoiceProvider`. Since the test module already uses `use super::*;`, and `VoiceProvider` is in the `config` crate, we need to either import it or define it differently. The test module should add:

```rust
use crate::config::VoiceProvider;
```

But wait — the test module already has access via `use super::*` within the voice crate. Since `VoiceProvider` is defined in `crate::config::types`, it needs explicit import. Add near the top of the test module:

Actually, let me check the existing imports at the test module level.

- [ ] **Step 3: Add required import at the top of the test module**

In the `#[cfg(test)] mod tests {` block at the top (around line 108 in the current file), add:

```rust
    use super::*;
    use crate::config::VoiceProvider;
    use std::time::Duration;
    // ... existing imports
```

The existing test module already has `use std::time::Duration;` and `use wiremock::...`. We just need to add `use crate::config::VoiceProvider;`.

- [ ] **Step 4: Re-export `WhisperCppTranscriber` from `voice/mod.rs`**

In `crates/tact/src/voice/mod.rs`, update the `pub use transcriber::...` line:

```rust
pub use transcriber::{OpenAiTranscriber, Transcriber, WhisperCppTranscriber};
```

- [ ] **Step 5: Verify compilation of the transcriber module**

Run: `cargo check -p tact 2>&1`
Expected: should fail only in `voice/mod.rs` for `spawn_worker` missing `provider` — the transcriber tests should compile.

- [ ] **Step 6: Run transcriber tests**

Run: `cargo test -p tact --lib voice::transcriber -- --test-threads=1 2>&1`
Expected: ALL tests (old + new) must pass.

- [ ] **Step 7: Commit**

```bash
git add crates/tact/src/voice/transcriber.rs crates/tact/src/voice/mod.rs
git commit -m "feat(voice): add WhisperCppTranscriber with wiremock tests"
```

---

### Task 4: Select transcriber based on provider in worker, fix all construction sites

**Files:**
- Modify: `crates/tact/src/voice/mod.rs`

**Interfaces:**
- Consumes: `VoiceSettings.provider`, `OpenAiTranscriber`, `WhisperCppTranscriber`
- Produces: working `spawn_worker()` that picks correct transcriber

- [ ] **Step 1: Update `spawn_worker()` to select transcriber by provider**

Replace the current `spawn_worker` function:

```rust
pub fn spawn_worker(settings: VoiceSettings) -> VoiceWorkerHandle {
    spawn_worker_with_components(
        settings.clone(),
        Arc::new(recorder::CpalRecorder),
        Arc::new(OpenAiTranscriber::new(settings)),
    )
}
```

With:

```rust
pub fn spawn_worker(settings: VoiceSettings) -> VoiceWorkerHandle {
    let transcriber: Arc<dyn Transcriber> = match settings.provider {
        crate::config::VoiceProvider::OpenAi => Arc::new(OpenAiTranscriber::new(settings.clone())),
        crate::config::VoiceProvider::WhisperCpp => Arc::new(WhisperCppTranscriber::new(settings.clone())),
    };
    spawn_worker_with_components(
        settings.clone(),
        Arc::new(recorder::CpalRecorder),
        transcriber,
    )
}
```

- [ ] **Step 2: Update test settings helpers in the mod.rs test module**

In the `#[cfg(test)] mod tests` block in `voice/mod.rs`, update the `test_settings()` helper and every test that constructs `VoiceSettings` to include `provider: VoiceProvider::OpenAi`. The test module already has `use crate::config::VoiceSettings;` — add `VoiceProvider` too:

```rust
use crate::config::VoiceSettings;
use crate::config::VoiceProvider;
```

Then update `test_settings()`:

```rust
fn test_settings() -> VoiceSettings {
    VoiceSettings {
        enabled: true,
        provider: VoiceProvider::OpenAi,
        api_key: Some("voice-test".into()),
        base_url: "http://localhost".into(),
        model: "gpt-4o-mini-transcribe".into(),
        language: Some("zh".into()),
        max_duration_secs: 300,
    }
}
```

And update the inline `VoiceSettings { api_key: None, ..test_settings() }` construction in the `missing_api_key_is_reported_by_transcriber_without_recording_failure` test — the `..test_settings()` spread will already pick up `provider` from `test_settings()`, so no change needed there.

But double-check the explicit construction at ~line 431:

```rust
let settings = VoiceSettings {
    api_key: None,
    ..test_settings()
};
```

This will work because `test_settings()` now includes `provider`. No change needed.

- [ ] **Step 3: Fix the `spawn_worker_with_components` signature to not need VoiceSettings**

Actually, `spawn_worker_with_components` already takes `settings: VoiceSettings` separately. The `spawn_worker` change above passes it. The tests also call `spawn_worker_with_components` directly with `test_settings()`. So everything should work.

- [ ] **Step 4: Verify compilation**

Run: `cargo check -p tact 2>&1`
Expected: should compile clean.

- [ ] **Step 5: Run all voice tests**

Run: `cargo test -p tact --lib voice:: -- --test-threads=1 2>&1`
Expected: ALL tests pass (existing + new).

- [ ] **Step 6: Run broader tests to catch any other `VoiceSettings` construction sites**

Run: `cargo test -p tact --lib -- --test-threads=1 2>&1`
Expected: ALL pass. This catches the test in `types.rs` (`parse_voice_config_and_defaults`) and any other site that constructs `VoiceSettings`.

- [ ] **Step 7: Fix any remaining compilation issues in other crates**

Run: `cargo check 2>&1`
Expected: all crates compile. Look for any `VoiceSettings { .. }` construction in `tact-ui` or `tui` crates that need `provider`.

The known construction sites are:
- `crates/tui/src/lib.rs:119` — `pub voice: tact::config::VoiceSettings`
- `crates/tui/src/handlers/select.rs:607` — `voice: tact::config::VoiceSettings::disabled_defaults()`
- `crates/tact-ui/tests/recovery_compaction.rs:53` — `voice: tact::config::VoiceSettings::disabled_defaults()`
- `crates/tact-ui/src/test_support.rs:61` — `voice: tact::config::VoiceSettings::disabled_defaults()`
- `crates/tact/src/agent/mod.rs:1463` — `voice: crate::config::VoiceSettings::disabled_defaults()`

All of these use `disabled_defaults()` which already includes `provider`. No changes needed.

- [ ] **Step 8: Commit**

```bash
git add crates/tact/src/voice/mod.rs
git commit -m "feat(voice): select transcriber by VoiceProvider in spawn_worker"
```

---

### Task 5: Update documentation

**Files:**
- Modify: `config.example.toml`
- Modify: `book/21_chapter_config.md`
- Modify: `book/21_chapter_config_zh.md`

- [ ] **Step 1: Update `config.example.toml`**

Replace the `[voice]` section:

```toml
[voice]
# Show the title-bar voice button and enable microphone capture (default: false).
enabled = false
# Transcription provider: "openai" (default) or "whisper_cpp" for a local whisper.cpp server.
# provider = "openai"
# API key for the transcription service — not shared with [llm.providers.*].
# Required for openai; ignored for whisper_cpp.
api_key = ""
# Transcription endpoint base URL.
# openai default: https://api.openai.com/v1
# whisper_cpp default: http://127.0.0.1:8080
# base_url = "https://api.openai.com/v1"
# Transcription model (default: gpt-4o-mini-transcribe; ignored for whisper_cpp).
# model = "gpt-4o-mini-transcribe"
# BCP-47 language hint sent to the API (default: zh).
# language = "zh"
# Maximum recording duration in seconds, 1–600 (default: 300).
# max_duration_secs = 300
```

- [ ] **Step 2: Update `book/21_chapter_config.md` — config example block**

Replace the `[voice]` section in the example config block (around line 144):

```markdown
[voice]
# Independent from [llm.providers.*]; disabled by default.
# enabled = false
# provider = "openai"
# api_key = ""
# base_url = "https://api.openai.com/v1"
# model = "gpt-4o-mini-transcribe"
# language = "zh"
# max_duration_secs = 300
```

- [ ] **Step 3: Update `book/21_chapter_config.md` — defaults table**

Add `voice.provider` row after `voice.enabled` in the table (around line 199):

```markdown
| `voice.provider` | `openai` | `openai` / `whisper_cpp` |
```

Full updated rows:

```markdown
| `voice.enabled` | `false` | — |
| `voice.provider` | `openai` | `openai` / `whisper_cpp` |
| `voice.base_url` | `https://api.openai.com/v1` (openai) / `http://127.0.0.1:8080` (whisper_cpp) | — |
| `voice.model` | `gpt-4o-mini-transcribe` (openai) / empty (whisper_cpp) | — |
| `voice.language` | `zh` | — |
| `voice.max_duration_secs` | `300` (valid `1..=600`) | — |
```

- [ ] **Step 4: Update `book/21_chapter_config.md` — `[voice]` section description**

Replace the `[voice]` description section (around line 205):

```markdown
### `[voice]` — speech-to-text input (macOS-first)

Independent API key and endpoint from `[llm.providers.*]`. `provider = "openai"` (default) sends
audio to `{base_url}/audio/transcriptions` and requires `api_key`. `provider = "whisper_cpp"`
sends to `{base_url}/inference` with no auth and no `model` field, for use with a local
[whisper.cpp](https://github.com/ggerganov/whisper.cpp) server. Transcripts are inserted into the
input box for review (never auto-submitted). `enabled = false` hides the title-bar button.
`enabled = true` without `api_key` (openai only) still shows the button and reports
`[voice].api_key` on click. Credentials are not logged or written to session history.
```

- [ ] **Step 5: Update `book/21_chapter_config_zh.md` — config example block**

Replace the `[voice]` section in the example config block (around line 142):

```markdown
[voice]
# 独立于 [llm.providers.*]；默认关闭。
# enabled = false
# provider = "openai"
# api_key = ""
# base_url = "https://api.openai.com/v1"
# model = "gpt-4o-mini-transcribe"
# language = "zh"
# max_duration_secs = 300
```

- [ ] **Step 6: Update `book/21_chapter_config_zh.md` — defaults table**

Add `voice.provider` row after `voice.enabled` in the table (around line 186):

```markdown
| `voice.provider` | `openai` | `openai` / `whisper_cpp` |
```

Full updated rows:

```markdown
| `voice.enabled` | `false` | — |
| `voice.provider` | `openai` | `openai` / `whisper_cpp` |
| `voice.base_url` | `https://api.openai.com/v1`（openai）/ `http://127.0.0.1:8080`（whisper_cpp） | — |
| `voice.model` | `gpt-4o-mini-transcribe`（openai）/ 空（whisper_cpp） | — |
| `voice.language` | `zh` | — |
| `voice.max_duration_secs` | `300`（有效范围 `1..=600`） | — |
```

- [ ] **Step 7: Update `book/21_chapter_config_zh.md` — `[voice]` section description**

Replace the `[voice]` description section (around line 192):

```markdown
### `[voice]` — 语音转文字输入（macOS 优先）

API 密钥与端点独立于 `[llm.providers.*]`。`provider = "openai"`（默认）将音频发往
`{base_url}/audio/transcriptions`，需要 `api_key`。`provider = "whisper_cpp"` 将音频发往
`{base_url}/inference`，无需认证与 `model` 字段，适用于本地
[whisper.cpp](https://github.com/ggerganov/whisper.cpp) 服务器。转写结果插入输入框供审阅（不会自动提交）。
`enabled = false` 隐藏标题栏按钮。`enabled = true` 但未配置 `api_key`（仅 openai）时仍显示按钮，
点击会提示 `[voice].api_key`。凭证不会写入日志或会话历史。
```

- [ ] **Step 8: Commit**

```bash
git add config.example.toml book/21_chapter_config.md book/21_chapter_config_zh.md
git commit -m "docs(voice): document provider field for local whisper.cpp integration"
```

---

### Task 6: Final integration verification

**Files:**
- None (verification only)

- [ ] **Step 1: Full test suite**

Run: `cargo test -- --test-threads=1 2>&1`
Expected: ALL tests pass across all crates.

- [ ] **Step 2: Clippy**

Run: `cargo clippy --all-targets 2>&1`
Expected: no warnings.

- [ ] **Step 3: Commit (if any fixups needed), or verify already committed**

```bash
git status
```

Expected: clean working tree.
```

---

## Self-Review

### 1. Spec coverage
- ✅ `VoiceProvider` enum with `OpenAi` and `WhisperCpp` variants — Task 1
- ✅ `provider` field in `VoiceTomlConfig` and `VoiceSettings` — Task 1
- ✅ `disabled_defaults()` updated — Task 1
- ✅ Conditional defaults in resolve (base_url, model per provider) — Task 2
- ✅ `WhisperCppTranscriber` implementing `Transcriber` — Task 3
- ✅ Tests: success, error status, cancellation, no-auth validation, missing language — Task 3
- ✅ Worker selection in `spawn_worker` — Task 4
- ✅ All test sites updated with `provider` field — Task 4
- ✅ `config.example.toml` updated — Task 5
- ✅ Book chapters (EN + ZH) updated — Task 5
- ✅ Backward compatibility (default `openai`) confirmed — global constraint

### 2. Placeholder scan
- ✅ No TBD, TODO, or placeholder language
- ✅ All code steps contain actual code blocks
- ✅ All commands include expected output checks

### 3. Type consistency
- ✅ `VoiceProvider` defined in Task 1, consumed in Tasks 2-4 — consistent
- ✅ `DEFAULT_WHISPER_CPP_BASE_URL` constant defined in Task 1, used in Task 2
- ✅ `WhisperCppTranscriber` defined in Task 3, used in Task 4
- ✅ `parse_transcription_response` reused, not redefined
- ✅ All test helpers use `VoiceProvider::OpenAi` or `VoiceProvider::WhisperCpp` consistently
