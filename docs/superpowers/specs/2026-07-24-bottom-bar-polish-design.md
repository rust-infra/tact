# Bottom Bar Visual Polish — Design

Date: 2026-07-24  
Status: Approved for implementation planning  
Related: `crates/tui/src/render/bar.rs`, `crates/tui/src/i18n.rs`, `crates/tui/src/theme.rs`, `docs/tui_rendering.md`, `docs/token_usage_schema.md` (TUI cache display)

## Goals

1. Make the two-row bottom status bar look like a professional TUI: consistent
   separators, narrow Unicode icons (no emoji), and clear color hierarchy.
2. Light declutter without removing the information architecture: shorten
   labels, collapse verbose balance/quota strings, compact model `max` /
   `think` into `8k/32k`.
3. Keep two rows and the same field set (elapsed, uptime, path, branch,
   balance/quota, model, context meter, tokens, cache hit %).

## Non-goals

- Collapsing to a single row, or swapping row roles (context vs usage).
- Adding a config/CLI flag to toggle the new style.
- Requiring Nerd Fonts or private-use glyphs.
- Changing how token/cache numbers are computed or persisted.
- Restyling the top status bar in this change.

## Motivation

Current bar mixes emoji, long bilingual labels (`Elapsed:`, `Balance:`,
`cache hit:`), and mixed separators (`│` / `|`). Both rows use a single
`Paragraph` style, so hierarchy is flat and hard to scan. Primary pain is
**visual polish**, not missing data or vertical height.

## Approach

**Polish + Span color hierarchy** (chosen over typography-only polish and
over a full row-role redesign):

1. Replace emoji with a fixed Unicode icon set.
2. Unify separators to ` · `.
3. Shorten labels and collapse secondary balance fields.
4. Render with ratatui `Line` / `Span` segments: dim icons & separators,
   bright primary values, accent branch, success/error balance, meter tint
   for the context bar.

## Visual language

### Target layout (wide terminal)

```text
◷ 00:03 · ⊙ 00:58 · ~/Projects/tact · ⎇ feat/web · ¤ CNY 9.60
deepseek-v4-flash 8k/32k · [░░░░░░░░░░] 0% 6.6K/1M · ∑6612 · ▣8%
```

### Icon map

| Meaning | Glyph | Notes |
|---------|-------|-------|
| Task elapsed | `◷` | Keep `Elapsed`/`耗时` label out of the string |
| Process uptime | `⊙` | |
| Git branch | `⎇` | Path has no icon |
| Balance / quota | `¤` | |
| Tokens | `∑` | |
| Cache hit % | `▣` | |

### Separators

- Between segments: ` · ` (space-middot-space) only.
- Do not mix `│`, `|`, or bare spaces as primary separators.

### Color roles (map to existing `Theme`)

| Role | Typical content | Theme source |
|------|-----------------|--------------|
| Bar background | row bg | `bottom_bar_bg` |
| Dim | icons, ` · ` | `theme.muted_fg()` |
| Primary | elapsed, model name | brighter than `bottom_bar_fg` (use `fg` or theme highlight if needed; do not invent new Theme fields) |
| Secondary | uptime, path, token/cache numbers | `bottom_bar_fg` |
| Accent | branch (`⎇ name`) | `accent` |
| Success / error | balance available vs not | `success` / `error` |
| Meter | context usage bar + pct | `accent` (same family as branch; distinguishable via `[█░]` shape) |

No new theme config keys.

## Content rules (light trim)

### Row 1 — context

| Field | Format |
|-------|--------|
| Elapsed | `◷ MM:SS` (or `--:--` when idle / no last prompt) |
| Uptime | `⊙ MM:SS` (or longer forms already used today when hours/days) |
| Workspace | path string as today (tilde form ok) |
| Branch | `⎇ {branch}` or unknown placeholder from i18n |
| Account | see below |

**Balance (default):** for each currency entry show `¤ {currency} {total}`
(e.g. `¤ CNY 9.60`); multiple currencies joined with ` · `. Drop `grant=` /
`topup=` / `Balance:` / `余额:` / availability emoji from the bar — availability
is color only (`success` / `error`).

**Quota (when no balance):** short form `¤ {label} {pct}%` when pct exists,
else `¤ {label} {remaining}/{limit}`. Multiple windows joined with ` · `.
No `Quota:` banner; availability is color only.

### Row 2 — usage

| Field | Format |
|-------|--------|
| Model | `{name}` then compact limits `8k/32k` when max/think present (`format_tokens_compact`-style). Optional short effort (`med`) only if it fits; omit when truncating. |
| Context meter | keep `[████…] pct used/window` compact form |
| Tokens | default `∑{total}` only (no `Tok:` / `(p…)+(c…)` in the default wide layout) |
| Cache | `▣{pct}%` or `▣--` before first cache sample |

## Narrow-width drop order

Build each row as an ordered list of segments. While Unicode width exceeds
`area.width`, drop in this order:

1. Cache (`▣…`)
2. Uptime (`⊙…`)
3. Shorten path to basename
4. Tokens already default to ∑-only; if still over, drop ∑ segment
5. Truncate model name last

Balance/quota remains high priority (same spirit as today’s
`append_account_suffix`). Prefer dropping optional segments over mid-glyph
clipping. Extend the current width-aware helper rather than inventing a
second truncation system.

## Implementation touchpoints

| Area | Change |
|------|--------|
| `render/bar.rs` | `render_bottom_bar` → build `Vec<Span>` / `Line`; extract pure helpers for compact model, short balance, segment drop |
| `i18n.rs` | Update `bottom_top_tmpl`, `bottom_mid_tmpl`, `bottom_cache_tmpl`, balance/usage templates for EN + 中文; icons may live as constants in `bar.rs` if language-invariant |
| `theme.rs` | Reuse only; no new fields required |
| Docs | `docs/tui_rendering.md` Bottom Bar section; `docs/token_usage_schema.md` TUI cache display note; Ch 26 issue entry (EN + ZH) for user-visible bar change |

## Testing

### Automated

- Unit-test pure formatters: icon/separator assembly, balance short form,
  `8k/32k` compression, drop-order given a width budget → expected plain
  string (ignore colors in string asserts, or assert span texts in order).
- Update any `render/*_tests.rs` expectations that snapshot bottom-bar text.

### Manual acceptance

- Solarized Dark + one light theme: two rows, no emoji, unified ` · `,
  branch/balance colors readable.
- ~80-column width: drop order matches above; no third row / broken glyphs.
- Balance unavailable / no account / quota-only: short form + colors correct.
- EN ↔ 中文: no leftover long English labels on Chinese UI.

## Decisions log

| Decision | Choice |
|----------|--------|
| Pain focus | Visual polish (not density-first or height-first) |
| Scope | Polish + light trim (not full IA redesign) |
| Icons | Unicode symbols (not emoji, not text-only) |
| Approach | Span color hierarchy on top of typographic polish |
| Rows | Keep two; keep current row roles |
| Config | No toggle |
