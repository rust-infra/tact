# Session Stats Table Layout — Design

Date: 2026-07-24  
Status: Approved for implementation planning  
Related: `docs/token_usage_schema.md` (Session Stats Display), `crates/tact/src/stats.rs`

## Goals

1. Fix misaligned Session Stats end-of-session output, especially the Tool calls
   block that today relies on ad-hoc `writeln!` spacing.
2. Render stats as **two tables**: scalar metrics, then per-tool breakdown.
3. Keep `SessionStats::summary() -> String` and
   `AgentUpdate::SessionStats(String)` unchanged so CLI / TUI / headless exit
   paths keep working.

## Non-goals

- Changing which metrics are collected or persisted.
- Changing zero-value hiding rules for cache / reasoning / empty tool list.
- Color / ANSI styling in the summary string (must stay plain for logs and TUI).
- Using `pretty-table` (rejected: stdout-oriented API; weaker fit for String
  return). Using `tabled` (heavier than needed for this surface).

## Approach

Depend on **comfy-table**. Build two `Table`s inside `summary()`, format via
`Display` (`format!("{table}")`), concatenate with the existing
`── Session Stats ──` banner lines.

## Layout contract

### Banner

```text
── Session Stats ─────────────────────────────
<metrics table>
[<blank line>]
[<optional "Tool calls" label + tools table>]
[optional total/avg tool time rows — see below]
[optional cache / reasoning rows — see below]
─────────────────────────────────────────────
```

Exact placement of total/avg tool time and cache/reasoning: they remain
**scalar Metric/Value rows** in the first table (same visibility rules as
today), not separate free-form lines after the tools table. Observable order
inside the metrics table:

1. Elapsed  
2. LLM API calls  
3. Total LLM time  
4. Prompt chars sent  
5. Response chars rcvd  
6. Thinking blocks  
7. Thinking chars  
8. Compactions  
9. Total tool time *(only if any tool timings recorded)*  
10. Avg tool time *(same condition)*  
11. Cache hit / miss / hit rate *(only if hit or miss > 0)*  
12. Reasoning tokens *(only if > 0)*

When tools exist, insert the tools table **after** Compactions and **before**
Total tool time rows — matching today’s visual grouping (tool list, then
aggregate tool time, then cache). Implementation detail: either (a) split the
metrics table into “before tools” / “after tools” tables with the same
headers, or (b) one metrics table then tools then a small trailing metrics
table. Prefer **(b)** so headers stay clear and order matches current UX.

### Metrics table(s)

| Column | Alignment | Content |
|--------|-----------|---------|
| Metric | Left | Label strings matching today’s wording |
| Value | Right | Same formatting as today (`fmt_duration`, counts, `%.1ms`, `%.1%`) |

Style: UTF8 box preset, **no** colors / attributes. Call `force_no_tty()` (or
equivalent) so width does not depend on ambient terminal size when the string
is built off a TTY.

### Tools table

Shown only when `tool_counts` is non-empty. Sorted by tool name (unchanged).

| Column | Alignment | Content |
|--------|-----------|---------|
| Tool | Left | Name, or `Total` for the summary row |
| Count(s/f) | Right | `n (ok/fail)` |
| Total | Right | Wall time (`Xs` / `Yms` rules unchanged); empty on Total row |
| Avg | Right | Average ms; empty on Total row |

Always print a short plain-text label `Tool calls` above the tools table
(not a library caption).

## Architecture

| Piece | Responsibility |
|-------|----------------|
| Workspace / `crates/tact` dep | Add `comfy-table` |
| `SessionStats::summary` | Build tables + banner; return `String` |
| Small private helpers | Duration / count cell formatting; shared table style setup |
| Call sites | Unchanged (`tact-ui` driver, TUI `AgentUpdate` handler) |

## Testing

- Keep existing `fmt_duration` and `record_token_usage` smoke tests.
- Extend smoke / add a focused test that a populated `SessionStats` summary
  contains metrics header `Metric` and, when tools present, tools header
  `Tool` / `Count(s/f)`.

## Docs sync

- Update the Session Stats Display example in `docs/token_usage_schema.md`.
- Append bilingual Ch 26 entries (`book/26_chapter_issue.md` + `_zh.md`) as
  type `optimization` for the user-visible layout change.

## Out of scope for follow-ups

- Live in-session stats panel.
- CSV / machine-readable export of the same counters.
