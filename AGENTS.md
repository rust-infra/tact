# Agent guidelines (Tact)

Conventions for AI agents working in this repository. Prefer small, focused diffs; do not commit unless asked.

When giving a final answer, always structure your reasoning as a Markdown list (using - or 1. 2. 3.), and place each step on a new line for readability.

## Cargo / tests (agents)

- **Never** launch multiple `cargo test` / `cargo build` / `cargo clippy` processes against this workspace in parallel. They contend on the same `target/` lock and can sit for many minutes looking hung.
- Prefer **one** cargo invocation with a filter (e.g. `cargo test -p tact --lib voice::`). Do not start a second cargo command until the first exits.
- Async tests that wait on channels must use timeouts (see `voice` worker tests); do not add unbounded `recv().await` in new tests.

## Documentation sync — when to update

Update docs **in the same change** (or immediately after) when behavior or public contracts change. Do not leave book / design docs lagging behind code.

| Trigger | Sync these |
|---------|------------|
| Agent loop / compaction / recovery behavior changes | `book/05_chapter_compact.md` **and** `book/05_chapter_compact_zh.md`; skim `ARCHITECTURE.md` §6 and `docs/compaction.md` if the overview drifts |
| Config / CLI flags rename or semantics change | `book/` chapter that documents them, `config.example.toml`, relevant `docs/superpowers/specs/` or plans |
| TUI bottom-bar / token / cache display changes | `docs/token_usage_schema.md` (TUI display notes) and any book section that describes the bar |
| New multi-step feature from brainstorming | Write `docs/superpowers/specs/YYYY-MM-DD-<topic>-design.md` after design approval; add `docs/superpowers/plans/YYYY-MM-DD-<topic>.md` before or with implementation |
| Store / session persistence contracts change | `book/01_chapter_store*.md`, `docs/token_usage_schema.md` if usage tables change |
| Shipped optimization or bug fix with user/API-visible behavior change | Append a newest-first entry to `book/26_chapter_issue.md` **and** `book/26_chapter_issue_zh.md` (same section id / heading hierarchy). Link the PR, design spec/plan if any, and related subsystem chapters. Do **not** replace subsystem chapters — Ch 26 is the changelog; Ch 5/7/… remain the how-it-works docs |

### Issue log entry requirements (`book/26_chapter_issue*`)

When the sync table requires a Ch 26 entry, include at least:

- Date (`YYYY-MM-DD`), type (`optimization` / `bugfix` / `removal` / `docs`), optional PR URL
- Symptom / motivation before the change
- Final decision and observable post-change behavior
- Code / spec / related chapter pointers

Skip Ch 26 for pure refactors, test-only changes, and comment/typo-only edits (same as “When *not* required” below).

### Bilingual book chapters

Paired files `book/NN_chapter_*.md` and `book/NN_chapter_*_zh.md` must stay **structurally aligned**:

- Same section numbering and heading hierarchy
- Same mermaid / tables updated on both sides when the described behavior changes
- Prefer updating both in one commit when the change is behavioral

If only wording polish is needed on one language, that is fine; do not leave one language describing an obsolete algorithm.

### When *not* required

- Pure refactors with no user-visible or API-visible behavior change
- Test-only changes
- Typo fixes confined to code comments

## TUI rendering — no shadow / residue (design invariants)

The recurring "shadow" / ghost-cell bugs share one root cause: ratatui
computes a full buffer each frame, then **diffs cell-by-cell and only sends
changed cells to the terminal**. Any cell you intended to restore to the
background but never actually re-wrote is judged "unchanged" and is not
emitted, so the terminal keeps the previous frame's style — which reads as a
shadow. Guard against it with these invariants when writing or reviewing
main-area render code (`crates/tui/src/render/**`):

1. **Every renderable unit paints its own background — never rely on the
   caller to clear first.** `render` / `render_partial` must begin by filling
   its full `area` with `base_bg` (`buf.set_style(area, Style::default().bg(theme.bg))`)
   or render through a widget that does (e.g. `Paragraph` with `.style(bg)`).
   Row tails, blank separator rows, and indent gutters are part of the unit's
   area, not someone else's job.
2. **A span-carried background only covers its glyph columns.** A span with
   `code_block_bg` / `theme.highlight` / any custom bg paints a colored patch,
   not a full-width bar. Only give a span a bg if that patch is wanted; if a
   row must be a full-width band, paint the whole row — not per-glyph.
   `wrap_line` re-slices styled spans per wrapped segment, so a bg on a
   wrapped line multiplies the patch across every continuation row.
3. **"Restore to default" must actually write the style.** For cells that must
   be correct even when their content looks unchanged across frames (chrome
   columns, scrollbar neighbors), force-emit with `CellDiffOption::AlwaysUpdate`
   — see `restamp_log_left_border` in `crates/tui/src/render/log.rs`.
4. **Overlays/popups size and center against their real parent rect, not the
   whole frame.** Palette / select / file-picker / slash popups are rendered
   from `lib.rs`; they must receive the main area (`chunks[1]`), never `size`.
   A popup's `Clear` rect must cover (≥) its draw rect and come from the same
   geometry, or adjacent chrome (input box / bottom bar) shows through as a
   shadow-like mess.
5. **Wide graphemes (emoji/CJK, width 2) make gaps worse**; after writing
   glyphs, the remainder of a row must still carry the base bg rather than
   leftover cells.
6. **Every new render unit ships a buffer-level test** asserting that blank
   cells carry `theme.bg` (pattern: `heading_rows_carry_no_highlight_band`,
   `full_frame_palette_popup_stays_inside_main_area` in
   `crates/tui/src/render/*_tests.rs`).

History (why each rule exists): `book/26_chapter_issue.md` entries
2026-07-27 "Log scroll restores the theme background", 2026-07-28 "Log
left-border scrollbar residue", 2026-08-16 "Main-area headings no longer
paint the highlight band" and "Overlay list popups stay inside the main
area".

## Compaction (quick pointer)

Current design: Codex-style rebuild — recent real user messages + `SUMMARY_PREFIX` handoff; entry path compacts **before** pushing `user_turn_message` and reserves incoming-turn size in `should_auto_compact`. Spec: `docs/superpowers/specs/2026-07-18-codex-style-compact-design.md`. Legacy single-summary path: `Agent::compact_history_legacy`.

## Hosted tools (Provider-executed) — design invariants

Hosted web search is a **Responses-protocol capability**, independent of the
endpoint/provider behind the protocol. The Responses adapter injects the
hosted `Tool::WebSearch` on **every** ordinary `/responses` request whenever
the user chooses `protocol = "responses"` — for OpenAI, DeepSeek, and custom
OpenAI-compatible endpoints alike. There is no per-provider switch and no
capability negotiation: the protocol is the contract. The only exception is
the `/responses/compact` path, which never sends tools. Do **not** regress
these invariants. The same rules apply to **any future hosted tool**
(file search, computer use, …): a hosted tool is one the Provider executes
and Tact only renders.

1. **Injection, never replacement.** `native_web_search` only *adds* a hosted
   tool in `create_response`; it must never inspect or rewrite tool names.
   An MCP-provided `web_search` function tool stays `Tool::Function` and both
   mechanisms coexist. Hosted-tool injection is `false` only for the
   `/responses/compact` path (the compact endpoint accepts no tools).
2. **Provider executes; Tact only renders.** A hosted-tool output item (e.g.
   `web_search_call`) must **never** become a `ContentBlock::ToolUse` (which
   would enter `execute_tool_call`). Terminal stop reason stays `completed`
   (not `tool_use`), so the agent loop finishes normally.
3. **Drive the TUI via real Step events.** In `stream.rs`, emit
   `StepStarted` on `output_item.added` (query may be empty — the provider
   populates `action` later) and `StepFinished`/`StepFailed` on
   `output_item.done` (emitted only on the **first** `done` for an index:
   repeated `done` events overwrite the slot without re-emitting). Map
   hosted-tool status enums exhaustively (for web search, all four
   `WebSearchToolCallStatus` variants: `completed` → success, `failed` →
   failed, and `in_progress`/`searching` at `done` → defensive failure,
   never silent success). Do **not** add dedicated SSE events
   (`response.web_search_call.in_progress/searching/completed`) —
   `output_item.added/done` fully covers the lifecycle; keep the stream
   whitelist minimal. (The dedicated events *do* exist in the official
   OpenAI spec and async-openai, but their payload is only
   `output_index`/`item_id`/`sequence_number` — no query or sources — so
   subscribing to them adds nothing for the tool card; the query/sources
   live on the `WebSearchToolCall.action` at `done`.)
4. **Unified TUI rendering for hosted tools.** Every hosted tool renders
   through the same `ToolWidget` as local tools — same ✓/✗ meta-row symbols,
   same expandable detail card. The only hosted-tool specifics are:
   - map its visual kind so result detail is expandable
     (`kind_from_presentation` fallback: `web_search` →
     `ToolVisualKind::Command`; add new hosted tools there too);
   - have a readable display name (`tool_display_name`: `web_search` →
     `🔍 Web Search`; add new hosted tools there too).
   Do **not** introduce per-tool rendering flags (e.g. a
   `suppress_phase_prefix`-style switch was tried and removed — hosted tools
   keep the standard ✓/✗ meta row).
5. **Query/sources come from the item's `action`** at `done`; the `added`
   event may carry no action. Never require `include` for
   `web_search_call.action.sources` — the provider fills sources in the item
   by default.
6. **Wire compatibility shim.** Some compatible endpoints (e.g. DeepSeek
   Responses) emit `web_search_call` search actions with a `queries` array
   instead of the singular `query` that async-openai 0.41.x models.
   `wire::normalize_web_search_call_query` fills `query` from `queries` only
   for typed parsing; the raw item JSON is preserved verbatim so follow-up
   turns replay the provider's own shape.


## NetWork Proxy

export https_proxy=http://127.0.0.1:7890 http_proxy=http://127.0.0.1:7890 all_proxy=http://127.0.0.1:7890
