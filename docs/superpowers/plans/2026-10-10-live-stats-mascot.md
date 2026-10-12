# Plan: the live-stats "working" mascot

Spec: [2026-10-10-live-stats-mascot-design.md](../specs/2026-10-10-live-stats-mascot-design.md)

Implemented on `main` 2026-10-10; this plan is **backfilled** 2026-10-12 from the
landed code, because main shipped the feature with a spec only.

Ported onto the post-migration layout (2026-10-12, `cae4920c`): paths are
`crates/agent_tui_kit/…` and `crates/tui/…`; main's version bump was not carried
over. `./scripts/check-rust.sh` → `Rust checks passed.`; whole-workspace tests
2987 → 3016 passed / 0 failed / 22 ignored (47 suites).

## 1. Pure geometry (`crates/agent_tui_kit/src/render/stats_line.rs`)

- `MASCOT_MIN_RUNWAY: u16 = 4` — the shortest runway worth walking. Below it
  `mascot_cells` returns `None`: a narrow row *is* a row with no padding, so
  there is no second degrade branch.
- `MASCOT_RIGHT: char = '\u{F0530}'` (`md-transfer_right`) and
  `MASCOT_LEFT: char = '\u{F0DA2}'` (`md-transfer_left`). A terminal cannot
  mirror a glyph, so "turning around" is the swap to the other-facing glyph.
- `mascot_runway(pad) = pad.saturating_sub(2)`: the text starts at `pad`, so the
  far end is `pad - 2` — one blank column always stays between mascot and text.
- `mascot_step(tick: u32, runway: u16) -> (u16, bool)`: a ping-pong, one column
  per tick, `bool` = heading for the far end. `leg = runway - 1`; `period =
  leg * 2`; `tick % period <= leg` → out, else back. `leg == 0` → `(0, true)`.
- `mascot_cells(area, pad, tick) -> Option<[(u16, char); 2]>`: left cell at
  `area.x + 1 + pos`, right mirrored at `area.x + width - (2 + pos)`; the two
  glyphs are opposite (`forward` → right/left, else left/right). `None` when the
  runway is short, the width < 2, or the two runways would meet (`right <= left + 1`).

## 2. Drawing on the live band (`render_live_stats_band`)

- The band paints the whole row with `theme.bg` first (the AGENTS.md render
  invariant), renders `task_stats_body` centered at `center_pad`, then draws the
  mascot cells last, on top.
- Mascot style is `theme.warning` on `theme.bg` — the row's own text is
  `theme.accent`, and a same-red critter would read as part of the sentence.
- The mascot is drawn only in the live band; the frozen log row is built by
  `task_stats_body` and never carries one (a finished turn must not animate).

## 3. Tick semantics: a monotonic counter (`u32`)

- `crates/agent_tui_kit/src/render/ctx.rs`: `RenderCtx::spinner_frame: u32` —
  monotonic, the host no longer wraps at 10, so a frame-index animation takes
  `% LEN` itself and the mascot's ping-pong can use the raw counter.
- `crates/tui/src/widgets/state/mod.rs`: `App::spinner_frame: pub(crate) u32`;
  initialized to `0` in `crates/tui/src/widgets/state/app/construct.rs`, and
  mapped into `RenderCtx` in `crates/tui/src/widgets/state/app/config.rs`.
- `crates/tui/src/lib.rs`: in the active state,
  `app.spinner_frame = app.spinner_frame.wrapping_add(1)` — the wrap (at `u32`,
  ~20 years of 150 ms frames) lives here, not at each animation.
- Consumers take their own modulo: `crates/agent_tui_kit/src/render/bar.rs`
  (`SPINNER_FRAMES[ctx.spinner_frame as usize % SPINNER_FRAMES.len()]`, the status
  bar's `Status::Planning` / `Status::Executing` spinners) and
  `crates/agent_tui_kit/src/render/log.rs` (`TOOL_RUNNING_SPINNER[ctx.spinner_frame
  as usize % …]`, `SPINNERS[…]` for the loading placeholder). `bar.rs` had no modulo
  before (it indexed `SPINNER_FRAMES` with the host's `% 10` value) — the modulo was
  added so a monotonic counter cannot index out of bounds.

## 4. Tests

- `cargo test -p agent_tui_kit --lib render::stats_line::` — the six pure cases:
  `the_mascot_walks_to_the_far_end_and_back` (full round trip, no skipped
  endpoint), `the_mascot_turns_to_face_the_way_it_is_walking` (swap only at the
  two ends), `the_mascot_stays_clear_of_the_text_columns` (every pad × every
  tick), `the_two_mascots_face_opposite_ways` (mirror phase),
  `a_row_without_padding_has_no_mascot` (below `MASCOT_MIN_RUNWAY` → `None`),
  `a_degenerate_runway_does_not_panic` (runways 0/1 and a `u32` wrap).
- `cargo test -p tui --lib render::log_render_tests::` —
  `the_live_row_walks_a_mascot_on_both_paddings`: the buffer row carries exactly
  two mascot glyphs, both outside the text ink, facing opposite ways, on
  `theme.bg`.
- **Changed assertion, not just a passing rerun:** `stats_row_padding` now treats
  the mascot glyphs as blank through `is_mascot_glyph` (matching `MASCOT_LEFT` /
  `MASCOT_RIGHT`); otherwise the centering test would measure where the critters
  stand instead of the text — it would still pass while measuring something else.
- `./scripts/check-rust.sh` (fmt + clippy `-D warnings` + the four packages).
  2987 → 3016 passed / 0 failed / 22 ignored (47 suites) for the three #87 slices.

## 5. Docs

- `docs/token_usage_schema.md` § Per-Turn Stats Line → "The mascot on the live
  row" (line ~432).
- `book/22_chapter_tui_zh.md` (the live-stats mascot paragraph, ~§6.6).
- `book/25_chapter_issue_zh.md` — the 2026-10-12 `移植 main #87：…吉祥物` entry.
