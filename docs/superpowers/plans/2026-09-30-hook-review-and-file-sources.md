# Plan: hook file sources + review + exit-2 contract

Spec: [2026-09-30-hook-review-and-file-sources-design.md](../specs/2026-09-30-hook-review-and-file-sources-design.md)

One `cargo` invocation at a time (AGENTS.md: parallel runs contend on `target/`).

1. **Sources** (`crates/tact_extensions/src/plugin/hooks.rs`)
   - `HookSource { label, dirs, hooks, origin }`, `HookOrigin { Plugin, UserFile, ProjectFile }`.
   - `collect_hook_sources(home, work_dir)` = plugins (existing `installed_hooks`) + `~/.tact/hooks.json` + `<workdir>/.tact/hooks.json`; missing or unparseable file is a reported warning, not a hard error for the project scope (same rule as a project `.mcp.json`).
   - Refactor `apply_plugin_hooks_with_home` into `apply_hook_sources(sources, trust, agent, work_dir)`.
2. **Review**
   - `hook_identity_hash(source, event, matcher, command)` (sha2).
   - `HookTrust` over `~/.tact/hooks-state.json`: `load`, `from_path`, `is_trusted`, `trust(entries)`, `forget_all`, `path()`.
   - `HookLoadReport { loaded, pending }` + `notice_lines()`.
   - `apply_plugin_hooks` keeps its signature for the two entry points and gains `apply_plugin_hooks_with_report` returning `(Agent, HookLoadReport)`; `interactive.rs` / `headless.rs` switch to it and emit the lines the way the MCP report does.
3. **Exit code 2**
   - `run_process` returns status + stderr; `HookOutput` gains the block reason.
   - Per-event mapping from the spec; JSON output still wins.
4. **CLI** (`crates/tact_ui/src/hooks_cli.rs`)
   - `tact-ui hooks list|trust|forget`, mirroring `mcp_cli.rs` (pure renderers + tests).
5. **TUI** (`/hooks`, `/hooks trust …`)
   - Registered like `/mcp list` / `/mcp auth`, idle-only, output through the same log helper.
6. **Tests**: `cargo test -p tact --lib plugin::hooks::`, `-p tact-ui --lib hooks_cli::`, `-p tui --lib handlers::`.
7. **Docs**: `book/09_chapter_hook_zh.md` (sources table, review section, exit-2 table, gaps), `config.example.toml` only if a config key appears (it does not), `book/26_chapter_issue_zh.md` entry.
