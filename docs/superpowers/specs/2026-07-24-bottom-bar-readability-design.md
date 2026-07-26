# Bottom Bar Readability — Design

Date: 2026-07-24  
Status: Approved for implementation planning  
Related: `crates/tui/src/render/bar.rs`, `crates/tui/src/i18n.rs`,
`docs/token_usage_schema.md` (TUI display notes), `book/23_chapter_tui*`,
`book/26_chapter_issue*`, prior polish
`docs/superpowers/specs/2026-07-24-bottom-bar-polish-design.md`

## Goals

1. Make the two-row bottom bar **readable without a legend**: short bilingual
   labels next to icons where meaning is not obvious.
2. Disambiguate overlapping token-like numbers (`8K/32K` vs context meter vs
   last-call total) and surface thinking **effort level** with budget.
3. Keep professional TUI density: two rows, same field set (do not silently
   drop account/branch from the wide layout), no Nerd Fonts, no new config flag.
4. Fix ctx progress glyphs so fill characters do not visually overflow `[]`.

## Non-goals

- Changing how token / cache / balance numbers are computed or persisted.
- Collapsing to one row, or restyling the top status bar.
- Requiring patched / Nerd Fonts.
- Adding a settings toggle for “compact vs labeled” in this change.
- Building a detail popup for token breakdown (may come later).

## Motivation

The 2026-07-24 visual polish replaced bilingual labels with narrow Unicode
icons (`◷`, `⊙`, `⎇`, `¤`, `∑`, `▣`) and unified separators to ` · `. Density
improved; **scanability for first-time / casual readers dropped**. Users
could not tell:

- what each icon means;
- that `8K/32K` is max output / thinking budget, not context usage;
- that `∑n` and the context meter numerator are the **same**
  `TokenUsageInfo.total` from the latest main-loop LLM call;
- that `▣pct%` is cache **hit rate**.

Separately, middot separators were too faint, and full-block `█`/`░` bars
often render taller than `[]` brackets.

## Approach

**Readability restore** (chosen over “row-2 only” and “default trim of
uptime/∑”):

- Keep the field set and DropGroup machinery from the polish pass.
- Add short i18n labels beside icons for elapsed / uptime / out / think /
  cache%.
- Split model limits: `输出 {max}` + `思考 {effort}({budget})` (fallback
  without effort).
- Mixed separators: row 1 uses ` │ `; row 2 uses exactly two ASCII spaces between segments.
- Replace progress fill with mid-height `■` / empty `·`.
- Token total icon becomes `∑ₜₒₖ` (U+2211 + subscript t/o/k); keep the
  segment for precise last-call total (still droppable; still same source as
  ctx numerator).

## Target layout (wide terminal, zh)

```text
◷ 耗时 00:02 │ ⊙ 运行 03:43 │ ~/Projects/tact │ ⎇ feat/web │ ¤ CNY 8.16
deepseek-v4-flash  输出 8K  思考 high(32K)  ctx [■■······] 0% 6.6K/1M  ∑ₜₒₖ 6584  ▣ 缓存% 0%
```

English (same structure):

```text
◷ Elapsed 00:02 │ ⊙ Up 03:43 │ ~/Projects/tact │ ⎇ feat/web │ ¤ CNY 8.16
deepseek-v4-flash  out 8K  think high(32K)  ctx [■■······] 0% 6.6K/1M  ∑ₜₒₖ 6584  ▣ cache% 0%
```

## Visual language

### Separators

| Row | Separator | Notes |
|-----|-----------|-------|
| 1 | ` │ ` (space + U+2502 + space) | Environment segments; clearer than ` · ` |
| 2 | exactly two ASCII spaces (`  `) between segments | Usage segments; avoid competing with `│` and with `6.6K/1M` |

Do not use ` · ` as the primary inter-segment separator anymore.

### Icon + label map

| Field | Glyph | ZH label | EN label | Notes |
|-------|-------|----------|----------|-------|
| Elapsed | `◷` | 耗时 | Elapsed | Always with label |
| Uptime | `⊙` | 运行 | Up | Always with label |
| Path | — | — | — | Path string only |
| Branch | `⎇` | — | — | Icon + name |
| Balance / quota | `¤` | — | — | Unchanged short forms from polish |
| Model | — | — | — | Name only |
| Max out | — | 输出 | out | Compact token count |
| Thinking | — | 思考 | think | See format below |
| Context | — | ctx | ctx | Same abbreviation both langs |
| Last total | `∑ₜₒₖ` | — | — | No extra word; icon carries “tokens” |
| Cache hit rate | `▣` | 缓存% | cache% | |

### Thinking format

Prefer effort from `status_bar.model_reasoning_effort` (already populated via
`current_reasoning_effort_from_budget` / explicit config):

- With effort + budget: `思考 high(32K)` / `think high(32K)`
- Budget only (no mappable effort): `思考 32K` / `think 32K`
- No thinking budget: omit the segment

Do **not** show the old combined `8K/32K` glued to the model name.

### Context meter glyphs

Replace `█` / `░` with:

- filled: `■` (U+25A0)
- empty: `·` (U+00B7)

Keep bracketed form `[……]` and existing width (`USAGE_BAR_WIDTH`). Apply the
same glyphs anywhere this helper is shared (bottom-bar ctx meter; if
`render_usage_bar` is reused elsewhere in the bar module, keep one glyph
pair).

### Colors

Reuse polish roles: dim icons/separators, primary values, accent branch,
success/error balance, meter readable on `bottom_bar_bg`. No new `Theme`
fields.

## Data semantics (unchanged computation)

| Display | Source |
|---------|--------|
| ctx `used/window` and `∑ₜₒₖ n` | `status_bar.token_total` ← latest `AgentUpdate::TokenUsage.total` (= provider `total_tokens`, typically prompt+completion) |
| ctx window denom | `app.model_context_window` |
| cache% | `hit / (hit+miss)` from latest usage; `--` when both zero |
| out / think budget | `model_max_tokens` / `model_thinking_budget` |
| think effort | `model_reasoning_effort` |

Document in `docs/token_usage_schema.md` that the bar shows **last-call**
totals for meter + `∑ₜₒₖ`, and that those two share one source.

## Narrow-width drop order

While Unicode width exceeds `area.width`, drop droppable segments in this
order (row-local DropGroups, same helper as today):

1. Cache% (`▣ …`)
2. Uptime (`⊙ 运行 …`)
3. Path segment (drop whole cwd group; basename shortening is out of scope)
4. `∑ₜₒₖ …`
5. Context meter segment (`ctx [..] pct used/window`)
6. Prefer keeping: elapsed, branch, balance, model, out, think

Never mid-glyph clip; prefer dropping whole segments.

## Implementation touchpoints

| Area | Change |
|------|--------|
| `crates/tui/src/render/bar.rs` | Separators; `format_model_*` split out/think; wire `model_reasoning_effort`; `∑ₜₒₖ`; `■`/`·` bar; row2 spacing; update unit tests |
| `crates/tui/src/i18n.rs` | Short labels: elapsed/uptime/out/think/cache% (zh+en) |
| Docs | `docs/token_usage_schema.md` TUI notes; `book/23_chapter_tui*` bottom bar; Ch 26 EN+ZH issue entry for user-visible change |

## Testing

### Automated

- Formatters: thinking with/without effort; cache% strings; bar glyphs
  `[■■··]` at 0/50/100%; `∑ₜₒₖ` present in wide layout.
- Render tests: zh labels on row1; row1 contains `│`; row2 lacks middot
  primary separators; narrow width drops cache before elapsed.
- `unicode_width` of `∑ₜₒₖ`: assert and document; if a platform reports
  pathological width, acceptable fallback in a follow-up is `∑_tok` (not in
  this change unless tests fail in CI).

### Manual

- Solarized dark + one light theme: brackets visually contain `■`/`·`.
- ~80 columns: drop order matches list; no third row.
- EN ↔ 中文: no leftover wrong-language labels.
- Model with effort `high` and budget 32K shows `high(32K)`.

## Decisions log

| Decision | Choice |
|----------|--------|
| Overall approach | Readability restore (full two-row labels), not row2-only / default trim |
| Row1 separator | ` │ ` |
| Row2 separator | Widened spaces |
| Label style | Icon + short label (branch/balance icon-only) |
| Thinking display | Level + budget `high(32K)` |
| Cache label | `缓存%` / `cache%` |
| Last-call total | Keep segment as `∑ₜₒₖ n` |
| Progress fill | `■` / `·` (not `█` / `░`) |
| Config / Nerd Font | None |

## Relationship to prior polish spec

This design **supersedes** the polish spec’s choices of middot-only
separators, icon-only primary fields, combined `8k/32k` on the model name,
and `█`/`░` meters for the bottom bar. Span color hierarchy, two-row
layout, balance short forms, and DropGroup fitting remain.
