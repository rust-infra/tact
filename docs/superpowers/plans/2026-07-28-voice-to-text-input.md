# Voice-to-Text Input Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a macOS-first, mouse-clickable voice input button that records microphone audio, sends it to an independent OpenAI transcription configuration, and inserts the reviewed transcript into Tact's existing editor.

**Architecture:** Add a `tact::voice` core with validated voice settings, in-memory WAV encoding, an OpenAI multipart transcriber, and a cancellable worker whose recorder/transcriber dependencies can be replaced by test fakes. Pass optional voice settings into the interactive TUI; the TUI owns `VoiceState`, renders a right-aligned title-bar hit region, sends start/stop/cancel commands, drains worker events before drawing, and inserts successful text through the existing editor without changing `UserCommand` or Agent behavior.

**Tech Stack:** Rust 2024; Tokio channels/tasks and `tokio_util::sync::CancellationToken`; `cpal = "0.15"` for microphone input; `reqwest` multipart HTTP; `serde`/TOML config; ratatui/crossterm mouse events; `wiremock = "0.6"` local mock HTTP tests; existing TUI render/test harness.

## Global Constraints

- Target macOS first; do not add a platform-specific native GUI control.
- Voice credentials are independent of `[llm.providers.*]` and are never logged or copied into Agent/session history.
- Default voice backend is OpenAI `POST {base_url}/audio/transcriptions`; custom URLs are supported for testing but are not promised to implement the API.
- Default model is `gpt-4o-mini-transcribe`; default base URL is `https://api.openai.com/v1`; default language is `zh`; default maximum recording duration is 300 seconds.
- `max_duration_secs` is valid only in the inclusive range `1..=600`.
- Missing `[voice]` or `enabled = false` hides the button; `enabled = true` without an API key keeps the button visible and reports `[voice].api_key` on click.
- The first release has no interim transcription, continuous listening, auto-submit, audio persistence, local Whisper, multiple cloud vendors, speaker identification, or voice command execution.
- Successful transcription is inserted at the UTF-8-safe cursor and never submitted automatically; `/help` in a transcript is ordinary editor text until the user presses Enter.
- Recording and HTTP work must not block the TUI event loop; shutdown must restore terminal state even when voice work fails.
- CI tests must not require a microphone, macOS permission, cloud credentials, or real API charges.
- Do not alter `UserCommand`, Agent loop semantics, session message schema, or input-history persistence contracts.

---

## File and module map

Create or modify only the following feature-related files unless an existing test requires its adjacent module:

- Create `crates/tact/src/voice/mod.rs`: public voice settings, command/event types, component traits, worker orchestration, production worker constructor.
- Create `crates/tact/src/voice/wav.rs`: deterministic mono/PCM WAV encoder and unit tests.
- Create `crates/tact/src/voice/recorder.rs`: `cpal` default-input recorder, sample conversion, duration/stop/cancel handling.
- Create `crates/tact/src/voice/transcriber.rs`: OpenAI multipart client, response parsing, bounded timeout, cancellation, and mock-server tests.
- Modify `crates/tact/src/lib.rs`: expose `pub mod voice`.
- Modify `Cargo.toml`, `Cargo.lock`, `crates/tact/Cargo.toml`: add `cpal = "0.15"`, `tokio-util = "0.7"` with its `rt` feature, and `wiremock = "0.6"` as a tact dev-dependency; do not enable unnecessary audio backends/features.
- Modify `crates/tact/src/config/types.rs`, `resolve.rs`, `mod.rs`: add TOML and resolved voice settings.
- Modify `config.example.toml`, `book/21_chapter_config.md`, `book/21_chapter_config_zh.md`: document configuration.
- Create `crates/tui/src/widgets/state/voice.rs`: TUI phase, hit-region, worker channels, and event application.
- Modify `crates/tui/src/widgets/state/mod.rs` and `app/construct.rs`: store and initialize voice state.
- Modify `crates/tui/src/render/input.rs`: render right-aligned title-bar button and update hit region.
- Modify `crates/tui/src/handlers/mouse.rs`: recognize button clicks before log/panel hit testing.
- Modify `crates/tui/src/handlers/insert.rs`: keyboard cancellation fallback and transcript insertion helper.
- Modify `crates/tui/src/lib.rs`: pass voice settings, spawn/drain worker, repaint timing, and graceful shutdown.
- Modify `crates/tact-ui/src/interactive.rs`: pass resolved voice settings to `TuiConfig`.
- Modify `crates/tui/src/i18n.rs`, `book/23_chapter_tui.md`, `book/23_chapter_tui_zh.md`: labels, behavior, and data flow.
- Modify `book/26_chapter_issue.md`, `book/26_chapter_issue_zh.md` after shipping behavior: newest-first issue-log entry with pointers to this plan/spec and chapters.

---

### Task 1: Add voice configuration and dependency scaffolding

**Files:**
- Modify: `Cargo.toml`
- Modify: `crates/tact/Cargo.toml`
- Modify: `crates/tact/src/config/types.rs`
- Modify: `crates/tact/src/config/resolve.rs`
- Modify: `crates/tact/src/config/mod.rs`
- Modify: `crates/tact/src/lib.rs`
- Modify: `config.example.toml`
- Test: `crates/tact/src/config/types.rs` and `crates/tact/src/config/resolve.rs` existing test modules

**Interfaces:**
- Produce `VoiceTomlConfig` with `enabled: Option<bool>`, `api_key: Option<String>`, `base_url: Option<String>`, `model: Option<String>`, `language: Option<String>`, and `max_duration_secs: Option<u64>`.
- Produce resolved `VoiceSettings` with `enabled: bool`, `api_key: Option<String>`, `base_url: String`, `model: String`, `language: Option<String>`, and `max_duration_secs: u64`.
- Add `ResolvedConfig::voice: VoiceSettings` and re-export the type from `config/mod.rs`.
- Add workspace/package dependencies `cpal = "0.15"`, `tokio-util = "0.7"` with its `rt` feature, and `wiremock = "0.6"` as a tact dev-dependency; do not enable unnecessary audio backends/features.

- [ ] **Step 1: Write failing config parsing tests**

Add tests covering the exact TOML shape and defaults:

```rust
#[test]
fn parse_voice_config_and_defaults() {
    let cfg: TactTomlConfig = toml::from_str(r#"
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
"#).unwrap();
    assert_eq!(cfg.voice.enabled, Some(true));
    assert_eq!(cfg.voice.max_duration_secs, Some(45));
}

#[test]
fn resolve_voice_defaults_and_validation() {
    let cfg = resolve_config(&empty_cli_args_with_openai(), &TactTomlConfig::default(), None).unwrap();
    assert!(!cfg.voice.enabled);
    assert_eq!(cfg.voice.base_url, "https://api.openai.com/v1");
    assert_eq!(cfg.voice.model, "gpt-4o-mini-transcribe");
    assert_eq!(cfg.voice.language.as_deref(), Some("zh"));
    assert_eq!(cfg.voice.max_duration_secs, 300);
}

#[test]
fn reject_voice_duration_outside_safe_range() {
    let mut toml_cfg = openai_toml_config();
    toml_cfg.voice.enabled = Some(true);
    toml_cfg.voice.max_duration_secs = Some(0);
    let err = resolve_config(&empty_cli_args(), &toml_cfg, None).unwrap_err();
    assert!(err.to_string().contains("voice.max_duration_secs"));
}
```

Use the existing test helpers and keep the LLM provider fixture valid so failures are specifically about voice fields.

- [ ] **Step 2: Run the focused tests and verify failure**

Run:

```bash
cargo test -p tact config::types::tests::parse_voice_config_and_defaults -- --nocapture
cargo test -p tact config::resolve::tests::resolve_voice_defaults_and_validation -- --nocapture
```

Expected: compilation/test failure because `TactTomlConfig::voice`, `VoiceSettings`, and resolver logic do not exist.

- [ ] **Step 3: Implement the config types and resolver**

Add `voice: VoiceTomlConfig` to `TactTomlConfig`, add the resolved type and constants, and resolve it before constructing `ResolvedConfig`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct VoiceTomlConfig {
    pub enabled: Option<bool>,
    pub api_key: Option<String>,
    pub base_url: Option<String>,
    pub model: Option<String>,
    pub language: Option<String>,
    pub max_duration_secs: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct VoiceSettings {
    pub enabled: bool,
    pub api_key: Option<String>,
    pub base_url: String,
    pub model: String,
    pub language: Option<String>,
    pub max_duration_secs: u64,
}
```

Resolve empty API keys to `None`, trim an empty language to `None`, use the documented defaults, and return an error (not clamp silently) when duration is outside `1..=600`. Do not require an API key while resolving: the UI must be able to show the button and explain the missing key on click. Add `pub mod voice;` in `crates/tact/src/lib.rs` as the module scaffold.

- [ ] **Step 4: Run config tests and compile the scaffold**

Run:

```bash
cargo test -p tact config::types::tests config::resolve::tests
cargo check -p tact
```

Expected: PASS; existing config tests remain green and `ResolvedConfig` contains disabled-by-default voice settings.

- [ ] **Step 5: Update the example configuration**

Add a `[voice]` section after `[ui]` with the exact defaults and comments explaining that the key is independent from LLM providers, audio is sent to the configured service, and `enabled = false` hides the button. Keep the example key empty and never add a real credential.

- [ ] **Step 6: Commit the configuration slice**

```bash
git add Cargo.toml Cargo.lock crates/tact/Cargo.toml crates/tact/src/lib.rs crates/tact/src/config/types.rs crates/tact/src/config/resolve.rs crates/tact/src/config/mod.rs config.example.toml
git commit -m "feat: add voice input configuration"
```

---

### Task 2: Implement deterministic WAV encoding and OpenAI transcription

**Files:**
- Create: `crates/tact/src/voice/wav.rs`
- Create: `crates/tact/src/voice/transcriber.rs`
- Modify: `crates/tact/src/voice/mod.rs`
- Test: `crates/tact/src/voice/wav.rs`
- Test: `crates/tact/src/voice/transcriber.rs`

**Interfaces:**
- Produce `pub fn encode_wav_mono_16k(samples: &[i16]) -> anyhow::Result<Vec<u8>>` with a 16 kHz, mono, PCM 16-bit RIFF/WAV header.
- Produce `#[async_trait] pub trait Transcriber: Send + Sync { async fn transcribe(&self, wav: Vec<u8>, cancel: CancellationToken) -> anyhow::Result<String>; }`.
- Produce `pub struct OpenAiTranscriber` with `new(settings: VoiceSettings) -> Self` and the trait implementation.
- Keep response parsing independent in `fn parse_transcription_response(status: StatusCode, body: &[u8]) -> anyhow::Result<String>` so it can be tested without HTTP.

- [ ] **Step 1: Write failing WAV tests**

```rust
#[test]
fn encode_wav_has_pcm_mono_16k_header_and_samples() {
    let wav = encode_wav_mono_16k(&[0, i16::MAX, i16::MIN]).unwrap();
    assert_eq!(&wav[0..4], b"RIFF");
    assert_eq!(&wav[8..12], b"WAVE");
    assert_eq!(&wav[12..16], b"fmt ");
    assert_eq!(u16::from_le_bytes([wav[22], wav[23]]), 1); // PCM
    assert_eq!(u16::from_le_bytes([wav[24], wav[25]]), 1); // mono
    assert_eq!(u32::from_le_bytes(wav[24..28].try_into().unwrap()), 16_000);
    assert_eq!(u16::from_le_bytes([wav[34], wav[35]]), 16);
    assert_eq!(&wav[36..40], b"data");
    assert_eq!(wav.len(), 44 + 6);
}

#[test]
fn encode_empty_wav_has_zero_data_length() {
    let wav = encode_wav_mono_16k(&[]).unwrap();
    assert_eq!(u32::from_le_bytes(wav[40..44].try_into().unwrap()), 0);
    assert_eq!(wav.len(), 44);
}
```

Also assert that an oversized sample slice whose byte length cannot fit a WAV `u32` length returns an error without allocating a second copy.

- [ ] **Step 2: Run WAV tests and verify failure**

Run `cargo test -p tact voice::wav`; expected failure because the module and encoder are absent.

- [ ] **Step 3: Implement the minimal WAV encoder**

Write the canonical 44-byte RIFF header in little-endian order, append each `i16` sample as little-endian bytes, and use checked arithmetic for file/data sizes. Reject lengths that exceed `u32::MAX` bytes. Do not resample in this function; the recorder will provide 16 kHz mono samples.

- [ ] **Step 4: Run WAV tests and verify pass**

Run `cargo test -p tact voice::wav`; expected PASS.

- [ ] **Step 5: Write failing transcription-client tests**

Use a local mock HTTP server (the test-only dependency added in Task 1) to assert:

```rust
#[tokio::test]
async fn transcriber_sends_expected_multipart_and_parses_text() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/audio/transcriptions"))
        .and(header("authorization", "Bearer voice-test"))
        .and(body_string_contains("name=\"model\""))
        .and(body_string_contains("gpt-4o-mini-transcribe"))
        .and(body_string_contains("name=\"language\""))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"text":"你好 Tact"})))
        .mount(&server).await;
    let settings = test_voice_settings(&server.uri());
    let text = OpenAiTranscriber::new(settings)
        .transcribe(vec![1, 2, 3], CancellationToken::new()).await.unwrap();
    assert_eq!(text, "你好 Tact");
}

#[tokio::test]
async fn transcriber_rejects_http_error_and_missing_text_without_leaking_key() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(401).set_body_string("unauthorized"))
        .mount(&server).await;
    let err = OpenAiTranscriber::new(test_voice_settings(&server.uri()))
        .transcribe(vec![1, 2, 3], CancellationToken::new()).await.unwrap_err();
    assert!(err.to_string().contains("401"));
    assert!(!err.to_string().contains("voice-test"));

    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .mount(&server).await;
    let err = OpenAiTranscriber::new(test_voice_settings(&server.uri()))
        .transcribe(vec![1, 2, 3], CancellationToken::new()).await.unwrap_err();
    assert!(err.to_string().contains("text"));
    assert!(!err.to_string().contains("voice-test"));
}

#[tokio::test]
async fn transcriber_cancellation_aborts_request() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(30)))
        .mount(&server).await;
    let cancel = CancellationToken::new();
    let task = tokio::spawn(OpenAiTranscriber::new(test_voice_settings(&server.uri()))
        .transcribe(vec![1, 2, 3], cancel.clone()));
    cancel.cancel();
    let err = task.await.unwrap().unwrap_err();
    assert!(err.to_string().to_ascii_lowercase().contains("cancel"));
}
```

Add parser tests for valid JSON, empty text, missing text, malformed JSON, and non-2xx response. Assert that `voice-test` is never present in returned error strings.

- [ ] **Step 6: Run transcription tests and verify failure**

Run `cargo test -p tact voice::transcriber`; expected failures until the client exists.

- [ ] **Step 7: Implement the client and parser**

Construct the endpoint as `base_url.trim_end_matches('/') + "/audio/transcriptions"`. Build a multipart request with `file` named `recording.wav` and `application/octet-stream` or `audio/wav`, plus `model`, `response_format=json`, and `language` only when non-empty. Add `Authorization: Bearer <key>` only when the key exists; return a configuration error naming `[voice].api_key` before making a request when it is missing. Use a bounded `reqwest::Client` timeout and `tokio::select!` between `request.send()` and `cancel.cancelled()`.

Parse `{ "text": "..." }`, reject absent/whitespace-only text, map non-2xx to a status plus sanitized short body (never headers or credentials), and avoid retries.

- [ ] **Step 8: Run all core voice tests**

Run `cargo test -p tact voice`; expected PASS without network access outside the local mock listener.

- [ ] **Step 9: Commit the core client slice**

```bash
git add crates/tact/src/voice Cargo.lock
git commit -m "feat: add voice wav encoder and transcription client"
```

---

### Task 3: Add the cancellable recorder and voice worker

**Files:**
- Create: `crates/tact/src/voice/recorder.rs`
- Modify: `crates/tact/src/voice/mod.rs`
- Test: `crates/tact/src/voice/mod.rs`
- Test: `crates/tact/src/voice/recorder.rs`

**Interfaces:**
- Produce `pub enum VoiceCommand { Start, Stop, Cancel, Shutdown }`.
- Produce the approved `pub enum VoiceEvent { RecordingStarted, RecordingStopped, Transcribing, Transcript(String), Error(String), Cancelled }`.
- Produce `#[async_trait] pub trait Recorder: Send + Sync { async fn record(&self, max_duration: Duration, stop: CancellationToken, cancel: CancellationToken) -> anyhow::Result<Option<Vec<i16>>>; }`.
- Produce `pub fn spawn_worker(settings: VoiceSettings) -> VoiceWorkerHandle` and `pub fn spawn_worker_with_components(settings: VoiceSettings, recorder: Arc<dyn Recorder>, transcriber: Arc<dyn Transcriber>) -> VoiceWorkerHandle`.
- `VoiceWorkerHandle` exposes `command_tx: UnboundedSender<VoiceCommand>`, `event_rx: UnboundedReceiver<VoiceEvent>`, and an async `shutdown(self)` that cancels and joins the task without panicking.

- [ ] **Step 1: Write worker tests with fake components**

Implement test-only fake recorder/transcriber in the test module. Cover the exact event sequences:

```rust
#[tokio::test]
async fn start_stop_records_then_transcribes_and_emits_text() {
    let mut worker = test_worker(Some("transcript"));
    worker.command_tx.send(VoiceCommand::Start).unwrap();
    assert_event(&mut worker.event_rx, VoiceEvent::RecordingStarted).await;
    worker.command_tx.send(VoiceCommand::Stop).unwrap();
    assert_event(&mut worker.event_rx, VoiceEvent::RecordingStopped).await;
    assert_event(&mut worker.event_rx, VoiceEvent::Transcribing).await;
    assert_event(&mut worker.event_rx, VoiceEvent::Transcript("transcript".into())).await;
}

#[tokio::test]
async fn cancel_recording_emits_cancelled_and_never_transcribes() {
    // Start the fake recorder, send Cancel, assert the next event is Cancelled, and assert the fake transcriber call count stays zero.
}

#[tokio::test]
async fn missing_api_key_is_reported_by_transcriber_without_recording_failure() {
    // Complete a fake recording with VoiceSettings.api_key = None, assert an Error mentioning `[voice].api_key`, and assert the worker returns to Idle.
}

#[tokio::test]
async fn second_start_while_active_is_ignored_and_shutdown_is_safe() {
    // Send Start twice, assert only one RecordingStarted event, then send Shutdown and await a non-panicking join.
}
```

Use a fake recorder that waits on `stop`/`cancel` tokens and a fake transcriber that records invocation count. Add a duration-limit fake result to verify automatic transition to transcription.

- [ ] **Step 2: Run worker tests and verify failure**

Run `cargo test -p tact voice::mod`; expected failure because command/event types and worker are absent.

- [ ] **Step 3: Implement command/event state machine**

The worker task must accept one active operation at a time. On `Start`, create child stop/cancel tokens, emit `RecordingStarted`, and run the recorder while still accepting `Stop`, `Cancel`, and `Shutdown`. `Stop` cancels only the stop token; a completed non-empty recording emits `RecordingStopped`, then `Transcribing`, then calls the transcriber. `Cancel` cancels the active operation, emits `Cancelled`, and discards audio. A duration expiry is treated as a normal completed recording. Empty audio returns to idle without making an HTTP request. Transcriber success emits `Transcript`; every failure emits a sanitized `Error`; late results after cancellation are ignored.

Ensure event ordering is deterministic and command-channel closure is equivalent to shutdown. Never unwrap a send or join in production code.

- [ ] **Step 4: Implement the `cpal` recorder**

Select `default_input_device`, inspect its supported config, and capture through an input stream callback into an `Arc<Mutex<Vec<i16>>>`. Convert `i16`, `u16`, and `f32` callback samples to signed 16-bit mono; if the device has multiple channels, average channel samples per frame. Resample to 16 kHz with a small linear/interleaving conversion routine that is deterministic and bounded; the WAV encoder remains fixed at 16 kHz. Build the stream before emitting `RecordingStarted` so missing devices/configuration errors are reported immediately.

Use a Tokio blocking bridge only around cpal setup/stream synchronization; do not hold a mutex across `.await`. Stop the stream on stop, cancel, stream error, duration expiry, and drop. Map permission/device failures to actionable errors: use "no usable microphone found" for a missing default device, and mention microphone access in macOS System Settings for permission/stream failures. Do not write audio files.

- [ ] **Step 5: Add recorder conversion tests**

Test `i16`, `u16`, and `f32` sample conversion, multi-channel averaging, clamping, empty input, and the 1–600 second duration passed by the worker. Keep real-device tests ignored and document that they run only on a macOS machine with microphone permission.

- [ ] **Step 6: Run voice tests and platform compilation**

Run:

```bash
cargo test -p tact voice
cargo check -p tact --target aarch64-apple-darwin
```

Expected: all fake/WAV/client tests PASS; the macOS target check succeeds when the Rust target is installed. If the target is unavailable, run `cargo check -p tact` and record the missing target as an environment prerequisite rather than weakening tests.

- [ ] **Step 7: Commit the worker slice**

```bash
git add crates/tact/src/voice Cargo.lock
 git commit -m "feat: add cancellable voice recording worker"
```

---

### Task 4: Add TUI VoiceState, transcript insertion, and the clickable title-bar button

**Files:**
- Create: `crates/tui/src/widgets/state/voice.rs`
- Modify: `crates/tui/src/widgets/state/mod.rs`
- Modify: `crates/tui/src/widgets/state/app/construct.rs`
- Modify: `crates/tui/src/render/input.rs`
- Modify: `crates/tui/src/handlers/mouse.rs`
- Modify: `crates/tui/src/handlers/insert.rs`
- Modify: `crates/tui/src/i18n.rs`
- Test: `crates/tui/src/render/input.rs`, `crates/tui/src/handlers/mouse.rs`, and `crates/tui/src/widgets/state/voice.rs`

**Interfaces:**
- Produce `VoicePhase::{Disabled, Idle, Recording{started_at: Instant}, Transcribing}`.
- Produce `VoiceState` containing phase, `button_area: Rect`, `command_tx`, and worker event receiver; provide `set_button_area`, `is_clickable`, `start/stop/cancel`, `drain_events`, and `shutdown` methods.
- Produce `pub(crate) fn insert_transcript(input: &mut String, cursor: &mut usize, transcript: &str)` with UTF-8-safe insertion and one-space separation rules.
- Keep the voice button hit region in `VoiceState::button_area: Rect`; rendering updates it every frame, and hidden/disabled state sets it to `Rect::default()`.

- [ ] **Step 1: Write failing transcript-insertion tests**

```rust
#[test]
fn insert_transcript_at_unicode_cursor_with_separator() {
    let mut input = "你好世界".to_string();
    let mut cursor = "你好".len();
    insert_transcript(&mut input, &mut cursor, "请检查代码");
    assert_eq!(input, "你好 请检查代码世界");
    assert_eq!(cursor, "你好 请检查代码".len());
}

#[test]
fn insert_transcript_preserves_newlines_and_does_not_execute_slash_text() {
    let mut input = "prefix\n".to_string();
    let mut cursor = input.len();
    insert_transcript(&mut input, &mut cursor, "/help\n下一行");
    assert_eq!(input, "prefix\n/help\n下一行");
}

#[test]
fn blank_transcript_is_noop_and_invalid_cursor_is_clamped_to_boundary() {
    // Assert that whitespace-only text leaves both input and cursor unchanged, and a cursor inside a UTF-8 code point is clamped before insertion.
}
```

- [ ] **Step 2: Run insertion tests and verify failure**

Run `cargo test -p tui insert_transcript`; expected failure because the helper does not exist.

- [ ] **Step 3: Implement insertion and TUI phase state**

Clamp the cursor with `floor_char_boundary`, compute the preceding character with `.chars().last()`, add a separator only when the preceding character is non-whitespace and the transcript's first character is non-whitespace, insert with `String::insert_str`, and advance by inserted byte length. Treat whitespace-only transcript as a no-op. Define phase transitions so duplicate starts and clicks during transcription are ignored.

`drain_events` must apply `RecordingStarted`, `RecordingStopped`, `Transcribing`, `Transcript`, `Error`, and `Cancelled`; transcript success calls the insertion helper and returns to Idle, while errors set `App.flash_msg` and return to Idle.

- [ ] **Step 4: Write failing render and mouse tests**

Add tests that set a fake enabled voice state and draw an 80-column input area, then assert the buffer includes `[🎙 Voice]` (or the localized equivalent), the button area is non-empty, and recording/transcribing labels replace it. Add mouse tests using existing helpers:

```rust
handle_mouse_event(&mut app, mouse_down(button_x, title_row));
assert!(matches!(app.voice.phase, VoicePhase::Recording { .. }));
handle_mouse_event(&mut app, mouse_down(button_x, title_row));
assert!(matches!(app.voice.phase, VoicePhase::Transcribing));
handle_mouse_event(&mut app, mouse_down(1, title_row));
// outside click does not start a second operation
```

Also assert a disabled/hidden voice configuration leaves the button area empty and a resize/render recomputes coordinates.

- [ ] **Step 5: Run render/mouse tests and verify failure**

Run `cargo test -p tui render::input::render_tests handlers::mouse::tests`; expected failures until state/render/hit testing is wired.

- [ ] **Step 6: Render the right-aligned title-bar button**

In `render_input_box`, preserve the existing left title and add a right-aligned `Line` title for the voice button. Compute the label width using `unicode-width`; reserve one cell of padding and store a `Rect` covering the button text on the top border row. Use accent/success style for Idle, warning style for Recording, and muted/disabled style for Transcribing. Keep the existing bottom title and cursor behavior unchanged.

The exact labels must come from `Messages` (English and Chinese), including `voice_idle`, `voice_stop`, `voice_transcribing`, `voice_missing_config`, `voice_error`, and `voice_cancelled` text as needed. Avoid relying on emoji width for hit testing: use the same measured label width that is used to construct the hit rectangle.

- [ ] **Step 7: Add mouse handling and keyboard cancellation**

Handle `MouseEventKind::Down(MouseButton::Left)` against the voice button before overlay/log handling. Idle sends `VoiceCommand::Start`, Recording sends `Stop`, and Transcribing ignores the click. Keep outside clicks unchanged. In insert-mode `Esc`, if `VoicePhase` is Recording or Transcribing, send `VoiceCommand::Cancel`, set the phase to Idle after the Cancelled event, and mark the app dirty; otherwise preserve the existing overlay dismissal.

- [ ] **Step 8: Run TUI tests and full crate checks**

Run:

```bash
cargo test -p tui
cargo clippy -p tui --all-targets -- -D warnings
```

Expected: existing mouse/render/input tests plus the new voice tests PASS; no existing input or popup behavior changes.

- [ ] **Step 9: Commit the TUI slice**

```bash
git add crates/tui/src/widgets/state crates/tui/src/render/input.rs crates/tui/src/handlers/mouse.rs crates/tui/src/handlers/insert.rs crates/tui/src/i18n.rs
git commit -m "feat: add clickable voice input button"
```

---

### Task 5: Bridge the worker into interactive TUI lifecycle and finish documentation

**Files:**
- Modify: `crates/tui/src/lib.rs`
- Modify: `crates/tact-ui/src/interactive.rs`
- Modify: `crates/tui/src/render/test_harness.rs` and `crates/tui/src/test_support.rs`: update their `App::new` fixtures with disabled voice settings and disconnected channels.
- Modify: `book/21_chapter_config.md`
- Modify: `book/21_chapter_config_zh.md`
- Modify: `book/23_chapter_tui.md`
- Modify: `book/23_chapter_tui_zh.md`
- Modify: `book/26_chapter_issue.md`
- Modify: `book/26_chapter_issue_zh.md`
- Test: `crates/tui/src/lib.rs` and `crates/tact-ui` integration tests

**Interfaces:**
- Extend `TuiConfig` with `pub voice: tact::config::VoiceSettings` (or `Option<VoiceSettings>` only if disabled settings cannot be represented); preserve all existing callers by updating constructors/tests.
- `run_tui` creates the production worker only for interactive TUI startup, drains voice events before rendering, and shuts it down before terminal restoration.
- Headless mode never constructs `TuiConfig` or initializes cpal.

- [ ] **Step 1: Write failing bridge/lifecycle tests**

Add a TUI test that constructs an enabled test `TuiConfig`/App with injected worker channels, sends `VoiceEvent::Transcript("voice text")`, drains events, and asserts `app.input` changes while no `UserCommand::SubmitTask` is received. Add a test that disabled config does not spawn a worker/button. Add an integration test that `interactive.rs` passes `settings().voice` into `TuiConfig` without changing the Agent command loop.

- [ ] **Step 2: Run bridge tests and verify failure**

Run `cargo test -p tui` and `cargo test -p tact-ui`; expected compile failures until the new `TuiConfig` field and event-loop drain are added.

- [ ] **Step 3: Wire settings and worker startup**

In `interactive.rs`, read the already installed `tact::config::settings().voice` and pass a clone into `TuiConfig`. In `run_tui`, create `spawn_worker(voice_settings)` only when `enabled`; initialize `VoiceState` with the command/event channels. Preserve the missing-key behavior by spawning the worker with settings containing `api_key: None`; the recorder can still be tested/started, and the transcriber reports the configuration error on stop.

Update all test/headless constructors with disabled voice settings or an injected fake worker. Do not call `cpal` from `App::new`, render tests, headless tests, or config-only commands.

- [ ] **Step 4: Drain events and update repaint scheduling**

Before the existing Agent/account/plugin drain and `terminal.draw`, call `app.drain_voice_events()`. Include active recording/transcribing in `should_repaint`; while recording, use the existing short active poll interval so the elapsed label updates, while transcription keeps the UI responsive to events/cancellation. Ensure `dirty` is set after every voice command/event.

- [ ] **Step 5: Add cancellation and graceful shutdown**

On `Esc`, button Stop, app quit, and channel closure, send the appropriate command. Before `disable_raw_mode`/`LeaveAlternateScreen`, call `app.shutdown_voice().await`; ignore only expected cancellation/closed-channel errors and never mask terminal restoration errors. Verify a failed worker cannot leave the button permanently in Recording or Transcribing.

- [ ] **Step 6: Run complete automated verification**

Run:

```bash
cargo fmt --all -- --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo check -p tact-ui
```

Expected: all workspace tests and clippy checks PASS without microphone access or cloud credentials. Run ignored hardware tests manually only on macOS with permission:

```bash
cargo test -p tact voice -- --ignored --nocapture
```

- [ ] **Step 7: Update bilingual configuration and TUI chapters**

Document the `[voice]` fields, independent API key, disabled-by-default behavior, macOS microphone permission, clickable title-bar states, worker data flow, transcript insertion, cancellation, and explicit non-goals in both English and Chinese chapters. Keep section numbering and heading hierarchy aligned between paired chapters.

- [ ] **Step 8: Add the shipped behavior issue-log entries**

Prepend matching English/Chinese entries dated `2026-07-28`, type `bugfix` or the repository’s feature-compatible type, describing the keyboard-only input motivation, the clickable macOS Voice button, cloud transcription, review-before-submit behavior, and pointers to:

- `docs/superpowers/specs/2026-07-28-voice-to-text-design.md`
- `docs/superpowers/plans/2026-07-28-voice-to-text-input.md`
- Ch 21 and Ch 23
- `crates/tact/src/voice/` and TUI input/handler modules

- [ ] **Step 9: Review docs and diff**

Run:

```bash
git diff --check
git diff --stat
git status --short
```

Manually verify no API key, audio bytes, or temporary recording path appears in docs, logs, test fixtures committed to the repository, or error snapshots. Confirm `AGENTS.md`’s pre-existing unrelated worktree change remains uncommitted.

- [ ] **Step 10: Commit the integrated feature**

```bash
git add crates/tui/src/lib.rs crates/tact-ui/src/interactive.rs crates/tui/src/render/test_harness.rs crates/tui/src/test_support.rs book config.example.toml
git commit -m "feat: integrate voice-to-text input"
```

---

## Plan self-review

- **Spec coverage:** Configuration/defaults are covered by Task 1; WAV and OpenAI request/error/cancellation behavior by Task 2; cpal capture, duration, device/permission errors, worker state, duplicate starts, and shutdown by Task 3; title-bar button, mouse hit testing, state display, UTF-8 insertion, slash-text safety, keyboard fallback, and repainting by Task 4; interactive lifecycle, headless exclusion, bilingual docs, issue log, and full verification by Task 5.
- **Scope check:** The plan keeps audio capture, transcription, worker orchestration, TUI, and docs as one feature because each task produces a testable slice and all are required for the clickable MVP; no real-time or local backend work is included.
- **Placeholder scan:** The plan contains no unresolved implementation placeholders. Every test sketch names the input, expected event/result, and exact condition to assert; the implementation task must turn those sketches into concrete tests before coding.
- **Type consistency:** `VoiceSettings` is produced in Task 1 and consumed by `OpenAiTranscriber`/`spawn_worker` in Tasks 2–3 and by `TuiConfig` in Task 5. `VoiceCommand`, `VoiceEvent`, `Recorder`, and `Transcriber` are defined before worker/TUI consumers. `insert_transcript` is defined and tested in Task 4 before render/bridge callers.
- **Verification:** The plan includes focused red/green tests per task, workspace formatting/tests/clippy, macOS target compilation, ignored hardware validation, and a final diff/status check.
