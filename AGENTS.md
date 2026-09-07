# Agent guidelines (Tact)

- Prefer small, focused diffs; do not commit unless asked.
- Structure final answers as Markdown lists (one step per line).

## Cargo / tests

- Never run `cargo test` / `build` / `clippy` in parallel against this workspace — they contend on the `target/` lock and can hang for minutes. One invocation at a time, ideally filtered (e.g. `cargo test -p tact --lib voice::`).
- Async tests that wait on channels must use timeouts; never unbounded `recv().await` in new tests.

## Docs: sync at push time, not per edit

Code first, then before pushing diff against the trigger table and sync all touched docs in one pass. Bilingual pairs (`book/*.md` + `*_zh.md`) must stay structurally aligned (same headings/mermaid/tables); update both in one commit for behavioral changes.

| Trigger | Sync |
|---|---|
| Agent loop / compaction / recovery | `book/05_chapter_compact*.md`; skim `ARCHITECTURE.md` §6 / `docs/compaction.md` if drifting |
| Config / CLI flag rename / semantics | Documenting `book/` chapter, `config.example.toml`, relevant spec/plan |
| TUI bottom-bar / token / cache display | `docs/token_usage_schema.md` + book section describing the bar |
| New multi-step feature | Spec `docs/superpowers/specs/YYYY-MM-DD-<topic>-design.md` (after approval) + plan `docs/superpowers/plans/YYYY-MM-DD-<topic>.md` |
| Store / session persistence contracts | `book/01_chapter_store*.md` (+ `docs/token_usage_schema.md` if usage tables change) |
| Shipped optimization / bugfix with user-visible change | Newest-first entry in `book/26_chapter_issue.md` **and** `_zh.md` (date, type, symptom, decision, observable behavior, pointers). Don't replace subsystem chapters |

Skip Ch 26 for pure refactors, test-only, and comment/typo-only edits.

## TUI rendering — no shadow / residue

Root cause: ratatui diffs cells and emits only changed ones, so a cell "restored" but never re-written keeps the old style. When writing/reviewing `crates/tui/src/render/**`:

1. **Every unit paints its own bg** over its full `area` (row tails, blank separators, indent gutters included); never rely on the caller to clear first.
2. **A span's bg only covers its glyph columns** — a patch, not a band. Full-width rows must be painted row-wide, not per-glyph; `wrap_line` re-slices spans, so span bg multiplies across wrapped continuation rows.
3. **"Restore to default" must write the style** — force-emit `CellDiffOption::AlwaysUpdate` for cells that must stay correct when content is unchanged (see `restamp_log_left_border` in `crates/tui/src/render/log.rs`).
4. **Overlays/popups size against their real parent rect** (main area `chunks[1]`, never full frame); their `Clear` rect must cover ≥ their draw rect, from the same geometry.
5. **After wide graphemes (emoji/CJK, width 2)**, the row remainder must still carry base bg.
6. **Every new render unit ships a buffer-level test** asserting blank cells carry `theme.bg` (patterns in `crates/tui/src/render/*_tests.rs`).

Why: `book/26_chapter_issue.md` — 2026-07-27 "Log scroll restores the theme background", 2026-07-28 "Log left-border scrollbar residue", 2026-08-16 heading-band / overlay-popup entries.

## Compaction

Codex-style rebuild: recent real user messages + `SUMMARY_PREFIX` handoff; entry path compacts **before** pushing `user_turn_message`; `should_auto_compact` reserves incoming-turn size. Spec: `docs/superpowers/specs/2026-07-18-codex-style-compact-design.md`. Legacy: `Agent::compact_history_legacy`.

## Hosted tools (Provider-executed)

Hosted tools (web search, …) are a **Responses-protocol capability**, not a per-provider option: the adapter injects hosted `Tool::WebSearch` on every ordinary `/responses` request (OpenAI / DeepSeek / custom OpenAI-compatible endpoints alike), never on `/responses/compact`. Provider executes; Tact only renders. Do not regress:

1. **Inject, never replace** — `native_web_search` only *adds* a hosted tool; never inspect/rewrite tool names. An MCP-provided `web_search` stays `Tool::Function`; both coexist. Injection is off only on `/responses/compact`.
2. **Hosted output never becomes `ContentBlock::ToolUse`** (would enter `execute_tool_call`); terminal stop reason stays `completed`, not `tool_use`.
3. **TUI via real Step events only** — `StepStarted` on `output_item.added` (query may arrive later), `StepFinished`/`StepFailed` on the **first** `done` per index. Map status enums exhaustively (web search: completed→success, failed→failed, in_progress/searching at done→defensive failure, never silent success). No dedicated SSE events.
4. **Unified rendering through `ToolWidget`** — same ✓/✗ meta row, same expandable detail. A new hosted tool = fallback visual kind (`web_search`→`ToolVisualKind::Command`) + readable display name (`🔍 Web Search`). No per-tool flags (a `suppress_phase_prefix`-style switch was tried and removed).
5. **Query/sources come from the item's `action` at `done`** — the `added` event may carry none; never require `include` for sources.
6. **Wire shim** — compatible endpoints may send a `queries[]` array; `wire::normalize_web_search_call_query` fills `query` for typed parsing only; the raw item JSON is preserved verbatim for replay.

## Network proxy (local dev)

```sh
export https_proxy=http://127.0.0.1:7890 http_proxy=http://127.0.0.1:7890 all_proxy=http://127.0.0.1:7890
```
