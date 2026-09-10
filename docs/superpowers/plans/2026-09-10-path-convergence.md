# Plan: Path Convergence (MCP config, plugin state, skill roots)

Date: 2026-09-10
Status: implemented

Spec: `docs/superpowers/specs/2026-09-10-path-convergence-design.md`

## Work Items

Work items are ordered so each stage compiles and tests green on its own.
Stages 1-2 are additive and carry no migration risk; stages 3-4 change
on-disk contracts and are gated behind their own tests.

### Stage 1 — MCP config file (additive)

1. Add `McpConfigFile`: parses `{"mcpServers": {...}}` into a map of server
   name to config, reusing the existing entry shape.
2. Add path accessors in `crates/tact/src/consts.rs`:
   `TactPath::mcp_config_path()` → `<workdir>/.tact/mcp.json`,
   `TactPath::home_mcp_config_path()` → `~/.tact/mcp.json`.
3. Add `collect_sourced_servers` (layered read) and `resolve_servers`
   (override resolution) and rewite `load_mcp_router` on top of them. Plain
   map key becomes the server name.
4. Give project scope a single filename (`<workdir>/.tact/mcp.json`); do
   **not** read a cwd `.mcp.json` or a cwd `.codex-plugin/plugin.json`.
5. Delete `PluginLoader` — the cwd manifest scan was its only caller.
   Keep `PluginManifest`, still read from installed plugin bundles.
6. Tests: parse, missing-file, malformed-file hard error, project-over-user
   override, key merge, tool-name shape, cwd `.mcp.json` **not** read.

### Stage 2 — Failure visibility (additive)

7. Introduce `McpLoadReport` and return it from
   `load_mcp_router_with_report`.
8. Populate `failures` on `McpClient::try_new` error, `shadowed` on override,
   `skipped_remote` for remote / command-less entries, `connected` on success.
9. Thread the report to `build_agent_for_interactive` / headless startup and
   emit one startup notice when non-empty; print nothing when clean.
10. Tests: failure recorded and startup not aborted; shadowed recorded;
    clean load yields an empty report.

### Stage 3 — Plugin state directory (contract change, migration)

11. Add `home` and `state` fields to `PluginHome`; drop the positional
    `plugin_home_dir` derivation in favour of `home`.
12. Point `PluginStore` reads/writes at `state/` with a legacy fallback read.
13. One-time migration: read legacy, write new, leave legacy in place.
14. Tests: legacy-only home resolves; migration writes `state/`; new location
    wins when both exist; `PluginHome` exposes explicit `home`/`state`;
    writes never target the legacy path.

### Stage 4 — Skill root precedence (behaviour change)

15. Reorder `TactPath::skill_search_dirs` to
    `[~/.agents/skills, ~/.tact/skills, <workdir>/.tact/skills]`.
16. Update the doc comment (it documented the inverted order).
17. Test: relative positions of the three roots, so Tact outranks the Codex
    compatibility root and the project outranks both.

### Stage 5 — Docs

18. `book/08_chapter_mcp.md` + `_zh.md`: `mcp.json`, source list, naming,
    failure reporting, gaps table. Keep the bilingual pair aligned.
19. `book/21_chapter_config*.md`: plugin state file location, MCP scope rule.
20. `book/26_chapter_issue.md` + `_zh.md`: one dated entry covering the
    user-visible changes (MCP config file, silent failures now reported,
    skill precedence flip, cwd manifest no longer read).
21. Correct the stale Ch 8 claim about cwd `.codex-plugin/plugin.json` and the
    `.mcp.json` asymmetry.

## Verification

- `cargo test -p tact mcp::` (Stages 1-2)
- `cargo test -p tact plugin::` (Stage 3)
- `cargo test -p tact skill::` + `cargo test -p tact consts::` (Stage 4)
- `cargo check --workspace` after the report threading (Stages 2, 5)
- Full suite: `cargo test -p tact --lib`; `cargo clippy -p tact --all-targets`
- Manual: create `~/.tact/mcp.json` with the basic-memory server, start the
  TUI, confirm `mcp__basic-memory__*` tools are advertised and a deliberately
  broken entry produces a visible notice.
- Manual: with a legacy `~/.tact/plugins/installed.json`, confirm
  `tact-ui plugin list` still works and `state/installed.json` is written.

## Not in this plan

- Remote MCP transport.
- `tools/list_changed` handling.
- Per-tool MCP permission granularity.
- Removing installed-plugin MCP support (a plugin is a distributable bundle,
  not a config convention).
- Removing the Codex/Claude *marketplace* discovery delivered separately in
  `2026-09-10-codex-marketplace-design.md`.
