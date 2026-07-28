# Local Whisper.cpp Server Integration for Voice Transcription

**Date:** 2026-07-28
**Status:** Approved design
**Motivation:** Allow Tact's voice feature to use a locally running whisper.cpp server for transcription instead of requiring an OpenAI API key.

## Overview

Tact's voice module currently supports only OpenAI's `/v1/audio/transcriptions` endpoint. Many users prefer running a local whisper.cpp server for privacy, offline use, or cost savings. This spec adds a `WhisperCppTranscriber` that implements the existing `Transcriber` trait and a `provider` config field to select between OpenAI and whisper.cpp.

## Provider API Differences

| Aspect | OpenAI | whisper.cpp |
|--------|--------|-------------|
| Endpoint path | `/v1/audio/transcriptions` | `/inference` |
| HTTP method | POST | POST |
| Content type | multipart/form-data | multipart/form-data |
| Auth | Bearer token (api_key) | None |
| Model field | Required (`model=...`) | Not needed (uses loaded model) |
| Extra params | `language`, `response_format` | `temperature`, `temperature_inc`, `no_speech_thold`, `response_format` |
| Response | `{"text":"..."}` | `{"text":"..."}` |
| Error format | JSON with `error` key | Plain text or JSON |

## Configuration Changes

### New `VoiceProvider` enum

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VoiceProvider {
    OpenAi,
    WhisperCpp,
}
```

Default: `OpenAi` (backward compatible).

### Updated `VoiceTomlConfig`

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

### Updated `VoiceSettings`

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

### Example config.toml

```toml
[voice]
enabled = true
provider = "whisper_cpp"   # "openai" (default) or "whisper_cpp"
base_url = "http://127.0.0.1:8080"
language = "zh"
max_duration_secs = 300
```

When `provider = "openai"` (or unset), behavior is unchanged: `api_key` must be configured, the endpoint is `{base_url}/audio/transcriptions`, and `model` is sent.

When `provider = "whisper_cpp"`, `api_key` and `model` are ignored, and the endpoint is `{base_url}/inference`. Users may configure `language` for whisper.cpp's language hint.

## Transcriber Architecture

### `WhisperCppTranscriber` (new)

```rust
pub struct WhisperCppTranscriber {
    settings: VoiceSettings,
    client: reqwest::Client,
}
```

- Implements the existing `Transcriber` trait.
- Builds the same multipart request but:
  - No `Authorization` header.
  - No `model` field in the form.
  - Endpoint: `{base_url.trim_end_matches('/')}/inference`.
- Response parsing reuses the existing `parse_transcription_response()` function (same `{"text":"..."}` JSON format).

### Selection logic in `spawn_worker()`

```rust
pub fn spawn_worker(settings: VoiceSettings) -> VoiceWorkerHandle {
    let transcriber: Arc<dyn Transcriber> = match settings.provider {
        VoiceProvider::OpenAi => Arc::new(OpenAiTranscriber::new(settings.clone())),
        VoiceProvider::WhisperCpp => Arc::new(WhisperCppTranscriber::new(settings.clone())),
    };
    spawn_worker_with_components(settings.clone(), Arc::new(CpalRecorder), transcriber)
}
```

## Default Values at Config Resolution

| Field | `OpenAi` default | `WhisperCpp` default |
|-------|------------------|----------------------|
| `base_url` | `https://api.openai.com/v1` | `http://127.0.0.1:8080` |
| `model` | `gpt-4o-mini-transcribe` | `""` (not sent) |
| `api_key` | Must be configured | None (not used) |

If `provider` is not set, defaults to `OpenAi` with existing defaults (fully backward compatible).

## Error Handling

- `WhisperCppTranscriber` does not require `api_key`. If `base_url` is empty, it errors at request time.
- Non-2xx responses from whisper.cpp are propagated via `parse_transcription_response()` which returns the HTTP status + first 200 chars of body.
- Cancellation via `CancellationToken` works identically to OpenAI path.

## Testing Plan

1. **Unit tests** for `WhisperCppTranscriber::transcribe()` with:
   - Successful response → returns text
   - Empty/blank text → error
   - HTTP error status → error with status code (no key leak)
   - Cancellation mid-request → cancellation error
   - Missing `language` config → no `language` field in form

2. **Wiremock tests** (matching existing pattern in `transcriber.rs`).

3. **Config resolution tests**:
   - Default provider → OpenAI with old defaults
   - Explicit `provider = "whisper_cpp"` → WhisperCpp with localhost defaults
   - Provider + custom base_url → custom endpoint

## Files Changed

| File | Change |
|------|--------|
| `crates/tact/src/config/types.rs` | Add `VoiceProvider` enum, update `VoiceSettings` and `VoiceTomlConfig` |
| `crates/tact/src/config/resolve.rs` | Resolve provider field, conditional defaults |
| `crates/tact/src/config/mod.rs` | Re-export `VoiceProvider` |
| `crates/tact/src/voice/transcriber.rs` | Add `WhisperCppTranscriber` + tests |
| `crates/tact/src/voice/mod.rs` | Select transcriber based on provider |
| `config.example.toml` | Document `provider` field |
| `book/NN_chapter_*.md` (voice chapter) | Document new provider option |

## Out of Scope

- Streaming transcription.
- Switching providers at runtime without restart.
- Support for whisper.cpp extra params (`temperature`, `no_speech_thold`, etc.) — can be added later as config fields.
- Other local whisper backends (e.g., faster-whisper, WhisperX, mlx-whisper).
