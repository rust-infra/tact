# Plan: `/hooks` slash command

Spec: [2026-09-30-tui-hooks-command-design.md](../specs/2026-09-30-tui-hooks-command-design.md)

1. **Protocol** (`crates/tact_protocol/src/agent.rs`)
   - `UserCommand::{HooksList, HooksTrust { all, source }, HooksForget}` with doc comments that
     state the `--all` / `--source` requirement.
2. **Kit bridge** (`crates/agent_tui_kit/src/bridge.rs`)
   - Three `Err(())` arms — the kit is generic and has no hook store.
3. **Driver** (`crates/tact_ui/src/driver.rs`)
   - `HooksList` → `survey_hooks` → `hooks_cli::render_hooks_listing` as `MdInfo`.
   - `HooksTrust` → `trust_hooks`; empty result and success get distinct messages, and success
     names the next-session caveat.
   - `HooksForget` → `forget_hook_trust`.
4. **TUI** (`crates/tui/src/handlers/hooks.rs`, `handlers/mod.rs`, `widgets/state/mod.rs`,
   `agent_tui_kit/src/i18n.rs`)
   - Prefix-based parser, idle gate on `list`, `usage()` helper; `command_needs_args` gains
     `hooks`; `PALETTE_COMMANDS` row; `hooks_usage` in EN and ZH.
5. **Tests**
   - `cargo test -p tui --lib handlers::hooks` (7), then the palette-sensitive suites
     (`render::popup_scene_tests`, `widgets::state::slash_command`) — the two that hardcoded the
     builtin count now derive it from `App::palette_commands()`.
   - `cargo test -p tact-ui --lib driver::tests::hooks` (2).
6. **Docs sync**
   - `book/09_chapter_hook_zh.md`: the Reviewing bullet names the TUI spelling and the idle
     gate; the code map gains the handler and driver rows.
   - `book/26_chapter_issue_zh.md`: newest-first entry.
