# agent_tui_kit

A reusable **agent-TUI component kit**, extracted from Tact's TUI: thinking
cards, tool cards, the streaming markdown log, the popup family, the task/plan
panels, the input box, and the status/bottom bars — wired together by one
contract:

- **In:** a stream of [`AgentUpdate`](crates/protocol/src/agent.rs) events
  from the host's agent.
- **Out:** [`Command`] values sent through the host's [`AgentBridge`]
  implementation.

The kit depends only on `tact_protocol` (the types-only wire contract) and the
ratatui/crossterm family. It **never** depends on a concrete agent — `cargo
tree -p agent_tui_kit` shows no `tact` / `tact_llm`.

Design: `docs/superpowers/specs/2026-08-18-tui-component-library-design.md` ·
Plan: `docs/superpowers/plans/2026-08-18-tui-component-library.md`

## Quick start

Headless end-to-end example (no TTY needed):

```sh
cargo run -p agent_tui_kit --example mock_agent
```

It feeds a full `AgentUpdate` sequence (thinking → step started → tool card →
stream chunk → token usage → task complete) into a minimal host shell built
only from kit state, renders status bar / log panel / input box / bottom bar
through `TestBackend`, and prints the frame.

## Contract

| Piece | Type | Notes |
|-------|------|-------|
| In | [`protocol::AgentUpdate`] | re-exported generic subset of `tact_protocol` |
| Out | [`bridge::Command`] | generic 9-variant command enum (`SubmitTask`, `Cancel`, `Compact`, `QueryStats`, `QueryBackground`, `SetPermissionMode`, `SetThinkingBudget`, `SetReasoningEffort`, `SetModel`) |
| Extension out | [`bridge::ExtensionCommand`] | Tact-only commands (`QueryBalance`) |
| Host impl | [`bridge::AgentBridge`] | `send_command` + optional `BridgeExtension` |
| Extension events | [`bridge::ExtensionEvent`] | protocol-level events only (`AccountUpdate`); host-internal events (plugins, voice) stay in the app layer |

## Component inventory

**Render panels** (`render/`, all pure `&RenderCtx` — the host builds one
`RenderCtx` per frame from disjoint borrows; the only mutation path is the
`Vec<RenderCommand>` write channel):

- `bar` — top status bar + bottom stats bar
- `input` — multi-line input box, pending (queued-message) block, palette
  command line
- `log` — streaming markdown log (pure render; the host's `prepare_*` phase
  rebuilds the wrap/scroll caches)
- `log_column` — viewport-clipped `Renderable` compositor
- `popups` — thinking / diff / code / mermaid / system-prompt / subagent /
  history / select popups + chrome helpers (`PopupMouseSurface` returns mouse
  hit areas to the host)
- `task_panel` — sticky persistent-task strip
- `render_md` / `pulldown` / `mermaid_sequence` — markdown → ratatui lines,
  Mermaid `sequenceDiagram` renderer
- `cells` — text / separator / thinking / tool / code / markdown cells
- `util` / `renderable` — wrapping helpers, `Renderable` trait
- `slash_style` — `/skill-name` highlighting (builtin set injected by host)

**State models** (`state/`, plain data + methods, no `App`):

`LogCoordinator` · `LogScroll` · `ToolState` (`ToolBlock`,
`ActiveToolBlock`, `DiffPopup`, `SubagentPopup`) · `ThinkingState`
(`ThinkingBlock`, `ThinkingPopup`, `PopupTextSelection`) · `StreamState` ·
`StatusBarState` · `PlanPanel` · `TaskPanelState` · `MouseState` ·
`SelectPopup` · `AccountState` · `LogItem` / `LogItemKind` / `SystemMsgStyle`
· `InputMode` / `FocusedPanel` / `Status` / `SkillEntry` / `HistoryEntry` /
`CodeBlock` / `MermaidBlock` / popup states · `PendingMessage`

**Widgets** (`widgets/`): `ToolWidget` (builder → `ToolRenderOutput`),
`HelpWidget`, `PopupWidget`, `SelectPopupWidget`.

**Theme / i18n:** `Theme` + `ThemeName` (12 schemes), `Messages` by
`Language` (EN/ZH).

## Host responsibilities (the `crates/tui` app layer in Tact)

1. Own the shell state (`App`) and the handlers.
2. Build `RenderCtx` once per frame (`App::render_ctx` is the reference
   implementation).
3. Run the `prepare_*` phases that need host data (skill styling, git diff
   loading, layout-cache rebuilds, caret clamping) before calling the pure
   renderers.
4. Apply `PopupMouseSurface` hit areas and `RenderCommand`s after the frame.
5. Implement `AgentBridge` / `BridgeExtension` for outbound commands and
   host events.

## Development

```sh
cargo test -p agent_tui_kit --lib      # kit unit tests
cargo test -p tui --lib                # app-layer integration (kit + tui)
cargo run -p agent_tui_kit --example mock_agent
```

Every render unit paints its own background (no shadow/residue invariant);
moved render code is verbatim (`&App` → `&RenderCtx`), verified by unchanged
buffer-level tests at each phase gate.
