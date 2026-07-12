# LLM Per-Provider Config + ProviderKind Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace stringly `provider` with `ProviderKind`, and restructure TOML so each provider’s credentials live under `[llm.providers.<name>]` with `llm.provider` selecting the active entry.

**Architecture:** `tact_llm::ProviderKind` is the single identity type (`FromStr` / `Display` / `default_base_url` / `is_openai_compatible`). Config parse drops flat `api_key`/`model`/`base_url` on `[llm]`; resolve looks up `providers[active]`, applies CLI overrides, then still emits one flat `LlmSettings` for the hot path. URL/model heuristics on `is_kimi` / `is_deepseek` stay.

**Tech Stack:** Rust, serde TOML, clap CLI, existing `tact` / `tact_llm` crates.

**Spec:** `docs/superpowers/specs/2026-07-12-llm-providers-config-design.md`

---

## File map

| File | Responsibility |
|------|----------------|
| Create `crates/tact_llm/src/provider_kind.rs` | `ProviderKind` enum + FromStr/Display/defaults |
| Modify `crates/tact_llm/src/lib.rs` | `mod provider_kind`; `pub use`; `ProviderInfo.provider: ProviderKind`; update `build_client` / `is_*` / tests |
| Modify `crates/tact_llm/src/openai.rs` | Test `ProviderInfo` literals use `ProviderKind` |
| Modify `crates/tact/src/config/types.rs` | `LlmTomlConfig` + `ProviderEntryToml`; `LlmSettings.provider: ProviderKind` |
| Modify `crates/tact/src/config/resolve.rs` | Map lookup + priority; per-entry max_tokens/thinking_budget |
| Modify `crates/tact/src/config/cli.rs` | Help text only (flags stay strings; parse in resolve) |
| Modify `tact.example.toml`, `book/21_chapter_config.md`, `book/22_chapter_llm.md` | Docs |

---

### Task 1: `ProviderKind` type (TDD)

**Files:**
- Create: `crates/tact_llm/src/provider_kind.rs`
- Modify: `crates/tact_llm/src/lib.rs` (add `mod provider_kind; pub use provider_kind::ProviderKind;`)

- [ ] **Step 1: Write failing unit tests in `provider_kind.rs`**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn from_str_round_trip() {
        for kind in [
            ProviderKind::Anthropic,
            ProviderKind::OpenAi,
            ProviderKind::DeepSeek,
            ProviderKind::Kimi,
        ] {
            assert_eq!(ProviderKind::from_str(kind.as_str()).unwrap(), kind);
            assert_eq!(kind.to_string(), kind.as_str());
        }
    }

    #[test]
    fn from_str_rejects_unknown() {
        assert!(ProviderKind::from_str("foo").is_err());
        assert!(ProviderKind::from_str("moonshot").is_err());
    }

    #[test]
    fn default_base_urls() {
        assert_eq!(
            ProviderKind::OpenAi.default_base_url(),
            Some("https://api.openai.com/v1")
        );
        assert_eq!(
            ProviderKind::DeepSeek.default_base_url(),
            Some("https://api.deepseek.com")
        );
        assert_eq!(
            ProviderKind::Kimi.default_base_url(),
            Some("https://api.moonshot.cn/v1")
        );
        assert_eq!(ProviderKind::Anthropic.default_base_url(), None);
    }

    #[test]
    fn openai_compatible_flags() {
        assert!(!ProviderKind::Anthropic.is_openai_compatible());
        assert!(ProviderKind::OpenAi.is_openai_compatible());
        assert!(ProviderKind::DeepSeek.is_openai_compatible());
        assert!(ProviderKind::Kimi.is_openai_compatible());
    }
}
```

- [ ] **Step 2: Run tests — expect compile/fail**

Run: `cargo test -p tact_llm provider_kind -- --nocapture`  
Expected: fail (module / type missing)

- [ ] **Step 3: Implement `provider_kind.rs`**

```rust
//! Typed LLM provider identity (config / CLI / runtime).

use std::fmt;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProviderKind {
    Anthropic,
    OpenAi,
    DeepSeek,
    Kimi,
}

impl ProviderKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic",
            Self::OpenAi => "openai",
            Self::DeepSeek => "deepseek",
            Self::Kimi => "kimi",
        }
    }

    pub fn default_base_url(self) -> Option<&'static str> {
        match self {
            Self::Anthropic => None,
            Self::OpenAi => Some("https://api.openai.com/v1"),
            Self::DeepSeek => Some("https://api.deepseek.com"),
            Self::Kimi => Some("https://api.moonshot.cn/v1"),
        }
    }

    pub fn is_openai_compatible(self) -> bool {
        !matches!(self, Self::Anthropic)
    }
}

impl FromStr for ProviderKind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "anthropic" => Ok(Self::Anthropic),
            "openai" => Ok(Self::OpenAi),
            "deepseek" => Ok(Self::DeepSeek),
            "kimi" => Ok(Self::Kimi),
            other => Err(format!(
                "unknown provider '{other}'; expected anthropic|openai|deepseek|kimi"
            )),
        }
    }
}

impl fmt::Display for ProviderKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}
```

Wire in `lib.rs`:

```rust
pub mod provider_kind;
pub use provider_kind::ProviderKind;
```

- [ ] **Step 4: Run tests — expect pass**

Run: `cargo test -p tact_llm provider_kind -- --nocapture`  
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/tact_llm/src/provider_kind.rs crates/tact_llm/src/lib.rs
git commit -m "$(cat <<'EOF'
feat(llm): add ProviderKind enum for typed provider identity

EOF
)"
```

---

### Task 2: `ProviderInfo` uses `ProviderKind`

**Files:**
- Modify: `crates/tact_llm/src/lib.rs` (`ProviderInfo`, `build_client`, `is_*`, tests)
- Modify: `crates/tact_llm/src/openai.rs` (test fixtures)

- [ ] **Step 1: Change `ProviderInfo` and fix compile errors driven by tests**

Replace:

```rust
#[derive(Debug, Default)]
pub struct ProviderInfo {
    pub api_key: String,
    pub base_url: String,
    pub model: String,
    pub provider: String,
}
```

with:

```rust
#[derive(Debug)]
pub struct ProviderInfo {
    pub api_key: String,
    pub base_url: String,
    pub model: String,
    pub provider: ProviderKind,
}

impl Default for ProviderInfo {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            base_url: String::new(),
            model: String::new(),
            provider: ProviderKind::OpenAi,
        }
    }
}
```

Update `build_client`:

```rust
pub fn build_client(&self) -> anyhow::Result<LlmProvider> {
    match self.provider {
        ProviderKind::Anthropic => self.build_anthropic(),
        ProviderKind::OpenAi | ProviderKind::DeepSeek | ProviderKind::Kimi => {
            self.build_openai_compatible()
        }
    }
}
```

Update `build_openai_compatible` empty-base_url branch to use `self.provider.default_base_url()` instead of string match. Error messages use `{self.provider}` (`Display`).

Update helpers:

```rust
pub fn is_kimi(&self) -> bool {
    self.provider == ProviderKind::Kimi
        || self.base_url.contains("moonshot")
        || self.base_url.contains("kimi")
        || self.model.contains("kimi")
}

pub fn is_account_query_supported(&self) -> bool {
    self.provider == ProviderKind::DeepSeek
        || self.base_url.contains("deepseek")
        || self.model.contains("deepseek")
        || self.is_kimi_balance_supported()
        || self.is_kimi_usage_supported()
}
```

Update crate-level `is_deepseek` the same way (`provider == ProviderKind::DeepSeek || …`).

Update all `provider_info("kimi", …)` test helpers to take `ProviderKind` or parse with `from_str`. Update `openai.rs` test `ProviderInfo { provider: "…".into(), … }` to `ProviderKind::…`.

- [ ] **Step 2: Run tact_llm tests**

Run: `cargo test -p tact_llm --lib`  
Expected: PASS (fix any remaining string comparisons)

- [ ] **Step 3: Commit**

```bash
git add crates/tact_llm/src/lib.rs crates/tact_llm/src/openai.rs
git commit -m "$(cat <<'EOF'
refactor(llm): store ProviderKind on ProviderInfo

EOF
)"
```

---

### Task 3: TOML types — providers map (breaking)

**Files:**
- Modify: `crates/tact/src/config/types.rs`
- Modify: tests inside `types.rs`

- [ ] **Step 1: Rewrite `LlmTomlConfig` and add `ProviderEntryToml`**

```rust
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
    pub providers: std::collections::HashMap<String, ProviderEntryToml>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ProviderEntryToml {
    pub api_key: Option<String>,
    pub model: Option<String>,
    pub base_url: Option<String>,
    pub max_tokens: Option<u32>,
    pub thinking_budget: Option<usize>,
}
```

Change `LlmSettings`:

```rust
pub struct LlmSettings {
    pub provider: tact_llm::ProviderKind,
    pub api_key: String,
    pub base_url: String,
    pub model: String,
}

impl LlmSettings {
    pub fn provider_info(&self) -> ProviderInfo {
        ProviderInfo {
            api_key: self.api_key.clone(),
            base_url: self.base_url.clone(),
            model: self.model.clone(),
            provider: self.provider,
        }
    }
}
```

Update `types.rs` unit tests to the new TOML shape, e.g.:

```toml
[llm]
provider = "openai"
max_tokens = 16000

[llm.providers.openai]
model = "gpt-4o"
api_key = "sk-test"
base_url = "https://proxy.example.com/v1"
```

- [ ] **Step 2: `cargo test -p tact config::types` — expect resolve tests to fail next; types tests should pass once updated**

Run: `cargo test -p tact config::types -- --nocapture`  
Expected: types tests PASS (resolve still broken until Task 4)

- [ ] **Step 3: Commit**

```bash
git add crates/tact/src/config/types.rs
git commit -m "$(cat <<'EOF'
feat(config): model llm TOML as providers map (breaking)

EOF
)"
```

---

### Task 4: Resolve logic + tests

**Files:**
- Modify: `crates/tact/src/config/resolve.rs`
- Modify: `crates/tact/src/config/cli.rs` (help strings)

- [ ] **Step 1: Rewrite failing resolve tests for map shape**

Replace existing resolve TOML fixtures with:

```toml
[llm]
provider = "kimi"
max_tokens = 8000

[llm.providers.kimi]
api_key = "mk-test"
model = "kimi-k2.5"

[llm.providers.openai]
api_key = "sk-test"
model = "gpt-4o"
```

Add cases:

1. `resolve_kimi_from_providers_map` — base_url defaults to moonshot.
2. `cli_provider_switches_entry` — args.provider = Some("openai") picks openai entry.
3. `per_provider_max_tokens_overrides_global` — entry `max_tokens = 32000` wins over global 8000.
4. `missing_provider_entry_errors` — provider=deepseek but only kimi configured.
5. `unknown_provider_name_errors` — provider=foo.
6. Keep `resolve_config_requires_model` adapted to missing model on entry.

- [ ] **Step 2: Run tests — expect FAIL**

Run: `cargo test -p tact config::resolve -- --nocapture`  
Expected: FAIL on new assertions / compile errors in `resolve_llm`

- [ ] **Step 3: Implement `resolve_llm`**

```rust
fn resolve_provider_kind(args: &CliArgs, toml_cfg: &TactTomlConfig) -> anyhow::Result<ProviderKind> {
    let raw = args
        .provider
        .clone()
        .or_else(|| toml_cfg.llm.provider.clone())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "LLM provider not configured. Set llm.provider in config.toml or pass --provider anthropic|openai|deepseek|kimi"
            )
        })?;
    raw.parse::<ProviderKind>()
        .map_err(anyhow::Error::msg)
}

fn resolve_llm(args: &CliArgs, toml_cfg: &TactTomlConfig) -> anyhow::Result<LlmSettings> {
    use tact_llm::ProviderKind;

    let provider = resolve_provider_kind(args, toml_cfg)?;

    // Validate all map keys are known kinds (clear errors early).
    for key in toml_cfg.llm.providers.keys() {
        key.parse::<ProviderKind>().map_err(anyhow::Error::msg)?;
    }

    let entry = toml_cfg
        .llm
        .providers
        .get(provider.as_str())
        .ok_or_else(|| {
            let have: Vec<_> = toml_cfg.llm.providers.keys().cloned().collect();
            anyhow::anyhow!(
                "provider '{provider}' not found in llm.providers (have: {})",
                if have.is_empty() {
                    "<none>".into()
                } else {
                    have.join(", ")
                }
            )
        })?;

    let api_key = args
        .api_key
        .clone()
        .or_else(|| entry.api_key.clone())
        .filter(|k| !k.is_empty())
        .ok_or_else(|| anyhow::anyhow!("api_key not configured for provider '{provider}'"))?;

    let base_url = args
        .base_url
        .clone()
        .or_else(|| entry.base_url.clone())
        .or_else(|| provider.default_base_url().map(str::to_string))
        .filter(|u| !u.is_empty())
        .ok_or_else(|| anyhow::anyhow!("base_url not configured for provider '{provider}'"))?;

    let model = args
        .model
        .clone()
        .or_else(|| entry.model.clone())
        .filter(|m| !m.trim().is_empty())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "model not configured for provider '{provider}'. Set llm.providers.{provider}.model or pass --model"
            )
        })?;

    Ok(LlmSettings {
        provider,
        api_key,
        base_url,
        model,
    })
}
```

Update `resolve_config` max_tokens / thinking_budget:

```rust
let entry = toml_cfg.llm.providers.get(llm.provider.as_str());

let max_tokens = args
    .max_tokens
    .or_else(|| entry.and_then(|e| e.max_tokens))
    .or(toml_cfg.llm.max_tokens)
    .unwrap_or_else(|| { /* existing kimi_k2x 32k / else 8k */ });

let thinking_budget = args
    .thinking_budget
    .or_else(|| entry.and_then(|e| e.thinking_budget))
    .or(toml_cfg.llm.thinking_budget)
    .unwrap_or(32_000);
```

Delete obsolete `default_base_url(&str)` helper in resolve (use `ProviderKind`).

Update `cli.rs` doc comment: mention providers map.

Fix `list_sessions` / empty-llm stubs that construct `LlmSettings` with `provider: String::new()` → use `ProviderKind::OpenAi` (or any) since those paths skip LLM.

- [ ] **Step 4: Run resolve tests — expect PASS**

Run: `cargo test -p tact config:: -- --nocapture`  
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/tact/src/config/resolve.rs crates/tact/src/config/cli.rs
git commit -m "$(cat <<'EOF'
feat(config): resolve active llm.providers entry by ProviderKind

EOF
)"
```

---

### Task 5: Workspace compile fix + docs

**Files:**
- Modify any remaining `provider: "…".to_string()` / `.provider ==` call sites under `crates/`
- Modify: `tact.example.toml`
- Modify: `book/21_chapter_config.md`, `book/22_chapter_llm.md`

- [ ] **Step 1: `cargo check --workspace` and fix breakages**

Run: `cargo check --workspace 2>&1`  
Fix any leftover `LlmSettings.provider` string uses (grep `llm.provider` / `ProviderInfo {`).

- [ ] **Step 2: Rewrite `tact.example.toml` LLM section** to the spec’s map shape (anthropic + openai + deepseek + kimi examples; active `provider` at top; no flat credentials).

- [ ] **Step 3: Update book chapters** — Ch21 resolution / example; Ch22 `ProviderInfo.provider: ProviderKind` and providers table.

- [ ] **Step 4: Full verification**

```bash
cargo test -p tact_llm --lib
cargo test -p tact config::
cargo clippy -p tact_llm -p tact --all-targets -- -D warnings
```

Expected: all green

- [ ] **Step 5: Commit**

```bash
git add tact.example.toml book/21_chapter_config.md book/22_chapter_llm.md
# plus any stray compile fixes
git commit -m "$(cat <<'EOF'
docs: document per-provider llm config and ProviderKind

EOF
)"
```

---

## Spec coverage check

| Spec item | Task |
|-----------|------|
| `ProviderKind` + FromStr/Display/defaults | 1 |
| `ProviderInfo.provider: ProviderKind`, build_client, heuristics kept | 2 |
| TOML providers map, drop flat fields | 3 |
| Resolve priority, errors, per-entry max_tokens | 4 |
| CLI help, example toml, book | 5 |
| Heuristic tests still green | 2 + 5 |
| No TUI switch / no flat migration | out of scope (non-goals) |

## Placeholder / consistency notes

- Map keys stay `String` in serde; validated via `ProviderKind::from_str` in resolve (matches spec).
- `LlmSettings.provider` and `ProviderInfo.provider` are both `ProviderKind` after Task 2–3.
- Do not add `xai` in this plan (not in current enum/spec list).
