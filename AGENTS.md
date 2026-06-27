# tact — Terminal-first AI Coding Agent

A Rust workspace (~5 crates). Terminal-first AI coding agent.
MIT licensed. Two binaries: `tact` (headless) and `tact-tui` (interactive).

## Crates

| Crate | Path | Role |
|---|---|---|
| `tact_protocol` | `crates/protocol` | Wire types: `AgentUpdate`, `UserCommand`, `StepResult`, etc. |
| `tact` | `crates/tact` | Agent runtime, tool router, MCP, hooks, LLM adapters, compaction, session store. Binary entry. |
| `tui` | `crates/tui` | ratatui terminal UI, widgets, rendering pipeline |
| `tools` | `crates/tools` | `Sandbox` — secure file I/O + command execution |
| `tool_refactor_macros` | `crates/tool_refactor_macros` | `#[tool]` proc-macro for tool trait impls |

## Key directories

- `crates/tact/src/tool/` — 40+ built-in tools (bash, read_file, write_file, edit_file, apply_patch, subagent, web_search, etc.)
- `crates/tact/src/store/session_store/` — SQLite session/message/token-usage persistence (`tact.db`)
- `crates/tact/src/lib.rs` — `agent_loop()`, `execute_tool_call()`, `snapshot_dir()` (Project structure in system prompt)
- `crates/tui/src/render/` — log panel, cells (text, tool, code, thinking), popups, LogColumnRenderer
- `crates/tui/src/render/cells/tool.rs` — `ToolCell` rendering (title + meta + detail card)
- `crates/tui/src/widgets/tool_widget.rs` — `ToolWidget` / `ToolRenderOutput` layout builder
- `crates/tui/src/widgets/state/tool_state.rs` — concurrent `ActiveToolBlock` list, diff popup state
- `crates/tui/src/widgets/state/app/` — App state, agent update handler, construct, popups, search
- `docs/` — batch_tools_flow, compaction, log-panel-analysis, state_machines, tui_rendering, **tool_rendering**, token_usage_schema, parallel_tool_execution

## Build & test

```bash
cargo build
cargo test -p tui
cargo check
```

On Linux CI / fresh machines, install SQLite build deps first (`libsqlite3-dev`, `pkg-config`, `clang`, `libclang-dev`) — see `.github/workflows/rust.yml`.

Full architecture: see `ARCHITECTURE.md`.

## Context management

This file replaces the auto-generated project tree to keep the prompt prefix stable across edits. When the project structure changes (new files, renames), this file should be updated manually to reflect the new layout.

At runtime, `load_dynamic_context()` also injects a **Project structure** snapshot (cached per session, default 80 entries via `TACT_SNAPSHOT_MAX_ITEMS`). See `ARCHITECTURE.md` §5.5.
