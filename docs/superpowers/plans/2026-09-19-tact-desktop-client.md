# Tact Desktop Client — Design and Delivery Plan

Status: Phases 0–2 and 4–8 complete; Phase 3 awaits the Figma connector.
The post-v1 layout store (persisted arrangement, draggable columns, presets, and
zoom), the Stats pane, session pinning, the PTY-backed Terminal pane, the
system-browser Browser pane, and right/left/bottom work-pane docking have since
shipped. Figma (Phase 3) is the only item still blocked.
Date: 2026-09-19  
Spec: `docs/superpowers/specs/2026-09-19-tact-desktop-client-design.md`  
Verification: cargo checks/tests/build/clippy passed offline; GUI smoke
verified the live Tact light theme and resume path.

## Phase 0 — Direction and theme approval

Status: complete — Direction A, the `Chat / Agent / Code` tabs, and the
Anthropic-first theme were adopted.

- Review the three layout directions in the spec.
- Approve or change the recommended shell: sidebar + transcript + resizable
  work pane.
- Approve or change the `Chat / Agent / Code` top-level tabs.
- Approve the Anthropic-first theme and the orange primary accent.
- Confirm v1 excludes embedded Browser and Terminal panes.

Exit criteria:

- One layout direction is selected.
- The theme token table and typography split are accepted.
- Open product decisions are recorded in the spec.

## Phase 1 — Clickable prototype

Status: complete — `docs/design/tact-desktop-prototype.html`.

- Build a realistic 1440 × 900 prototype using local HTML/CSS/JS or the
  prototype skill.
- Include light and dark modes.
- Include the shell, session sidebar, transcript detail levels, composer queue,
  tool activity rows, permission dialog, diff pane, plan pane, tasks table, and
  subagent pane.
- Include empty, running, waiting, failed, and narrow-window states.
- Verify keyboard focus, overlay dismissal, and pane toggling in the prototype.

Exit criteria:

- A reviewer can complete the primary task without explanation.
- The transcript remains readable at 1440 px and at the minimum supported
  width.
- No critical action depends on hover.

Verified: all four work panes render in both themes; the 1100 px work-pane
drawer and the 860 px sidebar overlay behave as specified; the prototype's
script parses cleanly. Long diff lines scroll rather than clip. See spec
§13.1 for the full checklist.

## Phase 2 — Static reference and review

Status: complete — see `docs/design/tact-desktop-design-review.md`.

- Render the selected prototype to PNG for documentation.
- Run the GPUI Kit Design Review checklist and the accessibility checklist.
- Record any deviations from the design guides with a concrete reason.

Exit criteria:

- The reference render matches the approved tokens.
- Contrast, focus, density, and copy issues are resolved or explicitly parked.

Verified: six reference renders (light/dark, Diff pane, command palette) match
the theme file after the contrast corrections; 40 of 40 measured text pairs meet
4.5:1; focus visibility, focus restoration, and reduced-motion behaviour were
fixed and re-measured. Remaining density, tooltip, zoom, and virtualization
items are parked in the review with concrete landing phases.

## Phase 3 — Figma source of truth

Status: deferred — the connector is not available in this environment. The
prototype and theme JSON remain the source of truth.

- Translate the approved prototype into Figma when the connector is available.
- Create component variants for buttons, fields, tabs, rows, tool activity,
  messages, dialogs, and work-pane headers.
- Keep Figma and the theme JSON aligned by token role, not by literal color
  duplication.

Exit criteria:

- Figma components cover the v1 shell and transcript.
- Token names match the documented semantic roles.

## Phase 4 — `tact-gui` bootstrap

Status: complete.

Dependency: `gpui-kit = "0.6.4"` (verified current on crates.io 2026-09-19; the
crate bundles GPUI, GPUI Base, GPUI Component, and default assets). It is not in
the local Cargo cache, so the first build needs network access. Expect a large
dependency tree because GPUI is pulled in transitively.

- Add `crates/tact-gui` to the workspace without touching the headless crates'
  dependency direction.
- Bootstrap `gpui-kit`, the theme registry, the root window, and the minimum
  window size.
- Implement the title bar, sidebar shell, transcript placeholder, status bar,
  and pane toggle.

Exit criteria:

- The app launches with the Tact light and dark themes.
- Window resize and focus ownership work at the minimum size.
- No raw color literals appear in feature code.

Verified: the Tact light/dark registry loads; the shell renders at 1440x900 and
960x640; the work pane becomes a right drawer below 1280 px and the sidebar
becomes an overlay below 960 px. No raw color literals appear in feature code.

## Phase 5 — Transcript and composer

Status: complete.

- Implement `MessageScroller` with tail following and jump-to-latest behavior.
- Map streaming text and thinking lifecycle events.
- Implement the tool activity row and detail expansion.
- Implement composer growth, mentions, attachments, queueing, Stop, model,
  effort, permission, and usage ring.
- Map the `AgentUpdate` and `UserCommand` tables from the spec.

Exit criteria:

- A real session can be started, streamed, cancelled, and resumed.
- Pending user responses cannot be lost or answered twice.
- Long output does not block the UI.

Verified: the `tact-session` bridge maps streaming, thinking, tool, task,
permission, and account updates; the composer queues while running, Stop sends
`Cancel`, and `MessageScroller` virtualizes long transcripts. The sidebar lists
recent workspace sessions and resumes by id; a temporary real-provider probe
confirmed a fresh session, incremental streaming, a resumed thread, and a
cancelled long turn. The probe was removed after verification because it was a
one-off diagnostic, not a shipped example.

## Phase 6 — Work panes

Status: complete.

- Implement Plan and Tasks from protocol snapshots.
- Implement Diff with file navigation, comments, and a clear return path to the
  conversation.
- Implement Subagent runs and transcript inspection.
- Implement Files as a tree and preview surface.
- Add narrow-window sheet behavior. Pane-layout persistence ships through
  `crates/tact-gui/src/layout.rs`: the arrangement, both column widths, the
  transcript detail level, and the base font size survive a restart, the two
  dividers are draggable, and four presets switch arrangements.

Exit criteria:

- The user can verify a change without leaving the app.
- Pane state remains associated with the correct session.
- Large diffs and task lists remain responsive.

Verified: Plan, Diff, Tasks, Subagent, and Files render from protocol
snapshots; the Files pane sorts directories first and hides dotfiles by
default, opens a bounded file preview, reveals the selected path, inserts an
`@relative/path` mention, and opens the selected file with the default
application; Plan rows expand to their input/result, failed steps can retry
through the agent, and Open transcript reveals the matching tool card; Tasks
rows filter/sort locally, advance status through `TaskUpdate`, and open their
owning session; Subagent rows can cancel a running child and inspect its stored
transcript; Diff cards stage a path through `git add`, and Comment prepares one
batch review draft in the composer; narrow windows use the work-pane drawer and
sidebar overlay. Session adoption resets transcript, request, diff, task,
subagent, and Files selection state with the session's `SessionState`; Files
expansion is an app-level navigation preference.

## Phase 7 — Commands, settings, and polish

Status: complete.

- Add the command palette and Tact action registry.
- Add settings with visible labels, help text, and immediate-state controls.
- Add notifications, error recovery, tooltips, and menu shortcuts.
- Verify light/dark, zoom, CJK, keyboard-only, reduced motion, and focus return.
- Add UI tests for shell invariants and critical event transitions.

Exit criteria:

- All v1 commands are reachable by keyboard.
- The design review checklist passes.
- No unresolved blocker remains for the first desktop release.

Verified: the command palette and settings are live; the Normal → Thinking →
Verbose cycle, theme controls, and reading controls update immediately; the
window-level keyboard contract is covered by headless UI tests. A 2026-09-19
GUI smoke run in the live workspace activated `Tact Anthropic Light` and
rendered the session list; `Ctrl+Tab` resumed a recent session and showed the
resume system row. The design review remains the visual checklist source of
truth.

## Phase 8 — Session actions

Status: complete.

- Add the reversible session metadata contract: `sessions.title` and
  `sessions.archived_at`, with an in-place migration for existing stores.
- Add presentation-neutral `rename`, `set_archived`, `duplicate`, and `reveal`
  actions in `tact-session`, then wire the GUI session menu to them.
- Keep archive non-destructive and make duplicates copy the conversation without
  provider state or recorded token usage.

Verified: `cargo test -p tact-gui --offline` passes outside the sandbox for the
git-worktree fixture (73 library + 89 integration tests);
`cargo test -p tact-session --offline` passes 39 tests; `cargo check -p
tact-gui --offline --all-targets` is clean. The click walk presses rename,
duplicate, archive/unarchive, and reveal, and asserts the visible result of
each.

## Deferred

- Free-form Dock rearrangement (a splitter tree).
- A web view embedded inside the Browser pane.
- Web/mobile client.

Everything else on the original deferred list has shipped:
**Terminal** as a real PTY-backed shell (`crates/tact-gui/src/terminal.rs`),
**Browser** as an address bar that hands URLs to the desktop's default browser,
**Stats** as the chart-heavy dashboard, **layout persistence** with draggable
columns and four presets (`crates/tact-gui/src/layout.rs`), and **work-pane
docking** to the right, left, or bottom.

Two of those are narrower than the original one-line item, and deliberately so.
The Browser pane cannot embed a web view: GPUI renders its own GPU surface and
has no way to host `webkit2gtk` or `wry` inside it, so the pane opens the system
browser instead and says so on its face. Docking ships as three named edges plus
persisted widths, not as a free-form splitter tree.


## Release verification

Run sequentially; Cargo commands must not be run in parallel against this
workspace because they contend on the `target/` lock.

- `cargo fmt --all -- --check` — pass.
- `cargo check --workspace --all-targets --offline` — pass.
- `cargo test -p tact-gui --offline --lib` — 110 library tests pass.
- `cargo test -p tact-gui --offline --test shell` — 98 integration tests pass.
- `cargo test -p tact-session --offline` — 41 tests pass.
- `cargo test -p tact --offline --lib store::session_store` — 25 tests pass.
- `cargo test -p tact-ui --offline` (outside the sandbox for wiremock) — 105
  tests pass across the unit and integration suites.
- `cargo build --workspace --offline` — pass.
- `cargo +stable clippy -p tact-gui -p tact-session --all-targets --offline
  --no-deps -- -D warnings` — pass.
- GUI smoke: `target/debug/tact-gui` launched under Hyprland, loaded the Tact
  theme, listed recent sessions, resumed a session with `Ctrl+Tab`, and exited
  without an asset or theme-registry error. The log contains only the expected
  mono-font fallback warning on this host.

The parked post-v1 scope is documented in the spec: layout persistence and
restoration, richer session search/grouping/branch metadata, and Phase 3 Figma.
