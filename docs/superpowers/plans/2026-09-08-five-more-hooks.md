# Plan: five more lifecycle hooks — Codex-aligned

Spec: `docs/superpowers/specs/2026-09-08-five-more-hooks-design.md`

## Order of work (each step compiles)

1. **`compact/mod.rs`** — add `CompactTrigger { Auto | Manual | Recovery | Command }` with
   `as_str()`.
2. **`hook/mod.rs`** — add `SubagentStopContext`, `SubagentStopFn`, `StopFn`, `SessionEndFn`,
   `PreCompactFn`, `PostCompactFn` traits + `Hook::{Stop, SessionEnd, PreCompact, PostCompact}`
   variants.
3. **`agent/mod.rs`** — builder methods (`with_stop`, `with_session_end`, `with_pre_compact`,
   `with_post_compact`), `last_assistant_message()` helper, `dispatch_session_end_hooks`,
   `dispatch_stop_hooks`, private pre/post-compact dispatchers; wrap `compact_history` →
   `compact_history_with_trigger(trigger, focus)`; PreCompact veto + PostCompact-on-success.
   Update call sites: recovery → `Recovery`, manual → `Manual`.
4. **`tool/mod.rs`** — add `ToolContext.subagent_stop_hooks`.
5. **`tool/subagent.rs`** — dispatch SubagentStop in sync + async tails (helper fn).
6. **`plugin/hooks.rs`** — `HookEventKind` 5 variants + `as_str`/`parse`; `apply_plugin_hooks`
   arms for Stop/SessionEnd/PreCompact/PostCompact; `plugin_subagent_stop_hooks`.
7. **drivers** — `interactive.rs`/`headless.rs` stamp `subagent_stop_hooks`; `driver.rs`
   `/compact` → `Command` trigger, Stop continuation loop, SessionEnd dispatch; `headless.rs`
   SessionEnd before `shutdown_mcp`.
8. **docs** — `book/09_chapter_hook*.md` (EN+ZH) §2 table + §6 bullets; Ch 26 issue entries.

## Verification

- `cargo test -p tact hook::` and `-p tact plugin::hooks` (new + existing).
- `cargo test -p tact-ui driver::` (Stop continuation loop + SessionEnd wiring).
- `cargo fmt` + `cargo clippy` on touched crates.
- Doc bilingual alignment check (EN/ZH headings + tables).
