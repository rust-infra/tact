# Voice-to-Text Input Design

- **Date:** 2026-07-28
- **Status:** Approved design; implementation has not started
- **Scope:** macOS-first voice input for the interactive TUI

## 1. Goals and non-goals

Tact's interactive TUI currently accepts keyboard and paste input. This feature adds a discoverable, mouse-clickable voice button to the input box. A user can record a spoken request, send it to a cloud transcription service, review the returned text in the existing editor, and submit it through the existing `UserCommand::SubmitTask` path.

The first release targets macOS and accepts cloud transcription. It must not couple voice input to the currently selected LLM provider: the user may be using Anthropic, DeepSeek, Kimi, or an OpenAI-compatible endpoint that does not implement audio transcription.

The first release does **not** include real-time interim transcription, continuous listening, automatic submission, audio persistence, local Whisper, multiple cloud vendors, speaker identification, or voice control of Tact commands.

## 2. User experience

### 2.1 Input button

When voice input is configured, the main input box title bar shows a right-aligned button:

```text
Message                                      [🎙 Voice]
```

The button is a TUI hit-test region, not a native macOS control. It is rendered by `render_input_box` and its coordinates are recorded for mouse handling. Resize must recompute the hit region on the next render.

The button has these states:

```text
Idle:          Message                         [🎙 Voice]
Recording:     Recording 00:08                 [■ Stop]
Transcribing:  Transcribing...                 [⟳ ...]
```

The recording duration is displayed while recording. The button is disabled while transcribing so a second worker cannot be started. `Esc` remains a keyboard fallback for cancelling recording or requesting cancellation of transcription.

### 2.2 State transitions

```text
Idle
  └─ click Voice ───────────────> Recording

Recording
  ├─ click Stop ────────────────> Transcribing
  ├─ press Esc ─────────────────> Idle (discard audio)
  └─ duration limit reached ────> Transcribing

Transcribing
  ├─ success ───────────────────> Idle (insert transcript)
  ├─ error ─────────────────────> Idle (show error)
  └─ Esc ───────────────────────> Idle (cancel request)
```

Recording and transcription must not submit an Agent task. Existing keyboard editing remains available, and a successful transcript is inserted into the editor rather than submitted. If an Agent task is already running, the normal existing busy-submit rule still applies.

### 2.3 Transcript insertion

The returned transcript is inserted at the current UTF-8-safe cursor position. Existing text and newlines are preserved. If non-whitespace text immediately precedes the insertion and the transcript does not begin with whitespace, insert one separating space. An empty or whitespace-only transcript leaves the input unchanged.

The transcript is never interpreted as a command during insertion. For example, a transcript containing `/help` is ordinary editor text; it only becomes a slash command if the user subsequently submits it through the normal Enter path. The existing input-history save path records the final submitted text, not audio.

## 3. Architecture

The feature is split into three boundaries:

```text
TUI mouse/event loop
        │ start / stop / cancel
        ▼
Voice worker (background Tokio task)
        │ recording result
        ▼
Recorder (macOS microphone via cpal)
        │ in-memory WAV
        ▼
OpenAI transcription client (reqwest multipart)
        │ VoiceEvent
        ▼
TUI VoiceState + existing App.input
```

The worker owns the long-running recording and network operations so the TUI event loop continues to repaint, process mouse events, and handle Agent updates. The TUI only owns presentation state and the worker lifecycle handle. The recording and HTTP implementations must be injectable or independently testable; TUI tests must not require a microphone or a real API key.

Suggested module placement:

```text
crates/tact/src/voice/
├── mod.rs              # public types, worker coordination, VoiceEvent
├── recorder.rs         # cpal capture and WAV encoding
└── transcriber.rs      # OpenAI multipart client

crates/tui/src/
├── widgets/state/voice.rs  # VoiceState and hit-region state
├── render/input.rs         # title-bar button and status rendering
├── handlers/mouse.rs       # button hit testing
└── lib.rs                  # worker event draining and lifecycle
```

The exact public API may be adjusted to existing crate conventions, but the dependency direction must remain: voice implementation is independent of rendering, and the Agent protocol is unchanged.

### 3.1 Voice events

The worker reports a small event set to the TUI:

```rust
pub enum VoiceEvent {
    RecordingStarted,
    RecordingStopped,
    Transcribing,
    Transcript(String),
    Error(String),
    Cancelled,
}
```

The TUI drains pending voice events before drawing, matching the existing update-before-render invariant. Errors become a user-visible flash/system message without exposing secrets. A worker must terminate before another recording can start.

### 3.2 Recording

The first implementation uses the default macOS input device through `cpal`. Captured samples are converted to mono, normalized to 16-bit PCM, and encoded as an in-memory WAV at 16 kHz. The feature does not require `ffmpeg`, Homebrew, or an external recording process. Audio is never written to a persistent file.

The default maximum duration is 300 seconds, with configuration validation restricting it to a safe range of 1–600 seconds. Reaching the limit automatically stops capture and starts transcription. Empty recordings are discarded without an API request.

The implementation must handle device absence, stream errors, and microphone permission denial. macOS microphone permission behavior (including the usage-description metadata required by the packaged CLI distribution) must be verified during implementation; failure must produce an actionable message directing the user to System Settings rather than panic or wedge the TUI.

### 3.3 OpenAI transcription

The default client sends an in-memory WAV through the OpenAI transcription endpoint:

```text
POST {voice.base_url}/audio/transcriptions
Authorization: Bearer <voice.api_key>
Content-Type: multipart/form-data

file: recording.wav
model: gpt-4o-mini-transcribe
language: zh       # omitted when configured empty
response_format: json
```

`base_url` defaults to `https://api.openai.com/v1`, but is configurable for mock servers and future compatible services. The first release guarantees the OpenAI API shape only; a custom URL is not a promise that every compatible endpoint supports audio. The API key is independent from `[llm.providers.*]`, is never logged, and must not appear in user-facing error text.

The HTTP client uses bounded timeouts and supports cancellation. Non-2xx responses, malformed JSON, missing `text`, transport failures, and timeouts become safe user-visible errors. Failed requests are not retried automatically, avoiding accidental duplicate charges.

## 4. Configuration

Add an optional section:

```toml
[voice]
enabled = false
api_key = ""
base_url = "https://api.openai.com/v1"
model = "gpt-4o-mini-transcribe"
language = "zh"
max_duration_secs = 300
```

Rules:

1. Missing `[voice]` or `enabled = false` disables the feature and hides the button.
2. `enabled = true` without an API key keeps the button visible but reports a configuration error when clicked, allowing users to discover the required setting.
3. An empty `language` omits the language field and delegates language detection to the service.
4. `max_duration_secs` must be between 1 and 600 seconds.
5. Voice credentials are not inherited from or written into the active LLM provider entry.
6. Headless mode does not initialize a microphone or voice worker.

The example configuration and the relevant configuration documentation must be updated in the same change.

## 5. Error, cancellation, and shutdown behavior

- No input device: return “no usable microphone found” (localized through the existing message mechanism where appropriate).
- Permission denied: explain that microphone access must be enabled for Tact/Terminal in macOS System Settings.
- Missing API key: identify `[voice].api_key` without printing the key.
- Network/API failure: show a concise error and return to Idle; leave the editor unchanged.
- Empty transcript: return to Idle without changing the editor.
- Cancel during recording: stop the stream and discard the in-memory buffer.
- Cancel during transcription: signal cancellation, abort the request, release the buffer, and ignore late results.
- Tact shutdown: cancel the worker, stop capture, await worker termination as far as the shutdown path permits, and always restore terminal raw mode/alternate-screen state.
- No operation may panic because a worker, audio device, or channel closes unexpectedly.

## 6. Testing

### 6.1 Pure unit tests

- WAV output is mono, 16 kHz, PCM 16-bit, including empty input and sample boundary values.
- Transcript insertion handles cursor positions, Unicode/CJK boundaries, multiline content, spacing, empty results, and slash-command text.
- Voice state transitions cover start, stop, transcribing, success, error, cancellation, duration limit, and duplicate-click prevention.
- Configuration defaults and validation cover missing sections, disabled voice, missing key, custom base URL, empty language, and duration boundaries.
- Transcription response parsing handles valid JSON, missing/empty text, non-2xx bodies, malformed JSON, timeout, and cancellation.

### 6.2 Mock HTTP tests

Use a local mock HTTP server to assert multipart field names and values, authorization behavior, response parsing, safe error handling, and cancellation. These tests must never contact OpenAI or incur API charges.

### 6.3 TUI tests

- The title-bar button renders only when enabled.
- Idle, recording, and transcribing labels/styles render correctly.
- The button hit region remains correct after terminal resize.
- Mouse down on the button starts/stops the worker; clicks outside it do nothing.
- Worker events are applied before rendering and transcripts are inserted without submission.
- Existing Agent busy-state and keyboard-submit behavior remains unchanged.

### 6.4 Hardware/API tests

Real microphone and real OpenAI tests are opt-in/ignored and documented for local validation. CI runs without microphone access, macOS permissions, or cloud credentials.

## 7. Documentation and compatibility

Because this changes a user-visible TUI input capability and adds public configuration, the implementation must update:

- `config.example.toml` and the relevant configuration chapter;
- `book/23_chapter_tui.md` and `book/23_chapter_tui_zh.md` for the button, states, and data flow;
- `book/26_chapter_issue.md` and `book/26_chapter_issue_zh.md` with a newest-first bugfix/feature entry describing the motivation, final behavior, and pointers to this spec and related TUI/config chapters.

The Agent loop, `UserCommand`, session message schema, and input-history persistence contracts remain unchanged.

## 8. Acceptance criteria

The feature is complete when a macOS user with `[voice].enabled = true` and a valid independent voice API key can:

1. Click the Voice button in the input title bar.
2. See recording state and elapsed duration without freezing the TUI.
3. Click Stop and see transcription state.
4. Receive the transcript at the current editor cursor, with safe Unicode and spacing behavior.
5. Edit the transcript and submit it normally with Enter.
6. Cancel recording/transcription and recover to a usable Idle state.
7. Receive actionable errors for missing permissions, missing configuration, device failures, and API failures.

All automated tests described above must pass without real audio hardware or cloud access.
