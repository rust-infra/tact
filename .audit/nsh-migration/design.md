# Tact Desktop on Native Shell

## Goal

Replace the GPUI desktop (`crates/desktop`, ~17k production lines) with a Native Shell app on the same `tact_desktop_core`, and delete GPUI from the workspace.

## Definition of done

- `cargo run -p tact_desktop_nsh` opens the Tact window on macOS, Windows and Linux.
- Every surface of the current desktop is reachable and behaves the same: title bar, session rail, run room (chat + trajectory), composer, inspector (Run/Goals/Subagents/Background/Context), command center, slash overlay, settings, session manager, extensions, readers, goals, subagents, background tab, task DAG, diff review, mermaid, ANSI, web search.
- `tact_desktop_core` is unchanged.
- Headless NSH tests cover the shell and the surfaces the GPUI tests covered.
- `crates/desktop` and every GPUI dependency are gone.
- `nsh package` produces the macOS, Windows and Linux apps.

## Architecture

- `crates/desktop_nsh` is the NSH app: `nsh.json`, `theme.json`, `assets/`, `src/main.rs` (`platform::run_app`), `src/lib.rs` (root `App`), `src/shell.rs` (window shell), `src/ui/` (surfaces and pure decision modules).
- `DesktopController` starts before the window and is stored in a process-wide `OnceLock`.
- One `Signal<SnapshotRef>` carries the latest `DesktopSnapshot`; a task reads `DesktopEvents` and sets it on every commit. `SnapshotRef` compares by `Arc` identity so a commit never deep-compares two view models.
- `AppState { controller, snapshot }` is provided as context at the root; components read it with `use_app()`.
- Pure decision modules from the old `ui/` are copied verbatim; rendering modules are rewritten with NSH elements.
- Theme tokens map 1:1 to `theme.json`; custom tokens (`sidebar`, `title-bar`, `link`, `success`, `warning`, `danger`, `selection`, …) are read with `theme().color("…")`.
- Fonts ship as assets: IBM Plex Sans and Lilex.

## Migration order

1. Foundation: bootstrap, controller bridge, theme, fonts, empty shell. Done.
2. Pure modules: i18n, layout, shell enums, rail/transcript/inspector decisions. Done.
3. Shell chrome: custom title bar, responsive three-column layout.
4. Session rail: projects, sessions, filters, search, new session.
5. Run room: transcript rows (user, assistant, thinking, plan, tool group, diff, gate, notice, hook, error), markdown, ANSI, mermaid, task DAG.
6. Composer: multiline input, model picker, send/stop, queue, voice.
7. Inspector: Run/Goals/Subagents/Background/Context, trajectory tabs.
8. Overlays: command center, slash overlay, forms, settings, session manager, extensions, readers.
9. Tests: headless NSH tests, parity checks.
10. Packaging: `nsh package` for the three desktops; remove `crates/desktop`.

## Status

- 2026-11-04: `nsh-cli 0.3.0` installed; a scaffold app builds and runs; `tact_desktop_nsh` opens a window with the real controller and theme; fonts registered through `AssetBase::Directory`.
- 2026-11-04: the shell renders the real snapshot: title bar, session rail (filters, search, session selection, new session), run room (Chat/Trajectory tabs, transcript rows, markdown via `TextView`, gate buttons), composer (send/stop, queue count) and inspector (Run/Goals/Subagents/Background/Context).
- Verified: `cargo test -p tact_desktop_nsh --test shell` passes headless; the app launches and stays up with no runtime errors.
- Copied pure modules keep their tests in the old crate until the old crate is deleted; the new crate's tests are rebuilt with `nsh-testing`.

- 2026-11-04: overlays render over the shell: command center (live command search and invoke), settings (read-only rows), session manager (open and archive), extensions (skills, plugins, marketplaces), model picker. Shortcuts: `mod+k`, `mod+,`, `escape`.
- 2026-11-04: the Trajectory page lists the ledger with Summary/Payload/Result/Schema/Timing detail tabs; diff rows use `nsh_base::DiffView`; tool groups show ANSI output through the ported parser; assistant rows render mermaid flowcharts under the Markdown; the composer has model and permission controls.
- Verified: 15 unit tests (ANSI and mermaid parsers) and 3 headless shell tests pass; the app launches and stays up.

- 2026-11-04: the reader overlay handles `DesktopEffect::OpenReader` (the `help` command, and the Open buttons on tools, thinking and diffs). Clipboard, external links, file opens and window close are handled from `DesktopEffect`.
- 2026-11-04: typed surfaces render in their own overlay: system prompt, statistics, history, MCP servers and prompts, hooks. They close per revision without a controller intent.
- 2026-11-04: the composer lists slash-command suggestions from the catalog and runs the chosen command; the settings overlay edits booleans, choices and text settings; gates support multi-select with a draft and a Submit button.
- Verified: 16 unit tests and 6 headless shell tests pass; the app launches with no warnings.

## Done

- Attachments write picked image bytes to a temp directory and submit the paths; voice drives `VoiceCommand` and inserts the transcript.
- The app menu is drawn by the app (File/View/Session) because NSH 0.3.0 exposes native menus only to TSX.
- Toasts, the failure banner, the smoke mode, and the catalog parity tests are in.
- The NSH crate is now `crates/desktop` (`tact-desktop`); the GPUI crate and `vendor/gpui-ai` are deleted, GPUI workspace dependencies are gone, syntect uses fancy-regex, CI installs GTK 4, and packaging ships `tact-desktop-assets`.
- `nsh package --target macos` produced a launchable `Tact Desktop.app`.

## Parity restored after the first pass

- Transcript: run header and hero title, empty state, user bubbles with attachments and timestamps, assistant suggestions and mermaid, collapsible thinking, plan status marks, tool cards with text/code/diff bodies, duration and token metadata, nested subagent transcripts, retryable errors, pending-gate banner.
- Composer: queued prompts with send-now/remove/edit/move, history recall, context meter, slash suggestions with arrow/Tab navigation and Enter-to-run, model and permission controls, attachments and voice.
- Rail: project switcher, filters, search, session rename.
- Inspector: elapsed, context meter, task list, latest-tool open, subagent and background actions.
- Overlays: command center (prefills commands that need arguments), editable settings, session manager with project filter, extension tabs (MCP/Skills/Plugins/Hooks), model picker, reader with ANSI and diff rendering, typed surfaces.
- Trajectory: lane overview, search, detail tabs.

## Known gaps

- The transcript renders with `each` in a scroll. NSH 0.3.0 has `virtual_list` but no programmatic scroll-to-index on the scroll view, so tail-following (the old shell's automatic scroll to the newest row) has no equivalent yet. Revisit when NSH exposes it.
- Commands with structured arguments prefill the composer instead of opening the old form overlay; choice lists come from typing or the dedicated pickers.
- File mentions (`@`) insert the marker but have no workspace file picker; NSH 0.3.0 exposes no file picker, only `pick_images`.
- The GPUI behavior tests in the old `root.rs` were not ported one by one; the headless shell tests cover the shell, menus, overlays, trajectory, reader, slash suggestions, slash submission and shortcuts.
- Linux and Windows packaging run in CI only; macOS packaging (`nsh package --target macos`) and the packaged smoke test were verified locally.
- The app menu is drawn by the app because NSH 0.3.0 exposes native menus only to TSX.

The API cheat sheet from the porting agent is at `.audit/nsh-migration/api-notes.md`.

## TSX option (evaluated 2026-11-04)

NSH's TSX would make layout parity easier: the design prototype is HTML/CSS,
TSX takes Tailwind classes, and NSH's docs target AI-generated TSX screens.
The blocker is state, not layout. The desktop's state lives in
`tact_desktop_core` (a tokio actor publishing `DesktopSnapshot` commits), and a
TSX view runs on QuickJS inside the NSH engine. A mixed app would have to
expose the controller to TSX through a `#[native_module]` and a host channel,
then cross the JS boundary on every commit; the large snapshot and high commit
rate make that a measurable risk. A pivot also rewrites the ~6k-line Rust view
layer.

Spike result (2026-11-04, `/tmp/nsh-mixed/spike`): the bridge works. A Rust
`#[class]` with an async member taking a JS callback
(`pub async fn subscribe(this: Instance<Self>, callback: Callback<String>)`)
can `callback.post(..)` from engine timers, and TSX receives the values in a
signal; `nsh test` showed `tick 0, tick 1, tick 2` on screen. Synchronous class
members cannot use ambient services (signals, timers): the engine panics with
"no reactive runtime", so the bridge members that schedule work must be async.

The remaining work for a TSX shell is the data layer: a JSON view of
`DesktopSnapshot` (the core types do not derive `Serialize`), a JSON intent
channel for dispatch, and the TSX views. Recommendation: build that bridge
against the existing Rust controller, keep the Rust UI as the fallback until
the TSX shell reaches parity, then delete the Rust views.

## TSX shell (first version, 2026-11-04)

The app is now mixed: `App.tsx` is the NSH entry, and the Rust crate exposes
`Controller` through `#[native_module("tact-desktop/native")]`. The class
starts the existing controller, pushes `ShellSnapshot` JSON on every commit,
serves reader content on demand, and maps `UiCommand` JSON to
`DesktopIntent`. `json.rs` is the snapshot/intent schema.

`tsx/Shell.tsx` renders the shell in TSX: title bar with menus, rail, run
tabs, run header, sticky task strip, transcript rows (user, assistant,
thinking, plan, tools with bodies, diff, gate, notices, hooks, errors),
composer with slash suggestions, model/permission controls, queue actions
and context meter, inspector tabs, overlays (command center, settings,
sessions, extensions, models, projects), reader and toasts. The trajectory
tab is a first-pass record list.

Verified with `nsh test`: 88 tests pass (Rust unit, parity, Rust headless
shell, and the three TSX shell tests). `nsh check` type-checks the TSX.

Still to port into TSX: attachments and voice, ANSI rendering in tool
output, nested subagent transcripts, queue edit/move, the trajectory
detail panel, and MCP/hook lists in the extensions overlay. The Rust view
layer stays until the TSX shell reaches parity; packaging will move to
`nsh package` when the TSX shell becomes the only shell.
