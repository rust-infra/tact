# `/model` Slash Command — Design

Date: 2026-07-12  
Status: Approved for implementation planning  
Inspired by: OpenAI Codex `/model` picker (session switch + optional persist); catalog is config-driven like Codex `model_catalog_json`, not a remote fetch in v1.

## Goals

1. Add `/model` to the TUI slash/palette command list so the user can switch the
   **active model string** for the **current provider** without restarting.
2. Candidate list comes from a new optional `models` array on
   `[llm.providers.<name>]`.
3. Selection applies immediately to subsequent LLM turns in this process.
4. Optionally persist by writing the chosen value back to the active provider’s
   `model` field in the loaded config file.

## Non-goals

- Switching `provider` / api_key / base_url (separate future command).
- Fetching `/v1/models` from the API (Codex OSS path); v1 is config-only.
- Codex-style searchable dedicated picker or reasoning-effort sub-picker.
- Persisting changes to the `models` array itself.

## Config shape

```toml
[llm]
provider = "kimi"

[llm.providers.kimi]
api_key = "sk-..."
model = "kimi-for-coding"   # default / last persisted active model
models = ["kimi-for-coding", "kimi-k2.5"]
```

| Field | Required | Notes |
|-------|----------|--------|
| `model` | yes (unchanged) | Default at startup; updated on persist |
| `models` | no | Picker candidates; empty/absent → `/model` errors with a clear hint |

Resolve still picks a single active `LlmSettings` from `llm.provider` + map entry.
`models` is carried into runtime settings (see below) for the TUI picker only;
it does not affect `build_client` beyond the chosen `model` string.

## Runtime model mutation

Today `tact_llm::PROVIDER` is a `OnceLock<ProviderInfo>`, which blocks mid-session
updates. Change to an interior-mutable holder, e.g. `RwLock<Option<ProviderInfo>>`
or `OnceLock<RwLock<ProviderInfo>>`, preserving:

- `init_provider(info)` — set once at install (still panic / err on double init of the lock shell if desired)
- `get_provider() -> ProviderInfo` or clone snapshot for callers that need owned data
- **New** `set_model(model: impl Into<String>) -> Result<(), …>` — updates only the
  `model` field; rejects empty/whitespace

Also update `tact::config` `ResolvedConfig.llm.model` under the existing
`SETTINGS: RwLock` so status/help that read config stay consistent.

Agent loop and adapters already read model via `get_provider()` /
`CreateMessageParams.model` built from settings at request time — after
`set_model`, the **next** turn uses the new id. In-flight streams are unchanged.

## UX

1. User runs `/model` (slash popup or `:` palette — same `execute_palette_command`
   path as `/theme`, `/balance`).
2. TUI opens existing `SelectPopup` (`InputMode::Select`):
   - Prompt: e.g. `Select model (kimi)` showing active provider kind.
   - Options: `models` list; if current `model` is missing from the list, prepend
     it and mark as current (display can use a `*` suffix or rely on pre-select).
   - Pre-select the index of the current model.
3. Enter → `set_model` → system log line confirming the new model → clear input.
4. Persist prompt (second `SelectPopup` or Yes/No options):
   - `Save to config?` → Yes / No
   - Yes: rewrite `model = "…"` under the active `[llm.providers.<name>]` in the
     config path that was loaded at startup (same file discovery as today).
   - No: session-only; restart restores toml `model`.
5. Esc cancels without changes.
6. If `models` is empty/absent: do not open picker; system message instructing
   to add `models = [...]` under the active provider.

Busy agent: allow switch; only subsequent turns use the new model (document in
help text).

## Persistence details

- Prefer in-place TOML edit of the known config file path (store path on
  `ResolvedConfig` or install-time side channel if not already available).
- Only update the active provider entry’s `model` key; do not reorder other keys
  more than the chosen TOML library requires.
- If no writable config path (e.g. CLI-only overrides with no file): skip persist
  prompt or show “no config file to update”.

## Command registration

- Add `("model", "Switch model for current provider")` to `PALETTE_COMMANDS`.
- Handle `"model"` in `execute_palette_command`.
- i18n / help strings if the help panel lists slash commands explicitly.

## Docs

- `tact.example.toml`: show `models = [...]` on at least one provider example.
- `book/21_chapter_config.md`: document the field and `/model` behavior.
- Optional one-line note in `book/22_chapter_llm.md` that runtime model can change.

## Tests

- Config parse: `ProviderEntryToml.models` round-trip.
- `set_model` updates `get_provider().model`; empty rejected.
- `/model` with empty candidates → handled message, no panic.
- `/model` picker confirm path (unit/handler test) calls set_model (mock or
  re-init test provider).
- Persist: optional unit test that a sample TOML fragment’s `model` key is
  updated for the active provider section.

## Implementation sketch

| Area | Change |
|------|--------|
| `config/types.rs` | `ProviderEntryToml.models: Vec<String>` (default empty) |
| `config/resolve.rs` | Pass `models` into resolved LLM settings (new field) |
| `tact_llm` | Mutable provider store + `set_model` |
| `tui` handlers + `PALETTE_COMMANDS` | `/model` → SelectPopup → set_model → optional persist |
| `tact` / `tact-ui` | Config path + TOML write helper for persist |
| example + book | Docs |

## Risks

- `OnceLock` → `RwLock` must not hold the lock across `.await` in hot paths
  (`get_provider` should clone or copy needed fields quickly).
- TOML rewrite can lose comments/formatting depending on library; prefer a
  minimal edit strategy or document that persist may reformat the file.
- Heuristics (`is_kimi_k2x`, etc.) depend on model string — switching can change
  defaults for max_tokens/context on **next** resolve only unless those are
  recomputed; v1 only changes the model id used in API requests, not
  re-running full `resolve_config` heuristics (call out in help if max_tokens
  stays at process-start values).
