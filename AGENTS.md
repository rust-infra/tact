# tact — Terminal-first AI Coding Agent

A Rust workspace (~5 crates). Terminal-first AI coding agent.
MIT licensed. Two binaries: `tact` (headless) and `tact-tui` (interactive).

## Crates

| Crate | Path | Role |
|---|---|---|
| `tact_core` | `crates/core` | Wire types: `AgentUpdate`, `UserCommand`, `StepResult`, etc. |
| `tact` | `crates/tact` | Agent runtime, tool router, MCP, hooks, LLM adapters, compaction. Binary entry. |
| `tui` | `crates/tui` | ratatui terminal UI, widgets, rendering pipeline |
| `tools` | `crates/tools` | `Sandbox` — secure file I/O + command execution |
| `tool_refactor_macros` | `crates/tool_refactor_macros` | `#[tool]` proc-macro for tool trait impls |

## Key directories

- `crates/tact/src/tool/` — 40+ built-in tools (bash, read_file, write_file, edit_file, apply_patch, subagent, web_search, etc.)
- `crates/tui/src/render/` — log panel, cells (text, tool, code, thinking), popups, LogColumnRenderer
- `crates/tui/src/widgets/state/app/` — App state, agent update handler, construct, popups, search
- `docs/` — batch_tools_flow, compaction, log-panel-analysis, state_machines, tui_rendering

## Build & test

```bash
cargo build
cargo test -p tui
cargo check
```

Full architecture: see `ARCHITECTURE.md`.

## Context management

This file replaces the auto-generated project tree to keep the prompt prefix stable across edits. When the project structure changes (new files, renames), this file should be updated manually to reflect the new layout.
