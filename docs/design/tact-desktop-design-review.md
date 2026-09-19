# Tact Desktop Client — Design Review

Date: 2026-09-19
Spec: `docs/superpowers/specs/2026-09-19-tact-desktop-client-design.md`
Subject: `docs/design/tact-desktop-prototype.html` and its reference renderings

This is the Phase 2 review. It runs the GPUI Kit Design Guides checklists against
the prototype, records what was fixed, and parks what cannot be settled before
`tact-gui` exists.

## Method

- Checklists: the "Design review checklist" and "Accessibility checklist" from
  the GPUI Kit Design Guides.
- Evidence: headless Chromium renders at 1440×900, 1100×800, and 860×700;
  computed-style measurements taken from the live page; a WCAG contrast script
  run over the resolved token pairs.
- Renders are produced with `--force-prefers-reduced-motion` and
  `--hide-scrollbars`. The first makes captures deterministic and exercises the
  reduced-motion path; the second avoids a known headless race where a late
  scrollbar changes layout between capture runs.

## Design review checklist

| # | Question | Verdict | Evidence |
|---|---|---|---|
| 1 | Is the task clear? | Pass | The window title names the session, the transcript carries the work, and the composer is the single primary action. A new reader can identify purpose, primary action, and next step without explanation. |
| 2 | Does every action keep its promise? | Pass | Send becomes Stop while running and returns to Send on cancel. Permission choices write their result into the card and mark it resolved. Work-pane tabs switch the pane they name. |
| 3 | Is hierarchy decisive and restrained? | Pass | One accent hue, one primary button. Session state uses a dot plus a label; only the active session row and the current plan step take the accent surface. |
| 4 | Could the interface do less, better? | Pass, with scope note | Four work panes, one composer, one status bar. Browser and Terminal were dropped from v1 rather than added as panes. Deferred items are listed in the spec's approval points. |
| 5 | Is the structure exact? | Pass after fix | Alignment spines are consistent. The diff pane previously rendered its code column and marker column with inherited box styling from unrelated classes (see Finding 1); both are now correct. |
| 6 | Does it follow the component system? | Partial — expected | The prototype is HTML, so it cannot exercise gpui-kit component geometry. It does mirror the intended mapping (Tabs, Sidebar, Resizable, MessageScroller, DataTable, Command, Dialog, Sheet). Real verification lands in Phase 4–6. |
| 7 | Does it remain usable in every state and constraint? | Pass after fixes | Verified at 1440×900, 1100×800, and 860×700; empty/running/blocked/failed states are present; reduced motion no longer strands the palette (Finding 2); focus is visible (Finding 3) and restored (Finding 4). |
| 8 | Has it been tested in a real window? | Partial — expected | Interaction is verified in a real browser, not in a real GPUI window. This is the known and intended limit of a prototype. |

## Accessibility checklist

| Item | Verdict | Evidence |
|---|---|---|
| Every action reachable and operable by keyboard | Pass | Top tabs, work-pane tabs, theme toggle, sidebar toggle, detail cycle, and composer are native `button`/`input` elements. ⌘K, ⌘B, ⌘\, Ctrl+O, and Escape are bound. |
| Focus order follows visual and task order | Pass | DOM order matches reading order: title bar, sidebar, transcript, composer, work pane. |
| Focus visible and restored after overlays | Pass after fixes | Added a `:focus-visible` ring (2 px accent, 2 px offset). Opening the palette stores the trigger and closing restores focus to it. Tab now cycles inside the palette. |
| Controls have names; icon-only controls have tooltips | Partial — parked | The palette and work-pane close button carry `aria-label`s. The prototype's title-bar icons do not yet have tooltips; the spec already requires them, and they are tracked as a Phase 4 item. |
| Text and meaningful boundaries have sufficient contrast | Pass after fix | 40 of 40 measured text/background pairs now meet 4.5:1. See the contrast table. |
| Status is not communicated by color alone | Pass | Session state pairs a dot with text (`Running`, `Review`, `Archived`, `Clean`). Diff rows carry `+`/`−` markers, not only tint. Task states are labelled (`In progress`, `Done`, `Blocked`). |
| Disabled and read-only states distinguishable | Partial — planned | Send is disabled when the composer is empty and while running. Disabled styling exists but was not exercised across every control; the component layer must own it. |
| Labels, errors, descriptions near their controls | Pass | Permission choices sit inside the approval card; the permission selector sits in the composer. |
| Usable with longer translations and larger text | Partial — parked | Layout is `rem`-free and px-based in the prototype. The gpui-kit `zoom`/base-font system is the right place to prove this, so it is parked to Phase 4. |
| Pointer targets comfortably sized in a dense layout | Partial — parked | Session rows are ≥44 px. Icon-only title-bar buttons are 28×28. Either raise them to 32 px or keep the 28 px visual with a larger hit area; decide in Phase 5. |

## Contrast evidence

Measured with the WCAG 2.1 relative-luminance formula against the resolved
tokens. "Tint" means the semantic colour composited over `surface` at the
token's alpha.

| Pair | Light | Dark |
|---|---|---|
| `ink` on `surface` | 18.14 | 15.10 |
| `ink2` on `surface` | 6.48 | 7.15 |
| `ink3` on `surface` | 5.02 | 4.59 |
| `ink3` on `canvas` | 4.51 | 5.02 |
| `ink3` on `surface2` | 4.68 | 4.86 |
| `accentInk` on `accentTint` | 4.60 | 5.21 |
| `green` on `greenTint` | 4.65 | 4.99 |
| `red` on `redTint` | 4.64 | 4.61 |
| `blue` on `blueTint` | 4.62 | 4.61 |

Before this pass, eight pairs failed: `ink3` on `canvas` (4.32) and on
`surface2` (4.48) in light, `ink3` on `surface` (4.14) and `surface2` (4.38) in
dark, plus light-theme `green` (3.62), `blue` (2.88), `orange` (4.00), and
`accentInk` on its tint (4.10), and dark-theme `red`/`blue` on their tints
(4.21 / 4.37).

Corrected tokens:

| Token | Light before → after | Dark before → after |
|---|---|---|
| `ink3` | `#747168` → `#716e65` | `#858278` → `#8c8a80` |
| `accentInk` | `#b8563a` → `#ab5036` | `#e58d70` → `#e58e71` |
| `green` | `#788c5d` → `#60704a` | `#8fa875` → `#90a876` |
| `red` | `#c64b4f` → `#b54448` | `#e06c72` → `#e3787e` |
| `blue` | `#6a9bcc` → `#4b6e91` | `#6a9bcc` → `#6d9dcd` |
| semantic tint alpha | 0.12–0.14 → 0.10 | 0.14–0.16 → 0.12 |

Both the prototype and `docs/design/tact-desktop-theme.json` carry the corrected
values, so the renderings and the loadable theme agree.

## Findings fixed in this pass

1. **Diff pane class collisions.** The diff code column reused `.code` (the
   transcript code-block wrapper) and the diff marker column reused `.mark` (the
   app-logo swatch), so both inherited unrelated border, background, and radius
   styling and the diff rendered as stacked dark pills. Renamed to `.dcode` and
   `.dmark`.
2. **Reduced motion hid the command palette.** With `prefers-reduced-motion`,
   `animation-duration` was clamped to 1 ms while `@keyframes pop` starts at
   `opacity: 0`, leaving the palette computed at `opacity: 0` — an invisible
   command palette for anyone who asks for reduced motion. Animated regions now
   set `animation: none` under reduced motion; measured `opacity: 1`,
   `transform: none` after the fix.
3. **No keyboard focus styling.** The prototype had no `:focus-visible` rule, so
   it could not demonstrate the focus behaviour the spec requires. Added an
   accent ring system with a tighter offset inside the palette and work pane.
4. **Palette focus handling.** Focus was not returned to the trigger, Tab could
   escape the overlay, and the palette had no dialog semantics. Added trigger
   restoration, a Tab trap, `role="dialog"`, `aria-modal`, and labels.
5. **Contrast.** Eight pairs were below 4.5:1. Tokens were re-solved until all
   measured pairs pass.

## Deviations from the design guides

Each is deliberate and has a reason.

- **Raw hex and `rgba()` inside the prototype.** The guides allow raw colour in
  theme definitions, and that is exactly where these live: the `:root` and
  `[data-theme=dark]` blocks are the prototype's theme layer. Application code
  in `tact-gui` must resolve these through `cx.theme()` instead.
- **A serif for assistant prose.** The guides call for the platform UI font for
  interface text and monospace for code. Assistant answers are document content
  rather than interface chrome, so Lora carries them; every control, label, and
  metadata value uses the platform UI font.
- **Window traffic lights.** The three title-bar dots are macOS window chrome,
  not product tokens. They are drawn as decoration in the mock and must come
  from the platform in `tact-gui`.
- **Density varies by region.** The sidebar list uses 44 px rows while tables
  and toolbars use compact 28–32 px controls. The guides ask for density to be
  set per local context, not per control; each region is internally consistent.

## Parked items

These were parked during the prototype review. The Phase 4–7 follow-up below
records which items the production shell has since resolved.

| Item | Why parked | Where it lands |
|---|---|---|
| Icon-only targets at 28 px | Needs a decision between visual size and hit area | Phase 5 composer/toolbar work |
| Tooltips on title-bar icons | Spec already requires them; not needed to validate layout | Phase 4 shell |
| Zoom and larger-text behaviour | Must be proven with gpui-kit's base-font and `rem` system, not in HTML | Phase 4 shell |
| Disabled/read-only coverage | Belongs to the component layer | Phase 5–7 |
| Virtualized transcript and lists | Static prototype cannot exercise it | Phase 5 transcript |
| Diff horizontal scrolling in static renders | `--hide-scrollbars` is needed for deterministic headless captures; live behaviour was verified interactively | Note for future render scripts |
| `--orange` is a dead token in the prototype | Unused; `base.yellow` in the theme JSON remains the amber palette entry | Cleanup during token import |
| Spacing scale ownership | gpui-kit does not persist a custom `SpacingTokens` scale, so the app must own that snapshot | Phase 4 shell |

## Verification performed

- Rendered 1440×900 in light and dark, plus the Diff pane and the command
  palette in both themes; six reference images, all distinct.
- Rendered 1100×800 and 860×700 to confirm the work-pane drawer and sidebar
  overlay.
- Parsed the prototype script with `node --check` after every edit.
- Measured computed `opacity`/`transform`/`animation` on the palette before and
  after the reduced-motion fix.
- Ran a WCAG contrast sweep over 40 resolved text/background pairs in both
  themes; zero remain below 4.5:1.
- Confirmed repeat renders are byte-identical under the documented flag set.

## Not verified by the prototype

- No GPUI window was built or run during Phase 2. Component geometry, keyboard
  traversal inside real widgets, and theme-registry loading were verified later
  in Phase 4–7.
- No localization or long-string layout test was performed in the HTML
  prototype; the production shell uses rem-based dimensions and theme tokens.

## Phase 4–7 implementation follow-up

The production shell now verifies the items that Phase 2 could only park:

- Title-bar icon controls have tooltips and accessible names.
- The shell uses rem-based dimensions and theme tokens; the 960×640 minimum
  window and the <1280 px work-pane drawer / <960 px sidebar overlay are covered
  by headless UI tests.
- `MessageScroller` and the work-pane lists virtualize long transcripts and
  diffs.
- Disabled/read-only behavior is delegated to the gpui-kit components rather
  than restyled locally.
- The command palette, settings dialog, Normal → Thinking → Verbose transcript
  cycle, light/dark controls, and window-level keyboard contract are exercised
  by `crates/tact-gui/tests/shell.rs`.
- The sidebar lists workspace-local sessions as short id + age + message count;
  selecting a row or pressing `Ctrl+Tab` resumes that session through the shared
  `tact-session` runtime.
- The 2026-09-19 GUI smoke run under Hyprland confirms the app launches in the
  live workspace, loads the Tact light theme without asset/theme-registry
  errors, renders the session surface, and reports a successful resume.
- Post-v1 layout persistence is parked: the v1 shell keeps fixed sidebar and
  work-pane widths, selected pane, and transcript detail instead of restoring
  them from a settings store.

The remaining visual risk is platform-specific font fallback and OS-level
window chrome; this host falls back from DejaVu Sans Mono to Noto Sans Mono.
