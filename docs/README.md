# Documentation map

Use this page to find the document that owns each topic. If a topic has both a
short reference page and a book chapter, the reference page answers lookup
questions and the book chapter explains the design and its history.

## Product and architecture

- [`README.md`](../README.md): install, configure, and run Tact.
- [`ARCHITECTURE.md`](../ARCHITECTURE.md): current crate boundaries, runtime
  flow, protocol, tools, permissions, and persistence.
- [`book/index_zh.html`](../book/index_zh.html): long-form product chapters.
- [`config.example.toml`](../config.example.toml): complete configuration
  template. It is the source of truth for supported keys.

## Runtime references

- [`compaction.md`](compaction.md): context compaction triggers and state.
- [`parallel_tool_execution.md`](parallel_tool_execution.md): resource
  conflicts and wave scheduling.
- [`state_machines.md`](state_machines.md): current UI, task, Goal, gate, and
  tool state enums.
- [`token_usage_schema.md`](token_usage_schema.md): `token_usages` schema and
  token accounting.
- [`agent_guidelines.md`](agent_guidelines.md): tool-use guidance for coding
  agents that work in this repository.

## Terminal UI

- [`tui_rendering.md`](tui_rendering.md): current TUI and `agent_tui_kit`
  rendering split.
- [`tool_rendering.md`](tool_rendering.md): tool cards, grouped tools, live
  output, and detail popups.
- [`book/23_chapter_tui_zh.html`](../book/23_chapter_tui_zh.html): TUI design
  and interaction history.
- [`book/25_chapter_protocol_zh.html`](../book/25_chapter_protocol_zh.html):
  `AgentUpdate`, `UserCommand`, and gate sequencing.

## Provider SDK

- [`tact-llm-public-api.md`](tact-llm-public-api.md): stable `tact_llm` surface.
- [`tact-llm-feature-edges.md`](tact-llm-feature-edges.md): optional feature
  combinations and dependency edges.
- [`tact-llm-redaction.md`](tact-llm-redaction.md): telemetry redaction rules.
- [`crates/tact_llm/README.md`](../crates/tact_llm/README.md): usage and
  release status.

## Desktop

- [`design/tact-desktop/README.md`](design/tact-desktop/README.md): prototype,
  current Tact-owned component map, and interaction inventory.
- [`design/tact-desktop/slash-command-audit.md`](design/tact-desktop/slash-command-audit.md):
  command-by-command desktop coverage.
- [`design/tact-desktop/visual-baseline.md`](design/tact-desktop/visual-baseline.md):
  reviewed layout baseline.
- [`design/tact-desktop/visual-verification.md`](design/tact-desktop/visual-verification.md):
  capture and live-window verification.

Delete a document when it is unreferenced, describes a removed implementation,
or repeats a current document without adding evidence. Keep decisions only in
the current architecture or book chapter that still depends on them.
