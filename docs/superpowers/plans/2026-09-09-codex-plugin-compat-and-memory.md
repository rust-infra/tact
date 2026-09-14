# Plan: adopt Codex plugin/ecosystem exclusively — remove Claude-directory compatibility

Status: **implemented.** ~/.agents is kept; Claude-directory compat code is deleted
outright (not gated). All crates compile; skill / instruction-sources / prompt /
plugin / mcp / tui / tact-ui test suites pass. Bilingual docs + Ch 26 synced.

## Decision (confirmed)

Tact standardises on the **Codex plugin system**. Claude-directory compatibility
(`.claude-plugin`, `.claude/`, `CLAUDE.md`, Claude skills dir) is **removed**, keeping
only the `.tact/` + `.agents/` + codex plugin systems. Rationale (from discussion):
claude and codex plugins share the same command-hook kernel, and codex's layout is the
cleaner, actively-maintained spec — maintaining two manifest/discovery systems is not
worth it. agentmemory ships `.codex-plugin` (plus `.claude-plugin`), so codex-only
still consumes it.

### Scope boundaries (things NOT touched)
- **Claude/Anthropic as an LLM provider + `claude-*` model names** — that is core
  provider support (`config/resolve.rs`, `tact_llm`), unrelated to directory/plugin
  compat. Keep.
- **`CLAUDE_PLUGIN_ROOT` env var name** — Codex's own hook engine injects this exact
  var into hook subprocesses; hook scripts (`${CLAUDE_PLUGIN_ROOT}/…`) rely on it.
  Keep the name, only the source dirs change.
- **`~/.agents`** — independent ecosystem (agents/skills), keep.
- **`.tact/settings.json` permission settings** — already tact-owned, keep.

## What is removed (Claude-directory compat surfaces)

1. **Plugin manifest dir** `.claude-plugin/plugin.json` → replaced by
   `.codex-plugin/plugin.json` everywhere:
   - `plugin/install.rs` `read_compatibility_manifest`, install-time feature detection
     (`has_hooks`/`has_mcp`), manifest name-conflict check.
   - `plugin/hooks.rs` `installed_hooks` (manifest dir + hooks default path).
   - `mcp/mod.rs` `PluginLoader::scan`, `installed_plugin_mcp_servers`.
   - `plugin/model.rs` / `store.rs` schema + any `.claude-plugin` fixtures.
2. **`.claude/skills`** project skill discovery — drop from
   `consts.rs::skill_search_dirs`; `claude_dir()`/`claude_skills_dir()` removed.
   Skills search becomes `.tact/skills` → `~/.tact/skills` → `~/.agents/skills`.
3. **`CLAUDE.md` instruction injection** — drop `ClaudeMdUser`/`ClaudeMdProject`/
   `ClaudeMdSubdir` from `config/instruction_sources.rs` (and `claude_md*` parsing),
   the `home_claude_dir()` lookup in `agent/mod.rs:2016`, and the model-context
   shorthand. Keep `agents_md` (+ AGENTS.md) as the instruction source.
4. **`~/.claude` global dir helpers** — `consts.rs::home_claude_dir()`, its comment
   on memory location, and `claude_dir()`.
5. **plugin hooks default path** — confirm codex hooks discovery: default
   `hooks/hooks.json` shared; codex manifest may point at `hooks/hooks.codex.json`
   (agentmemory does) — honour both through the codex manifest field.

## Replacement: codex plugin layout as canonical

- Manifest `.codex-plugin/plugin.json`, fields incl. `name/version/description/
  skills/hooks/mcpServers/interface`. Map to tact's existing loader model.
- Marketplace/install/commit paths unchanged in structure — only the manifest-dir
  string and any codex marketplace conventions (e.g. `.agents/plugins/marketplace.json`)
  need mapping where applicable.
- No “claude|codex” runtime switch needed — codex-only means no toggle; default
  behaviour IS codex. (This also removes the need for the `PluginSystem` switch from
  the earlier draft.)

## Work items (each step compiles/tests)

1. `consts.rs` — remove `claude_dir()`/`home_claude_dir()`/`.claude` skill dir from
   `skill_search_dirs`; keep `.agents`.
2. `config/instruction_sources.rs` + `config/types.rs` + `config/resolve.rs` —
   drop `claude_md*` sources & shorthand; update defaults/tests.
3. `plugin/{install,model,store,hooks}.rs`, `mcp/mod.rs` — manifest dir
   `.claude-plugin` → `.codex-plugin`; support codex hooks file name; update all
   `.claude-plugin` fixtures/tests to `.codex-plugin`.
4. `agent/mod.rs`, `prompt/mod.rs`, `skill/mod.rs`, `tool/test_support.rs`,
   `memory/mod.rs`, drivers (`interactive.rs`/`headless.rs`), `tui/…/agent.rs` —
   drop remaining `.claude`/CLAUDE.md references; keep `CLAUDE_PLUGIN_ROOT`.
5. Docs — update `book/*` EN+ZH chapters that reference `.claude-plugin`, `.claude/`,
   `CLAUDE.md`, `claude_md*` (skill/memory/prompt/mcp/hook/agent-loop/compact/
   subagent/config + issue log). Keep bilingual pairs aligned.
6. Migration note — existing installed `.claude-plugin` caches/marketplace state under
   `~/.tact/plugins` invalid: add a one-time note (or detect-and-ignore) so stale
   claude caches don't error.

## Out of scope
- No memory-provider wiring (deferred follow-up).
- No codex desktop / `~/.codex/config.toml` parsing — only plugin manifest/hooks/mcp/
  skills layout compatibility.

## Verification
- `cargo test -p tact plugin::` and `-p tact mcp::` with `.codex-plugin` fixtures.
- `cargo test -p tact config::` / `instruction_sources` for removed claude_md paths.
- Default-path regression: a codex-layout plugin loads hooks + skills + mcp.
- `cargo fmt` + `cargo clippy` on touched crates; doc bilingual alignment.

## Open questions

- **`~/.agents`** — KEPT as a user-level skill root (independent ecosystem).
- **Deletion vs gating** — code is **deleted outright** (per confirmed decision); no
  runtime gating of removed Claude surfaces.
