# DeepSeek Responses Protocol Support Design

- **Date:** 2026-08-02
- **Status:** Approved design; implementation not started
- **Scope:** Allow `protocol = "responses"` for the DeepSeek provider, reusing the existing generic Responses adapter

## 1. Goals and non-goals

### Goals

1. Let the DeepSeek provider use `protocol = "responses"` in `config.toml`.
2. Route DeepSeek + Responses to the existing generic `OpenAiResponsesAdapter` against the DeepSeek `base_url`.
3. Achieve parity with the OpenAI Responses path: ordinary chat, `context_management` compaction (`responses_compact_threshold`), `reasoning.effort` from `thinking_budget`, and Responses conversation-state continuation.
4. Keep DeepSeek + `chat_completions` (the default) unchanged.
5. Keep the config error for providers that must not use Responses (Kimi, Anthropic).

### Non-goals

1. No new adapter, no wire-format changes.
2. No porting of DeepSeek chat-completions-only extras (`user_id` KV-cache isolation, DeepSeek `thinking` mapping) into the Responses path.
3. No `reasoning_effort` config field for DeepSeek (the field stays OpenAI-only; `thinking_budget` already maps to effort automatically).
4. No Responses support for Kimi or Anthropic.
5. No chat-completions fallback when the DeepSeek `/responses` endpoint is incompatible.

## 2. Current implementation and gap

Two things currently block DeepSeek + Responses:

1. `crates/tact/src/config/resolve.rs` `resolve_llm()` bails with
   `protocol 'responses' is only supported for provider 'openai'` when
   `protocol == Responses && provider != ProviderKind::OpenAi`.
2. `crates/tact_llm/src/provider.rs` `ProviderInfo::build_client()` routes by
   provider kind first and only consults `protocol` for `ProviderKind::OpenAi`;
   DeepSeek always builds the Chat Completions `DeepSeekAdapter`.

The Responses adapter (`OpenAiResponsesAdapter`) is already generic: it is
constructed from `api_key`, `base_url`, `reasoning_effort`, and
`responses_compact_threshold`, and it already handles streaming normalization,
`context_management` compaction, and Responses conversation state. Nothing in
it assumes an OpenAI base URL.

The subagent path is unaffected: `resolve_subagent()` parses `protocol` without
an OpenAI-only check, so relaxing the main check covers subagents that reuse the
DeepSeek provider entry.

## 3. Architecture

### 3.1 Config validation (`crates/tact/src/config/resolve.rs`)

Relax the check in `resolve_llm()` so Responses is accepted for OpenAI and
DeepSeek:

```rust
if protocol == OpenAiProtocol::Responses
    && provider != ProviderKind::OpenAi
    && provider != ProviderKind::DeepSeek
{
    anyhow::bail!(
        "protocol 'responses' is only supported for provider 'openai' or 'deepseek'"
    );
}
```

The `reasoning_effort` check stays OpenAI-only (non-goal 3).

### 3.2 Client routing (`crates/tact_llm/src/provider.rs`)

`build_client()` DeepSeek branch gains a protocol match:

```rust
ProviderKind::DeepSeek => match self.protocol {
    OpenAiProtocol::ChatCompletions => self.build_deepseek(),
    OpenAiProtocol::Responses => self.build_openai_responses(),
},
```

`build_openai_responses()` already falls back to
`ProviderKind::DeepSeek::default_base_url()` (`https://api.deepseek.com`) when
`base_url` is empty, so no changes are needed there.

No other components change: `LlmProvider` variants, the agent layer's
`is_deepseek()` (keyed on provider kind/base_url/model), and DeepSeek balance
queries all continue to work because they do not depend on the client variant.

## 4. Data flow

```text
config.toml ([llm.providers.deepseek] protocol = "responses")
  -> resolve_llm() accepts
  -> init_provider() installs LlmSettings
  -> get_llm_client() -> build_client() -> LlmProvider::OpenAiResponses
  -> adapter hits {base_url}/responses
```

Each request follows the existing OpenAI Responses path: the adapter assembles
`/responses` input items (including the state baseline), `reasoning.effort`
derived from `thinking_budget`, and `context_management` when
`responses_compact_threshold` is configured; responses normalize to
`LlmResponse` plus `ProviderStateUpdate::OpenAiResponses` for continuation.
Compaction uses the same native Responses compaction as OpenAI.

## 5. Error handling

1. Kimi/Anthropic + `responses` still fail at config time with the updated
   message naming both allowed providers.
2. Protocol/network failures surface through the existing
   `LlmError::OpenAiResponses` mapping; no new error path.
3. If the DeepSeek `/responses` endpoint turns out to be incompatible, the error
   is surfaced as-is; there is no silent fallback to chat completions.

## 6. Testing

### Unit tests

- `crates/tact/src/config/resolve.rs`: update the existing
  `reject_responses_non_openai_provider` test to assert DeepSeek + Responses
  resolves, while Kimi and Anthropic + Responses are rejected.
- `crates/tact_llm/src/provider.rs`: add a case that DeepSeek +
  `OpenAiProtocol::Responses` builds `LlmProvider::OpenAiResponses` with the
  DeepSeek base URL; the existing default DeepSeek -> `DeepSeekAdapter` case
  stays.
- Existing Responses adapter tests remain valid unchanged.

### Live smoke test

After implementation, send one minimal `/responses` request to
`https://api.deepseek.com` with the configured API key to confirm endpoint
compatibility, and report the result. (User approved; consumes a small amount
of tokens.)

Per repo `AGENTS.md`: run cargo commands one at a time (single process against
the shared `target/` lock).

## 7. Documentation sync

Per `AGENTS.md` config-semantics rules:

- `config.example.toml`: add a `protocol` comment to the DeepSeek provider entry.
- `book/21_chapter_config.md` and `book/21_chapter_config_zh.md`: update the
  "Responses is valid only for OpenAI" wording.
- `book/26_chapter_issue.md` and `book/26_chapter_issue_zh.md`: append a
  newest-first `feature` entry with date, before/after behavior, and pointers to
  this spec and the config chapters.

## 8. Configuration restore

After implementation, restore `protocol = "responses"` under
`[llm.providers.deepseek]` in `~/.tact/config.toml` (backup already exists at
`/tmp/tact.config.toml.bak-20260802`) so `tact-ui --resume-last` runs with the
intended configuration.
