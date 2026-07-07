# Configuration

This chapter describes how Tact loads, merges, and installs runtime settings before any agent work begins. Configuration is the **bootstrap layer** that wires LLM credentials, agent limits, UI theme, tool keys, and permission mode into a process-global `ResolvedConfig`.

Implementation: `crates/tact/src/config/` (`mod.rs`, `cli.rs`, `load.rs`, `resolve.rs`, `types.rs`).

---

## 1. What Configuration Owns

| Concern | Resolved field | Primary consumers |
|---------|----------------|-------------------|
| LLM credentials | `ResolvedConfig::llm` → `tact_llm::init_provider` | [Ch 22 LLM](./22_chapter_llm.md), `Agent::stream_message` |
| Agent limits | `agent.*` | [Ch 5 Compact](./05_chapter_compact.md), [Ch 4 Prompt](./04_chapter_prompt.md), [Ch 17 Notify](./17_chapter_notify.md) |
| Permission mode string | `permission_mode: Option<String>` | Headless only — see [§6 Gaps](#6-current-gaps) |
| UI theme | `ui.theme` | [Ch 23 TUI](./23_chapter_tui.md) |
| Tool keys | `tools.brave_search_api_key` | `web_search` tool |
| Debug | `tokio_console` | `tact-ui` `main()` |

Every binary entry point should call `tact::config::init()` (or `init_config()`) **once** at startup.

---

## 2. Startup Flow

```mermaid
flowchart LR
    CLI[CliArgs::parse] --> Load[load_toml_config]
    Load --> Merge[resolve_config]
    Merge --> Install[config::install]
    Install --> LLM[tact_llm::init_provider]
    Install --> Global[SETTINGS OnceLock]
```

```rust
pub fn init() -> anyhow::Result<CliArgs> {
    init_config()
}

pub fn init_config() -> anyhow::Result<CliArgs> {
    let args = CliArgs::parse();
    let toml_cfg = load::load_toml_config(args.config.as_ref())?;

    if args.list_sessions {
        install_without_llm(resolve::resolve_non_llm_settings(&args, &toml_cfg));
        return Ok(args);
    }

    let resolved = resolve::resolve_config(&args, &toml_cfg)?;
    install(resolved);
    Ok(args)
}
```

`install()` does two things:

1. **`tact_llm::init_provider(config.llm.provider_info())`** — stores `ProviderInfo` for `get_llm_client()` ([Ch 22](./22_chapter_llm.md)).
2. **`SETTINGS.set(config)`** — makes `config::settings()` available for the rest of the process.

`install_without_llm()` skips provider init for commands like `--list-sessions` that never call the model.

---

## 3. Config Sources and Priority

### TOML search order

When `--config` is **not** passed, `load_toml_config` scans in order and uses the **first file that exists**:

| Order | Path |
|-------|------|
| 1 | `./.tact/config.toml` |
| 2 | `./tact.toml` |
| 3 | `~/.tact/config.toml` |

If none exist, an empty `TactTomlConfig::default()` is used.

Explicit `--config /path/to/file.toml` bypasses the search list.

### Merge rule: CLI > TOML > defaults

For each field, `resolve.rs` prefers CLI flags, then TOML values, then built-in defaults. Example for LLM:

```rust
let provider = args.provider.clone()
    .or(toml_cfg.llm.provider.clone())
    .ok_or_else(|| anyhow!("LLM provider not configured"))?;

let api_key = args.api_key.clone()
    .or(toml_cfg.llm.api_key.clone())
    .ok_or_else(|| anyhow!("api_key not configured"))?;
```

Required LLM fields without a default: **`provider`**, **`api_key`**, **`model`**.  
`base_url` falls back to provider-specific defaults (`openai`, `deepseek`, `kimi`).

---

## 4. TOML Schema

Top-level sections in `TactTomlConfig`:

```toml
[llm]
provider = "kimi"          # anthropic | openai | deepseek | kimi
model = "kimi-for-coding"
api_key = "sk-..."
base_url = "https://api.kimi.com/coding/v1"   # optional; default is https://api.moonshot.cn/v1
max_tokens = 32000
thinking_budget = 32000

[permission]
mode = "default"           # default | plan | auto

[agent]
context_limit_chars = 900000
notifications_enabled = true
snapshot_max_items = 80
micro_compact_enabled = true

[ui]
theme = "retro"

[tools]
brave_search_api_key = "bsk-..."
```

See `types.rs` for the full serde structs and unit tests with sample TOML.

---

## 5. Resolved Defaults

After merge, `resolve_config` applies these defaults when neither CLI nor TOML set a value:

| Setting | Default | Kimi K2.x override |
|---------|---------|-------------------|
| `max_tokens` | 8_000 | 32_000 |
| `thinking_budget` | 32_000 | — |
| `context_limit_chars` | 500_000 | 900_000 |
| `notifications_enabled` | `true` | — |
| `snapshot_max_items` | 80 | — |
| `micro_compact_enabled` | `true` | — |
| `ui.theme` | `"retro"` | — |

Kimi K2.x detection uses `provider_info.is_kimi_k2x()` at resolve time ([Ch 22](./22_chapter_llm.md)).

CLI-only overrides:

- `--no-notifications` forces notifications off.
- `--no-micro-compact` forces micro-compaction off.
- `--tokio-console` enables the tokio-console subscriber in `tact-ui`.

---

## 6. CLI Surface

`CliArgs` (clap) mirrors most TOML fields:

| Flag | Maps to |
|------|---------|
| `--provider`, `--model`, `--api-key`, `--base-url` | `[llm]` |
| `--max-tokens`, `--thinking-budget` | `[llm]` + agent request assembly |
| `-m` / `--permission-mode` | `[permission].mode` |
| `--context-limit-chars`, `--snapshot-max-items` | `[agent]` |
| `--notifications` / `--no-notifications` | `[agent].notifications_enabled` |
| `--theme` | `[ui].theme` |
| `--brave-search-api-key` | `[tools]` |
| `--session`, `--resume-last`, `--list-sessions` | session store (not in TOML). `--resume-last` and `--list-sessions` pass `list_sessions(Some(root_dir))` so only sessions for the current workdir appear. |
| `--config` | explicit TOML path |

Subcommand:

```bash
tact-ui headless "Summarize this repo"
```

Both entry points read `permission_mode` via `permission_mode_from_config()` in `crates/tact-ui/src/permission.rs`.

---

## 7. Accessing Settings at Runtime

```rust
use tact::config;

config::init()?;                          // once in main
let max = config::settings().agent.max_tokens;
let theme = config::settings().ui.theme.clone();
```

`settings()` panics if `init()` was not called — intentional fail-fast for miswired binaries.

Agent loop reads `context_limit_chars`, `max_tokens`, and `thinking_budget` from `settings()` when building each LLM request ([Ch 18](./18_chapter_agent_loop.md)).

---

## 8. Code Map

| File | Role |
|------|------|
| `config/mod.rs` | `init`, `install`, `settings`, public re-exports |
| `config/cli.rs` | `CliArgs`, `CliCommand::Headless` |
| `config/load.rs` | TOML discovery and parse |
| `config/resolve.rs` | CLI + TOML merge, Kimi-aware defaults |
| `config/types.rs` | `TactTomlConfig`, `ResolvedConfig`, section structs |
| `crates/tact-ui/src/main.rs` | Calls `config::init()` in `main()` |
| `crates/tact-ui/src/permission.rs` | Reads resolved `permission_mode` for both entry points |

---

## 9. Current Gaps

| Gap | Detail |
|-----|--------|
| **No env-var layer** | Only CLI and TOML; no `TACT_*` or provider env fallbacks in `resolve.rs` |
| **`anthropic` requires explicit `base_url`** | Unlike OpenAI-compatible providers, no default Anthropic URL in `default_base_url()` |
| **Secrets in plain TOML** | `api_key` stored as text; no keychain integration |
| **`list-sessions` stub LLM block** | `resolve_non_llm_settings` fills empty LLM fields — callers must not invoke `get_llm_client()` |

---

## Related Docs

- [LLM Providers](./22_chapter_llm.md) — what `install()` initializes
- [Agent Main Loop](./18_chapter_agent_loop.md) — runtime consumer of agent settings
- [Permission Model](./10_chapter_permission.md) — mode string vs TUI wiring
- [TUI](./23_chapter_tui.md) — theme and channel bootstrap
