# `/model` Slash Command Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `/model` to switch the active provider’s model via SelectPopup; candidates from `[llm.providers.<name>].models`; optional persist to config `model`.

**Architecture:** Carry `models: Vec<String>` on resolved LLM settings. Make `tact_llm` provider store mutable (`OnceLock<RwLock<ProviderInfo>>`) with `set_model`. TUI `/model` opens existing SelectPopup with a `SelectKind` for model pick + persist Yes/No. Store loaded config path for TOML rewrite.

**Tech Stack:** Rust, existing SelectPopup, toml crate.

**Spec:** `docs/superpowers/specs/2026-07-12-model-switch-command-design.md`

---

### Task 1: Config `models` field + resolved settings + config path

**Files:** `crates/tact/src/config/types.rs`, `resolve.rs`, `load.rs`, `mod.rs`

- [ ] Add `models: Vec<String>` to `ProviderEntryToml` (serde default empty)
- [ ] Add `models: Vec<String>` to `LlmSettings`; fill from active entry in `resolve_llm`
- [ ] Change `load_toml_config` to return `(TactTomlConfig, Option<PathBuf>)` loaded path
- [ ] Store `config_path: Option<PathBuf>` on `ResolvedConfig`
- [ ] Tests: parse models; resolve copies models
- [ ] Commit

### Task 2: Mutable provider + `set_model`

**Files:** `crates/tact_llm/src/lib.rs`, `crates/tact/src/lib.rs`

- [ ] `ProviderInfo: Clone`
- [ ] `PROVIDER: OnceLock<RwLock<ProviderInfo>>`
- [ ] `get_provider() -> ProviderInfo` (clone under read lock)
- [ ] `set_model(model: String) -> Result<(), String>` reject empty/whitespace
- [ ] Fix call sites that assumed `&'static ProviderInfo` / `get_model() -> &'static str` → `String`
- [ ] Unit tests for set_model
- [ ] Commit

### Task 3: TUI `/model` + persist

**Files:** `crates/tui/...` handlers, state, i18n; `crates/tact` TOML update helper; `tact-ui` if wiring needed

- [ ] `PALETTE_COMMANDS` + `execute_palette_command("model")`
- [ ] `SelectKind::{Agent, ModelPick, PersistModel { model }}` on App
- [ ] Open SelectPopup with candidates from `tact::config::settings().llm.models` (ensure current model present)
- [ ] On confirm: `set_model` + sync `ResolvedConfig.llm.model` via new `config::update_llm_model`
- [ ] Second popup Save to config? → rewrite TOML `model` under active provider
- [ ] Empty models → system message
- [ ] Handler tests
- [ ] Commit

### Task 4: Docs + verify

- [ ] `tact.example.toml`, book/21 (and brief book/22)
- [ ] `cargo test -p tact_llm --lib`, `cargo test -p tact config::`, `cargo test -p tui`, clippy
- [ ] Commit
