# TUI rendering

The terminal UI uses two crates:

- `crates/tui`: application state, key and mouse handlers, host integration,
  layout, and app-specific prepare phases.
- `crates/agent_tui_kit`: reusable render functions, state models, widgets,
  themes, and localization.

The split lets another host reuse the drawing code without depending on the
full Tact product.

## Frame flow

`run_tui()` in `crates/tui/src/lib.rs` owns the event loop:

1. Drain `AgentUpdate`, account, plugin, and voice events.
2. Flush queued prompts when the agent becomes idle.
3. Repaint only when state is dirty, a timed state needs cleanup, or an
   animation is active.
4. Split the terminal into status bar, main area, input, and two-row bottom
   bar.
5. Run app-specific prepare phases, then call the pure kit renderers.
6. Apply popup mouse surfaces and render commands.

The current layout is:

```text
status bar       1 row
main area        remaining rows
input            wrapped input rows + border + queued-message rows
bottom bar       2 rows
```

The main area contains one of these surfaces:

- the Log;
- the task history panel;
- the help panel;
- the subagent transcript view.

When a sticky Goal, task, subagent, or background strip is visible, the Log
occupies the upper part of the main area and the strip closes the lower part.
The scroll-back pill is drawn on the bottom border of the panel that closes the
Log box.

## RenderCtx

Pure renderers accept `agent_tui_kit::render::ctx::RenderCtx`. The app builds
one context per frame from disjoint reads of `App`.

```text
App
  -> RenderCtx
  -> pure render functions
  -> Vec<RenderCommand> and popup mouse surfaces
  -> App applies the commands after the frame
```

Render code cannot mutate `App`. A renderer that needs to queue a message,
repaint, or record a click area returns a typed command or hit area.

## Prepare phases

Some work needs mutable state or host data before drawing:

- `crates/tui/src/render/log.rs` rebuilds logical-to-physical row maps, wrapped
  visual caches, viewport clipping, and the live stats row reserve.
- `crates/tui/src/render/input.rs` clamps and scrolls the caret into view.
- popup modules may load a diff, prepare Markdown, or cache rendered selection
  text.

The prepare phase and the pure renderer consume the same frame geometry. This
keeps scroll, mouse hit testing, and painted rows aligned.

## Module map

```text
crates/tui/src/
  lib.rs                    event loop and frame composition
  handlers/                 keyboard and mouse behavior by mode
  render/
    layout.rs               main-area selection and sticky split
    log.rs                  mutable log prepare phase + pure kit call
    input.rs                input prepare phase
    bar.rs                  status and bottom-bar app wrappers
    task_panel.rs           sticky strip placement
    popups/                 app-specific popup entry points
  widgets/state/app/        App state and update handlers

crates/agent_tui_kit/src/
  render/
    bar.rs                  status and bottom bars
    input.rs                input rendering and wrapping
    log.rs                  pure Log renderer
    log_column.rs           viewport-clipped Renderable compositor
    sticky_host.rs          sticky strip rendering
    stats_line.rs           live and frozen task stats rows
    cells/                  text, Markdown, code, thinking, and tool cells
    popups/                 reusable popup renderers
  state/                    reusable UI state models
  widgets/                  shared widgets and tool-card builder
  components/               host-facing component registry
  theme.rs                  built-in themes
  i18n.rs                   English and Chinese messages
```

## Log rendering

The Log has four stages:

```text
physical messages
  -> visible logical rows
  -> wrapped visual lines
  -> viewport slice
  -> cells and overlays
```

`LogScroll` stores the authoritative visual top. Logical offsets are derived
for hit testing and popup selection. A bottom-pinned viewport follows new rows;
a user scroll position remains fixed until the user returns to the tail.

Completed thinking rows, tool cards, code blocks, and Mermaid diagrams use
placeholder row ranges so the Log height and mouse mapping stay stable while
content changes.

## Popups

Popups fall into two groups:

- `crates/tui/src/render/popups/`: entry points that need full `App` state.
- `crates/agent_tui_kit/src/render/popups/`: reusable renderers that need only
  `RenderCtx`.

Each active popup records its mouse surface. Click outside, `Esc`, scrolling,
copy, and selection behavior are handled by the app layer. Tool, thinking, code,
Mermaid, system-prompt, and subagent popups use shared chrome and scrolling
helpers where possible.

## Caching and repaint policy

| Cache or flag | Owner | Invalidated by |
|---|---|---|
| Logical and visual row maps | `LogScroll` | Message list or width change |
| Styled Markdown and code | Block state | Block creation or language/theme change |
| Prepared diff rows | Tool popup state | Popup content change |
| `App::dirty` | App | Agent, input, timer, or host event |
| Animation tick | App frame loop | Each active animation frame |

The event loop polls at 10 ms when dirty, 150 ms while animating, 200 ms while
a timed state is expiring, and 1000 ms when idle.

## Theme and localization

`agent_tui_kit::theme::ThemeName` defines the built-in themes. The current list
includes Dark, Light, Solarized Dark, Solarized Light, Gruvbox Dark, Nord,
Retro, Kawaii, Japanese, Brutal, Ink, Ink Light, Claude, and Claude Dark.

`agent_tui_kit::i18n::Messages` supplies English and Chinese strings. The app
builds a fresh `Messages` value in each frame, so stored render outputs must
not freeze localized text or colors.

## Tests

Buffer-level tests live beside the renderers in both crates. App-level tests use
`crates/tui/src/render/test_harness.rs` and the fixtures in
`crates/tui/src/test_fixtures.rs`.

```bash
cargo test -p agent_tui_kit --lib
cargo test -p tui --lib
```

## Related documents

- [`tool_rendering.md`](tool_rendering.md)
- [`state_machines.md`](state_machines.md)
- [`book/23_chapter_tui_zh.html`](../book/23_chapter_tui_zh.html)
