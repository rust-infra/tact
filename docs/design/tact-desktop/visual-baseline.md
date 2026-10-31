# Tact Desktop visual baseline

The 2026-10-31 review found that the desktop shell passed every automated gate
while still looking nothing like the approved prototype. The gates measured
routes, constants, and "does not panic"; none of them compared the rendered
surface with the design. This baseline exists so that cannot happen silently
again.

## What "done" means now

A desktop surface is not done because its route exists or its test is green. It
is done when:

1. the off-screen capture is deterministic (no developer session data),
2. the prototype and the implementation are reviewed side by side, and
3. the layout assertions that guard the reviewed geometry pass.

The prototype in `docs/design/tact-desktop/previews` is the design oracle. The
implementation is expected to match its structure and information density, not
merely its feature list.

## Status: UI restoration complete

Every surface the prototype defines is implemented and measured against a
1280x840 @2x render of the prototype: title bar, rail (project card, new run,
search, sections, trailing times, quota card, settings), transcript (run
header, user/assistant/tool/diff/notice/hook/error/gate rows, plan card),
composer (single footer row, running status, stop + send), inspector (Run map,
goals, subagents, context), and the overlays (command palette, session manager,
extensions popover/manager, hooks sheet, gate review, subagent drawer,
settings).

Automated guards: `chrome_matches_prototype_metrics`,
`overlays_match_prototype_sizes`, `extensions_overlays_match_prototype_sizes`,
`composer_shows_stop_and_send_while_running`, the row-kind tests, and the
Light/Dark anchor checks in `scripts/check-ui.sh`.

Deferred to business wiring, not to visual work: live account identity (avatar
initials, user avatar), the run summary line under the headline, model and
permission values, extension counts, and hooks/session data. The capture
harness seeds the design's copy from `26a` onward so the transcript compares
like-for-like.

## 1. Deterministic captures

`crates/desktop/src/capture.rs` now starts the controller against a disposable
project under the harness's temp data directory and seeds four fixed session
rows. It no longer opens the checkout's `.tact` store, so:

- the rail always shows `Sessions · 4` with stable titles and counts,
- no developer session, prompt, or title appears in a screenshot,
- two runs on different machines produce the same shell.

The seeded transcript, inspector, background, and gate captures come from
`crates/desktop_core/src/fixtures.rs`, as before.

```bash
cargo run -p tact-desktop --example ui_capture --features ui-capture -- target/ui-capture
```

## 2. Prototype-vs-implementation pairs

`scripts/ui-parity.py` composes one image per reviewed surface with the
prototype on the left and the capture on the right, plus a contact sheet.

```bash
python3 scripts/ui-parity.py target/ui-capture target/ui-parity
```

It requires Pillow. The pairs are:

| Name | Prototype | Capture |
|---|---|---|
| `shell-light` | `light.png` | `01-shell-light.png` |
| `shell-dark` | `dark.png` | `02-shell-dark.png` |
| `command-center` | `command-palette.png` | `03-command-center.png` |
| `session-manager` | `session-manager.png` | `06-session-manager.png` |
| `extensions` | `extensions-popover.png` | `07-extensions-mcp.png` |
| `permission-gate` | `permission-gate.png` | `33-gate.png` |
| `sticky-tasks` | `sticky-tasks.png` | `26b-transcript-bottom.png` |
| `composer` | `composer-attachments-queue.png` | `26b-transcript-bottom.png` |

## 2b. Pixel workflow (design parity)

The prototype is rendered by headless Chrome at the same logical size as the
capture (1280x840 @2x), so both images share coordinates:

```bash
node scripts/proto-shot.mjs target/ui-proto/prototype-light.png light
python3 scripts/ui-check.py target/ui-capture target/ui-proto/prototype-light.png
```

`scripts/ui-check.py` fails when a structural anchor drifts more than 3 device
pixels from the design: rail edge, titlebar height, composer left/right.
`scripts/ui-diff.py` writes the prototype | implementation | diff overlay and
per-region mean differences for human review. `scripts/check-ui.sh` runs the
whole loop (capture, prototype render, anchors, overlay).

Current anchors at 1280x840: rail edge 498px (249 logical), titlebar edge
114px (57 + 1px border = 58), composer 630..2429px (315..1214.5 logical,
900px wide, centered). Latest 2026-10-31 region means (0-255): Light titlebar
10.26, rail 18.53, composer 8.81, inspector 5.67; Dark 9.90, 23.12, 7.29,
7.30.

## 3. Review protocol

For every desktop change that touches a shell surface:

1. regenerate the captures;
2. regenerate the pairs;
3. open the affected pair and list the differences that are *not* intentional;
4. fix them, or record them as an explicit deferral in this file.

A pair that was generated but never opened is not a review.

## 4. Automated guards

These tests fail when the reviewed geometry regresses:

- `chrome_matches_prototype_metrics` — titlebar 58px, rail 250px, composer and
  sticky 900px centered.
- `overlays_match_prototype_sizes` — command palette 700px, session manager
  1040x720.
- `scripts/ui-check.py` runs for both Light and Dark in `check-ui.sh`.

- `session_list_fills_the_rail_instead_of_an_empty_spacer` — the session list
  must reach the extensions section; an empty flex spacer must not steal the
  rail's free height.
- `back_to_bottom_floats_above_the_sticky_strip` — the recovery pill must sit
  above the sticky strip *and* left of the open inspector.

The remaining P0/P1 gaps are tracked in the remediation plan, not hidden in a
green test run.

## 5. Known gaps at baseline (2026-10-31)

| Surface | Gap | Status |
|---|---|---|
| Session rail | Rows show title + count only; no relative time or preview | Fixed: two-line rows (`SidebarNavItem::detail`) |
| Session rail | Titles truncate without ellipsis; one UUID fallback observed | Fixed: `truncate()` on the row label |
| Composer | Permission picker is a separate full-width row; `@`/`/` are a separate button row; no single composer bar | Fixed: one footer row inside `PromptBar` |
| Sticky strip | One-line text bar; no task card, Goals/Agents counts, or background entry | Fixed: task card with header, counts, progress, rows |
| Title bar | No branch/worktree or account avatar (PRD §title bar) | Fixed: branch pill from `.git/HEAD` (avatar is tracked separately) |
| Inspector | Fixed 318px; values were clipped under the scrollbar (now padded, still cramped) | Fixed: scrollbar padding + truncation |
| Command Center | Flat list; no groups, icons, recent section, or per-row shortcut hints | Fixed: group subtitle, group icon, `/command` hint, count caption |
| Session Manager | No project grouping/chips, archive badge, or keyboard hints | Fixed: project chips include All projects; archived rows carry a badge |
| Extensions | Empty tabs render nothing; `Refresh MCP` is a bare link | Fixed: MCP shows `No MCP servers yet` above Refresh |
| Transcript | Hook context rendered as bare markdown; tool groups and diff were outside the capture window | Fixed: hook context card; captures now cover tool cards (`26c`) and the transcript tail |
| Inspector | Run tab was a flat key/value list with no current step or signal rows | Fixed: Run map header, gauge, Current step, Thread signals, Latest activity |
| Gate | Centered review modal with full-width buttons instead of the inline card | Deferred: the review surface is the documented keyboard flow (README "Pending permission"); the inline card remains the transcript summary |
| Account avatar | Title bar has no account avatar | Deferred: `AccountView` carries plan/quota but no user identity to derive initials from |
| Command Center icons | Rows have no leading icon | Fixed: group icons via the `CommandSearchItem::icon` patch |
| Session Manager | Layout was a single narrow column | Fixed: 1040x720 dialog with a 232px project pane, header and list |
| Overlays | Palette was 640px; manager 760px | Fixed: palette 700px, manager 1040x720 |
| Extensions | One 720px dialog for both popover and manager | Fixed: popover 520px, manager 780x620 with underline tabs |
| Session rail | Project subtitle had no branch | Fixed: `4 sessions · feat/desktop` |
| Composer | No running status or stop affordance; send was a disabled outline | Fixed: `Running · Esc interrupts`, red stop button beside a solid blue send button |
| Title bar | Workspace pill showed only the folder name | Fixed: `owner/repo` from the origin remote, branch and short session id (`rust-infra/tact \| feat/desktop \| session 8f2a`) |
| Subagent transcript | Centered 820px modal | Fixed: right drawer 520px, full height |
| Gate review | Plain 560px modal with full-width rows | Fixed: 640px amber card with signal dot, kicker, esc hint |
| Rail rows | 55px pitch | Fixed: 53px pitch (`SidebarNav` row padding 7) |
| Run gauge | 4px ring | Fixed: 5px ring |
| Hooks | Only an Extensions tab | Fixed: dedicated 780x620 `Command hooks` sheet with trust rows and footer |
| Rail quota | Card was absent because the capture had no account data | Fixed: `Kimi K2.5 · 78%` card with green bar and reset line (real `AccountView::UsageQuota`) |
| Rail session rows | Right side showed an active check, no time | Fixed: `SidebarNav` trailing slot shows `19:04` / `Oct 28`; active dot marks selection |
| Transcript plan | No plan row kind existed | Fixed: `RowBody::Plan` with titled step list, `PLAN / N STEPS` kicker and EXECUTING chip |
| Run title | Included the leading URL from the first prompt | Fixed: `strip_leading_url` drops a leading `http(s)://` token, so the headline matches the design |
| Run summary | No second line under the headline | Fixed: the prompt's second line renders as the muted summary (`split_run_text`) |
| Avatars | Hard-coded `Y` / `RG` | Fixed: local account initials from `USER`/`USERNAME` |
| Dark theme | Not compared against the design | Fixed: dark prototype render and anchor check |
| Session Manager keyboard hints | No footer shortcut hints | Deferred (P2) |
| i18n | System notices such as `Context compacted: ...` stay English in the Chinese shell | Deferred: the text is produced by the runtime, not the desktop shell |
