# Google Cloud API-Key Voice Transcription Design

- **Date:** 2026-08-02
- **Status:** Approved for specification; implementation pending review
- **Branch:** `wt/feature-google-voice-transcription`

## 1. Problem and goals

Tact's optional voice input currently supports OpenAI-compatible transcription and a
local `whisper.cpp` server. Users who already have a Google Cloud Speech-to-Text
API key should be able to use Google's synchronous Speech-to-Text endpoint without
changing the recorder or TUI workflow.

This change will:

- add a `google` voice provider using a Google Cloud API key;
- send short WAV recordings to the synchronous `speech:recognize` endpoint;
- preserve the existing cancellation, error, and transcript insertion behavior;
- expose provider defaults and examples in configuration and documentation; and
- keep the provider testable with a local HTTP mock.

## 2. Scope and non-goals

### In scope

- Google Cloud Speech-to-Text synchronous REST API (`v1/speech:recognize`);
- API-key authentication through the existing `voice.api_key` field;
- base URL override for tests and compatible gateways;
- BCP-47 language codes such as `zh-CN` and `en-US`;
- configurable Google model with a default of `latest_short`;
- Google-specific recording limit of 1–60 seconds;
- unit and wire-level tests; and
- synchronized English/Chinese configuration and issue-log documentation.

### Out of scope

- Service Account JSON, OAuth, ADC, or workload identity;
- asynchronous `longrunningrecognize` jobs;
- automatic audio segmentation or stitching;
- streaming recognition or interim results;
- automatic language detection; and
- changes to microphone capture, TUI state transitions, or transcript insertion.

## 3. Configuration contract

`VoiceProvider` gains a `Google` variant, deserialized from `provider = "google"`.
Existing fields retain their meaning:

```toml
[voice]
enabled = true
provider = "google"
api_key = "AIza..."
base_url = "https://speech.googleapis.com/v1"
model = "latest_short"
language = "zh-CN"
max_duration_secs = 60
```

Resolved defaults are provider-specific:

| Setting | Google default | Google validation |
|---|---|---|
| `base_url` | `https://speech.googleapis.com/v1` | non-empty after resolution |
| `model` | `latest_short` | non-empty after resolution |
| `language` | existing default remains `zh` when omitted | sent unchanged as `languageCode` |
| `max_duration_secs` | `60` | `1..=60` |
| `api_key` | none | required when transcription starts |

OpenAI and `whisper_cpp` keep their existing defaults and `1..=600` duration
range. The resolver must report a provider-specific validation error when Google
is configured above 60 seconds. `enabled = true` without an API key remains a
runtime click/start error, consistent with the existing OpenAI behavior; secrets
must not be included in errors, logs, or session data.

## 4. Runtime architecture and request flow

The existing `Transcriber` trait remains the provider boundary:

```text
TUI voice control
  -> VoiceWorkerHandle
  -> recorder -> WAV (mono, 16 kHz, LINEAR16)
  -> dyn Transcriber
       -> GoogleTranscriber
  -> VoiceEvent::Transcript
  -> TUI input insertion
```

`spawn_worker` selects `GoogleTranscriber` for `VoiceProvider::Google`. No worker
or TUI state-machine changes are required beyond the provider selection and
provider-specific duration resolved in configuration.

`GoogleTranscriber::transcribe` will:

1. require a non-empty `voice.api_key`;
2. build `POST {base_url}/speech:recognize?key={api_key}` using the configured
   base URL with trailing slashes removed;
3. send `Content-Type: application/json` with a body equivalent to:

   ```json
   {
     "config": {
       "encoding": "LINEAR16",
       "sampleRateHertz": 16000,
       "languageCode": "zh-CN",
       "model": "latest_short"
     },
     "audio": {
       "content": "<base64 WAV bytes>"
     }
   }
   ```

4. use the existing cancellation token to abort waiting for the HTTP response;
5. reject non-success statuses with a bounded response snippet that excludes the
   API key; and
6. parse and concatenate each `results[].alternatives[0].transcript`, trim the
   final string, and reject responses with no non-empty transcript.

The API key is placed in the query string because that is Google's documented API
key authentication form. It must never be copied into an error message or emitted
by application logging. Test assertions should verify that failures do not expose
it.

## 5. Error handling and compatibility

- Missing API key: return `[voice].api_key is not configured` before making a request.
- HTTP failure: return status plus a bounded body snippet, without credentials.
- Malformed JSON: return a stable parse error.
- Missing/empty `results` or alternatives: return a missing non-empty transcript
  error rather than emitting an empty TUI insertion.
- Cancellation: follow existing `tokio::select!` behavior and return a cancellation
  error; the worker's generation handling prevents late results from being emitted.
- Google API errors may contain the key in an echoed URL only if a gateway returns
  it; response snippets should be sanitized or bounded without including request
  URLs, and tests must cover the normal error path.

OpenAI and `whisper_cpp` wire formats and behavior remain unchanged. The Google
provider is additive and does not change the default provider (`openai`).

## 6. Testing strategy

Add focused tests alongside the existing voice tests:

1. **Configuration**
   - deserialize `provider = "google"`;
   - resolve Google defaults (`base_url`, `latest_short`, 60 seconds);
   - accept a custom language/model and a duration up to 60 seconds;
   - reject a Google duration above 60 seconds; and
   - keep existing provider duration behavior intact.

2. **Google transcriber wire contract** using `wiremock`
   - assert POST path `/v1/speech:recognize` and query `key`;
   - assert JSON encoding, sample rate, language, model, and base64 audio;
   - parse multiple results and concatenate transcripts;
   - reject HTTP errors, malformed JSON, and empty results without leaking the key;
   - reject missing API key without making a request; and
   - verify cancellation of a delayed response.

3. **Provider selection**
   - ensure `spawn_worker` constructs the Google implementation through the
     provider dispatch path; worker lifecycle coverage remains shared by the
     existing fake transcriber tests.

Tests must use bounded waits/timeouts where channels are involved, following the
repository's voice test conventions.

## 7. Documentation and release notes

Update all user-facing configuration references together:

- `config.example.toml`;
- `README.md`;
- `book/21_chapter_config.md`;
- `book/21_chapter_config_zh.md`; and
- newest-first entries in `book/26_chapter_issue.md` and
  `book/26_chapter_issue_zh.md`.

Documentation will state that Google API-key mode is synchronous and limited to
60 seconds, does not support Service Accounts or long-running recognition, uses
`zh-CN`/`en-US`-style language codes, and requires the Cloud Speech-to-Text API to
be enabled for the project.

## 8. Acceptance criteria

The feature is complete when:

- a user can configure `provider = "google"` with an API key and obtain a transcript;
- the default Google request targets `speech.googleapis.com/v1/speech:recognize`;
- request cancellation and errors follow existing voice behavior;
- API keys are not exposed in diagnostics or tests;
- Google recordings cannot exceed 60 seconds through configuration resolution;
- existing OpenAI and `whisper_cpp` voice tests continue to pass; and
- code, configuration examples, bilingual docs, and issue logs describe the same
  contract.
