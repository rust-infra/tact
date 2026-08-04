# Google Cloud API-Key Voice Transcription Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a Google Cloud Speech-to-Text API-key voice provider for synchronous WAV transcription, limited to 60-second recordings, without changing the existing TUI voice workflow.

**Architecture:** Extend the existing `VoiceProvider` enum and resolver with provider-specific Google defaults and duration validation. Add a focused `GoogleTranscriber` implementation behind the existing `Transcriber` trait; it sends base64-encoded mono 16 kHz LINEAR16 WAV JSON to `speech:recognize` and parses concatenated result transcripts. Extend `spawn_worker` dispatch and synchronize configuration/docs/tests while preserving OpenAI and whisper.cpp behavior.

**Tech Stack:** Rust 2024, Tokio, async-trait, reqwest JSON client, serde/serde_json, base64, wiremock, TOML configuration, Markdown book/docs.

## Global Constraints

- Google authentication is API-key only through `voice.api_key`; do not add Service Account JSON, OAuth, ADC, or workload identity.
- Google uses synchronous `v1/speech:recognize`; do not add long-running, streaming, interim-result, segmentation, or language-detection support.
- Google default base URL is `https://speech.googleapis.com/v1`; default model is `latest_short`.
- Google `max_duration_secs` must resolve in `1..=60`; OpenAI and whisper.cpp retain `1..=600`.
- `voice.language` is sent unchanged as Google `languageCode`; examples use `zh-CN` and `en-US`.
- API keys must not appear in errors, logs, session history, or test failure output.
- Use bounded timeouts for async channel tests; never add unbounded `recv().await` tests.
- Do not run multiple Cargo commands concurrently against the same workspace/target directory.
- Keep English/Chinese documentation structurally aligned and update both in the same change.

---

## Task 1: Add Google configuration types, defaults, and provider-specific validation

**Files:**
- Modify: `crates/tact/src/config/types.rs` (`VoiceProvider`, `VoiceSettings` constants/defaults)
- Modify: `crates/tact/src/config/resolve.rs` (`resolve_voice` provider defaults and duration validation)
- Test: `crates/tact/src/config/types.rs` and the existing config resolver test module

**Interfaces:**
- Consumes: existing `VoiceTomlConfig` fields and `VoiceProvider` resolution.
- Produces: `VoiceProvider::Google`; `VoiceSettings::DEFAULT_GOOGLE_BASE_URL`, `DEFAULT_GOOGLE_MODEL`, and `DEFAULT_GOOGLE_MAX_DURATION_SECS`; resolved Google settings with a 60-second maximum.

- [ ] **Step 1: Write failing configuration tests**

Add tests that deserialize `provider = "google"`, resolve omitted Google base URL/model/duration to `https://speech.googleapis.com/v1`, `latest_short`, and `60`, accept `max_duration_secs = 60`, and reject `61` with an error mentioning Google and the 60-second limit. Also assert OpenAI and whisper.cpp still accept their existing duration range.

- [ ] **Step 2: Run the focused tests and verify failure**

Run:

```bash
cargo test -p tact config::types::tests::parse_google_voice_config -- --exact
cargo test -p tact config::resolve::tests::google_voice -- --nocapture
```

Expected: compilation/test failure because `VoiceProvider::Google` and Google defaults/validation do not yet exist.

- [ ] **Step 3: Implement the minimal configuration changes**

Add the enum variant and provider-specific constants. In `resolve_voice`, choose Google defaults separately from OpenAI and whisper.cpp, then validate the resolved duration with:

```rust
let max_allowed = match provider {
    VoiceProvider::Google => 60,
    VoiceProvider::OpenAi | VoiceProvider::WhisperCpp => 600,
};
if !(1..=max_allowed).contains(&max_duration_secs) {
    anyhow::bail!(
        "voice.max_duration_secs for {provider:?} must be between 1 and {max_allowed} (got {max_duration_secs})"
    );
}
```

Keep the existing `voice.api_key` optional at resolution time so enabled Google voice reports the missing-key error when recording starts.

- [ ] **Step 4: Run focused tests and verify success**

Run:

```bash
cargo test -p tact config::types::tests::parse_google_voice_config -- --exact
cargo test -p tact config::resolve::tests::google_voice -- --nocapture
```

Expected: all new and existing configuration tests pass.

- [ ] **Step 5: Commit the configuration slice**

```bash
git add crates/tact/src/config/types.rs crates/tact/src/config/resolve.rs
git commit -m "feat(voice): add Google provider configuration"
```

---

## Task 2: Implement and test the Google synchronous transcriber

**Files:**
- Modify: `crates/tact/src/voice/transcriber.rs`
- Modify: `crates/tact/Cargo.toml` only if an existing workspace dependency must be enabled (prefer existing `base64` workspace dependency)
- Test: `crates/tact/src/voice/transcriber.rs` test module

**Interfaces:**
- Consumes: `VoiceSettings`, `Transcriber`, `CancellationToken`, and existing WAV bytes.
- Produces: `GoogleTranscriber::new(settings)`, `Transcriber::transcribe(wav, cancel)`, and Google response parsing that returns a non-empty concatenated `String`.

- [ ] **Step 1: Write failing parser and wire-contract tests**

Add tests covering:

```rust
#[tokio::test]
async fn google_transcriber_sends_json_request_and_joins_results() {
    // mount POST /v1/speech:recognize, assert query key, content type,
    // config.encoding == "LINEAR16", config.sampleRateHertz == 16000,
    // config.languageCode/model, and audio.content decodes to the WAV bytes.
    // Return two results and assert "first second".
}

#[test]
fn parse_google_response_rejects_empty_results() { /* assert error */ }

#[tokio::test]
async fn google_transcriber_missing_key_does_not_request() { /* assert error */ }

#[tokio::test]
async fn google_transcriber_cancellation_aborts_request() { /* delayed wiremock + bounded timeout */ }
```

Use a test API key only in request matching; assert error strings do not contain it. Add malformed JSON and non-success status cases with bounded response snippets.

- [ ] **Step 2: Run the new tests and verify failure**

Run:

```bash
cargo test -p tact voice::transcriber::tests::google -- --nocapture
```

Expected: compilation failure because `GoogleTranscriber` and Google response parsing are absent.

- [ ] **Step 3: Implement Google request/response handling**

Add serializable request structs or `serde_json::json!` for:

```json
{
  "config": {
    "encoding": "LINEAR16",
    "sampleRateHertz": 16000,
    "languageCode": "<settings.language>",
    "model": "<settings.model>"
  },
  "audio": { "content": "<base64 WAV>" }
}
```

Build the endpoint as `{base_url}/speech:recognize`, append the API key with reqwest query parameters (`.query(&[("key", api_key)])`), set JSON content, and select request completion against `cancel.cancelled()`. Parse `results[].alternatives[0].transcript`, filter empty values, join with spaces, and return a bounded non-success error without copying the API key. Use the existing reqwest client timeout pattern.

- [ ] **Step 4: Run the Google transcriber tests and verify success**

Run:

```bash
cargo test -p tact voice::transcriber::tests::google -- --nocapture
cargo test -p tact voice::transcriber::tests -- --nocapture
```

Expected: all Google, OpenAI, and whisper.cpp transcriber tests pass.

- [ ] **Step 5: Commit the transcriber slice**

```bash
git add crates/tact/src/voice/transcriber.rs crates/tact/Cargo.toml Cargo.lock
 git commit -m "feat(voice): add Google Speech-to-Text transcriber"
```

Only include `Cargo.toml`/`Cargo.lock` if dependency declarations actually changed.

---

## Task 3: Wire Google into the voice worker and verify lifecycle compatibility

**Files:**
- Modify: `crates/tact/src/voice/mod.rs` (public re-export and `spawn_worker` dispatch)
- Test: `crates/tact/src/voice/mod.rs` provider dispatch/lifecycle tests if a focused test hook is needed

**Interfaces:**
- Consumes: `VoiceProvider::Google` and `GoogleTranscriber` from Tasks 1–2.
- Produces: `spawn_worker` selecting `GoogleTranscriber` for Google while leaving recorder, cancellation, generation, and TUI events unchanged.

- [ ] **Step 1: Add the provider-dispatch test or compile coverage**

Extend the provider dispatch coverage so all enum variants are handled. Keep lifecycle tests using `spawn_worker_with_components` and fake transcribers; add no unbounded channel receives.

- [ ] **Step 2: Run the focused worker tests and verify failure**

Run:

```bash
cargo test -p tact voice::tests -- --nocapture
```

Expected: failure or non-exhaustive match until the Google dispatch arm is added.

- [ ] **Step 3: Add the Google dispatch arm**

Re-export `GoogleTranscriber` alongside the existing transcribers and add:

```rust
VoiceProvider::Google => Arc::new(GoogleTranscriber::new(settings.clone())),
```

Do not modify worker cancellation, generation checks, or event semantics.

- [ ] **Step 4: Run worker and voice tests**

Run:

```bash
cargo test -p tact voice:: -- --nocapture
```

Expected: 32 existing voice tests plus new tests pass; the hardware recorder test remains ignored.

- [ ] **Step 5: Commit the worker integration**

```bash
git add crates/tact/src/voice/mod.rs
 git commit -m "feat(voice): route worker through Google transcriber"
```

---

## Task 4: Synchronize examples, README, bilingual config docs, and issue log

**Files:**
- Modify: `config.example.toml` voice section
- Modify: `README.md` voice configuration and feature text
- Modify: `book/21_chapter_config.md`
- Modify: `book/21_chapter_config_zh.md`
- Modify: `book/26_chapter_issue.md`
- Modify: `book/26_chapter_issue_zh.md`

**Interfaces:**
- Consumes: final configuration contract from Tasks 1–3.
- Produces: consistent user-facing documentation stating Google API-key auth, synchronous 60-second limit, defaults, language examples, and out-of-scope authentication/long-running behavior.

- [ ] **Step 1: Update the example and README**

Document `provider = "google"`, `https://speech.googleapis.com/v1`, `latest_short`, `zh-CN`/`en-US`, and `max_duration_secs = 60`. Explain that `voice.api_key` is a Google Cloud API key and that the Speech-to-Text API must be enabled.

- [ ] **Step 2: Update English and Chinese configuration chapters together**

Add Google to provider/default tables and describe the same synchronous-only, API-key-only contract in both languages. Keep heading levels, table rows, and section order structurally aligned.

- [ ] **Step 3: Add newest-first bilingual issue-log entries**

Add matching entries dated `2026-08-02`, type `feature`, describing the motivation, API-key synchronous design, 60-second limit, observable behavior, and pointers to the implementation/spec/chapters.

- [ ] **Step 4: Review documentation consistency**

Run:

```bash
rg -n 'whisper_cpp|provider = "google"|latest_short|speech.googleapis.com|60|Service Account|long-running' config.example.toml README.md book/21_chapter_config.md book/21_chapter_config_zh.md book/26_chapter_issue.md book/26_chapter_issue_zh.md
git diff --check
```

Expected: all public configuration references describe the same defaults and limits; no whitespace errors.

- [ ] **Step 5: Commit documentation**

```bash
git add config.example.toml README.md book/21_chapter_config.md book/21_chapter_config_zh.md book/26_chapter_issue.md book/26_chapter_issue_zh.md
git commit -m "docs(voice): document Google transcription provider"
```

If ignored-path rules affect `docs/superpowers`, do not alter `.gitignore`; only force-add the specific approved spec/plan files.

---

## Task 5: Full verification and release handoff

**Files:**
- Modify: none unless verification reveals a concrete defect

**Interfaces:**
- Consumes: completed implementation and synchronized documentation from Tasks 1–4.
- Produces: verified branch ready for review/PR.

- [ ] **Step 1: Run formatting and lint checks**

```bash
cargo fmt -- --check
cargo clippy --all-targets -- -D warnings
```

Expected: both commands exit 0.

- [ ] **Step 2: Run focused voice and configuration tests**

```bash
cargo test -p tact voice:: -- --nocapture
cargo test -p tact config:: -- --nocapture
```

Expected: all relevant tests pass; only hardware-dependent tests may be ignored.

- [ ] **Step 3: Run repository checks serially**

```bash
./scripts/check-rust.sh
```

Expected: format, clippy, and the repository test suite pass with 0 failures.

- [ ] **Step 4: Inspect final diff and branch state**

```bash
git diff origin/main...HEAD --check
git diff --stat origin/main...HEAD
git status -sb
git log --oneline origin/main..HEAD
```

Expected: only Google voice implementation, tests, docs, and the approved spec/plan are present; worktree is clean.

- [ ] **Step 5: Push branch for review**

```bash
git push -u origin wt/feature-google-voice-transcription
```

Expected: remote branch created with no uncommitted changes. Create a PR only after the user requests it.
