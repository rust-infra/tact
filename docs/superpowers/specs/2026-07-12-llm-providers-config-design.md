# LLM Per-Provider Config + ProviderKind — Design

Date: 2026-07-12  
Status: Approved for implementation planning  
Related: keeps URL/model heuristics for endpoint flavor; no flat-field migration

## Goals

1. One config file can hold credentials for multiple providers; switching is
   `llm.provider = "…"` (or `--provider`), ready for a future TUI switcher.
2. Replace `ProviderInfo.provider: String` / `LlmSettings.provider: String`
   with a typed `ProviderKind` enum so `build_client`, defaults, and CLI/TOML
   parsing share one exhaustively matched identity.
3. Simplify resolve code: **no** backward compatibility for flat
   `[llm] model` / `api_key` / `base_url`.

## Non-goals

- TUI / terminal hot-switch UI (map shape only; runtime still picks one active
  provider at process start).
- Migrating or warning on old flat LLM fields.
- Removing `is_kimi()` / `is_deepseek()` URL/model heuristics (retained so
  `provider = openai` + Moonshot/DeepSeek-compatible base URL still works).

## TOML shape

```toml
[llm]
provider = "kimi"              # required: active ProviderKind
max_tokens = 8000              # optional global default
thinking_budget = 32000        # optional global default

[llm.providers.anthropic]
api_key = "sk-ant-..."
model = "claude-sonnet-4-20250514"
base_url = "https://api.anthropic.com"

[llm.providers.openai]
api_key = "sk-..."
model = "gpt-4o"
# base_url defaults to https://api.openai.com/v1

[llm.providers.deepseek]
api_key = "sk-..."
model = "deepseek-chat"

[llm.providers.kimi]
api_key = "sk-..."
model = "kimi-k2.5"
# max_tokens = 32000           # optional per-provider override
# thinking_budget = 64000
```

### Per-provider fields (`ProviderToml`)

| Field | Required | Notes |
|-------|----------|--------|
| `api_key` | yes | |
| `model` | yes | empty/whitespace rejected |
| `base_url` | no | falls back to `ProviderKind::default_base_url()`; Anthropic has no default → must set |
| `max_tokens` | no | overrides `[llm].max_tokens` when set |
| `thinking_budget` | no | overrides `[llm].thinking_budget` when set |

`llm.providers` is a `HashMap<String, ProviderToml>` (or map keyed after parse).
Keys must parse as `ProviderKind`. Unknown keys → config error listing valid names.

## ProviderKind

Defined in `tact_llm` (single source of truth):

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProviderKind {
    Anthropic,
    OpenAi,
    DeepSeek,
    Kimi,
}

impl ProviderKind {
    pub fn as_str(self) -> &'static str { /* anthropic|openai|deepseek|kimi */ }
    pub fn default_base_url(self) -> Option<&'static str> { /* … */ }
    pub fn is_openai_compatible(self) -> bool {
        !matches!(self, Self::Anthropic)
    }
}

impl std::str::FromStr for ProviderKind { /* … */ }
impl std::fmt::Display for ProviderKind { /* as_str */ }
```

- `ProviderInfo.provider: ProviderKind`
- `LlmSettings.provider: ProviderKind`
- `Default` for `ProviderInfo`: pick a sentinel only if needed for tests;
  production always sets an explicit kind after resolve.

`build_client` matches on `ProviderKind` (exhaustive). Adding a variant
forces updates at compile time.

Serde: TOML/CLI strings use `FromStr` / `as_str` (lowercase names above).
Do not invent alternate aliases (`moonshot` ≠ `kimi`).

## Resolve priority

For the **active** provider name `P` (CLI `--provider` else `llm.provider`):

1. Require `llm.providers` contains an entry whose key parses to `P`.
2. Credentials:
   - `api_key` / `model`: CLI flag if set, else entry field (required).
   - `base_url`: CLI → entry → `P.default_base_url()` → error if still missing.
3. Generation limits (into existing agent settings, not `LlmSettings`):
   - `max_tokens` / `thinking_budget`: CLI → entry → `[llm]` global →
     existing code defaults (including Kimi K2.x specials via heuristics).

Error examples:

- missing `llm.provider`
- `provider 'openai' not found in llm.providers (have: anthropic, kimi)`
- `unknown provider 'foo'; expected anthropic|openai|deepseek|kimi`
- `api_key not configured for provider 'kimi'`

## Runtime

Unchanged hot path: resolve still produces one flat active config
(`LlmSettings` + agent max_tokens/thinking_budget). Do **not** keep the full
providers map in `OnceLock` in this change; future TUI switch can revisit.

## Heuristics (kept)

`ProviderInfo::is_kimi` / crate `is_deepseek` (and related) continue to combine:

- `provider == ProviderKind::Kimi` / `DeepSeek`, **or**
- `base_url` / `model` substring checks (moonshot, kimi, deepseek, …)

So `provider = openai` + `base_url = https://api.moonshot.cn/v1` still
behaves as Kimi for thinking injection and balance polling. Document that
the **recommended** setup is a dedicated `[llm.providers.kimi]` entry.

`is_kimi_k2x` / `is_kimi_k27` remain model/URL refinements on top of
`is_kimi()`.

## CLI

Keep `--provider`, `--model`, `--api-key`, `--base-url`, `--max-tokens`,
`--thinking-budget`. `--provider` selects which map entry is active;
other flags override that entry’s fields.

## Docs / examples

- Rewrite `tact.example.toml` to the providers map shape; drop flat fields
  and the “old style openai + kimi URL” as the primary path (optional short
  note that heuristics still allow proxy-as-openai).
- Update `book/21_chapter_config.md` (and LLM chapter provider list /
  `ProviderInfo` field type).

## Tests

- Parse multi-provider TOML; resolve with `provider = kimi` picks kimi entry.
- CLI `--provider openai` switches active entry.
- Per-provider `max_tokens` overrides global; CLI overrides both.
- Unknown provider key / missing active entry / missing api_key|model errors.
- `ProviderKind::from_str` round-trip; `build_client` for each kind with
  empty base_url uses defaults (Anthropic without base_url fails).
- Existing Kimi-via-openai-URL heuristic tests still pass with
  `ProviderKind::OpenAi` + moonshot URL.

## Implementation sketch (files)

| Area | Change |
|------|--------|
| `tact_llm` | add `ProviderKind`; change `ProviderInfo`; update `build_client` / helpers / tests |
| `config/types.rs` | `LlmTomlConfig` with `provider`, globals, `providers: HashMap<…>`; drop flat credential fields |
| `config/resolve.rs` | map lookup + priority above; use `ProviderKind` |
| `config/cli.rs` | help text; parse provider via `FromStr` at resolve |
| `tact.example.toml`, book | docs |

## Risks

- Breaking change for all existing user configs (accepted).
- Heuristics + enum can disagree in edge cases (openai identity + kimi URL);
  documented and intentional until a later “strict mode”.
