# Bottom Bar Readability — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Restore readable short labels on the two-row bottom bar, surface thinking effort as `high(32K)`, use mixed separators, `缓存%/cache%`, `∑ₜₒₖ`, and mid-height `■`/`·` progress glyphs — per `docs/superpowers/specs/2026-07-24-bottom-bar-readability-design.md`.

**Architecture:** (1) Add i18n short labels + pure formatters with unit tests; (2) rewire `render_bottom_bar` DropGroups (row1 ` │ `, row2 two spaces, split model/out/think, new icons/glyphs); (3) update render assertions; (4) sync docs / Ch 26.

**Tech Stack:** Rust, ratatui `Span`/`Line`, `unicode-width`, existing `Messages` i18n.

**Spec:** `docs/superpowers/specs/2026-07-24-bottom-bar-readability-design.md`

## File map

| File | Responsibility |
|------|----------------|
| `crates/tui/src/i18n.rs` | Short labels: elapsed / uptime / out / think / cache% / ctx (zh+en) |
| `crates/tui/src/render/bar.rs` | Icons, formatters, `render_usage_bar` glyphs, `render_bottom_bar` layout + tests |
| `docs/token_usage_schema.md` | TUI display note: last-call total shared by ctx meter + `∑ₜₒₖ`; cache% label |
| `book/23_chapter_tui.md` + `_zh.md` | Bottom bar field description |
| `book/26_chapter_issue.md` + `_zh.md` | Newest-first changelog entry |

## Global constraints

- No new `Theme` fields; no Nerd Fonts; no config flag.
- Do not change token/cache computation or persistence.
- Leave top-bar `render_progress_bar` (`█`/`░`) unchanged — only `render_usage_bar` (ctx meter) switches to `■`/`·`.
- `docs/superpowers/` is gitignored — use `git add -f` for plan/spec/docs under that tree if committing them; book/ and `docs/token_usage_schema.md` are normal tracked paths.

---

### Task 1: i18n labels + pure formatters (TDD)

**Files:**
- Modify: `crates/tui/src/i18n.rs`
- Modify: `crates/tui/src/render/bar.rs`

- [ ] **Step 1: Add failing unit tests for new formatters**

In `crates/tui/src/render/bar.rs` `#[cfg(test)]` module, add (and keep old tests until Step 4 removes/replaces them):

```rust
#[test]
fn render_usage_bar_uses_mid_height_glyphs() {
    assert_eq!(super::render_usage_bar(0.0), "[········]");
    assert_eq!(super::render_usage_bar(50.0), "[■■■■····]");
    assert_eq!(super::render_usage_bar(100.0), "[■■■■■■■■]");
}

#[test]
fn format_out_tokens_labeled() {
    assert_eq!(super::format_out_tokens("输出", 8_000), "输出 8K");
    assert_eq!(super::format_out_tokens("out", 8_000), "out 8K");
}

#[test]
fn format_think_with_effort_and_budget() {
    assert_eq!(
        super::format_think_segment("思考", Some("high"), Some(32_000)),
        Some("思考 high(32K)".into())
    );
    assert_eq!(
        super::format_think_segment("think", Some("medium"), Some(8_000)),
        Some("think medium(8K)".into())
    );
}

#[test]
fn format_think_budget_only() {
    assert_eq!(
        super::format_think_segment("思考", None, Some(32_000)),
        Some("思考 32K".into())
    );
}

#[test]
fn format_think_omitted_without_budget() {
    assert_eq!(super::format_think_segment("think", Some("high"), None), None);
    assert_eq!(super::format_think_segment("think", None, Some(0)), None);
    assert_eq!(super::format_think_segment("think", None, None), None);
}

#[test]
fn format_cache_pct_with_label() {
    assert_eq!(super::format_cache_pct(0, 0, "缓存%"), "▣ 缓存% --");
    assert_eq!(super::format_cache_pct(30, 70, "cache%"), "▣ cache% 30%");
    assert_eq!(super::format_cache_pct(100, 0, "缓存%"), "▣ 缓存% 100%");
}

#[test]
fn format_context_meter_labeled() {
    let s = super::format_context_meter("ctx", 0, 1_000_000);
    assert!(s.starts_with("ctx ["), "got {s}");
    assert!(s.contains("0%"), "got {s}");
    assert!(s.contains("0/1M"), "got {s}");
    assert!(!s.contains('█') && !s.contains('░'), "old glyphs present: {s}");
    assert!(!s.contains('·') || s.contains('[') , "meter should use · inside brackets");
}

#[test]
fn format_token_total_icon() {
    assert_eq!(super::format_token_total(6584), "∑ₜₒₖ 6584");
}

#[test]
fn sigma_tok_unicode_width_is_sane() {
    let w = unicode_width::UnicodeWidthStr::width(super::ICON_TOKENS);
    assert!(
        (1..=8).contains(&w),
        "∑ₜₒₖ width {w} looks pathological; consider ∑_tok fallback"
    );
}
```

- [ ] **Step 2: Run tests — expect FAIL**

```bash
cargo test -p tact-tui render_usage_bar_uses_mid_height_glyphs format_out_tokens_labeled format_think_with_effort_and_budget format_cache_pct_with_label format_token_total_icon -- --nocapture
```

Expected: compile errors and/or FAIL (missing symbols / old signatures).

- [ ] **Step 3: Add i18n fields**

In `Messages` (near other `bottom_*` fields):

```rust
pub bottom_elapsed: &'static str,
pub bottom_uptime: &'static str,
pub bottom_out: &'static str,
pub bottom_think: &'static str,
pub bottom_ctx: &'static str,
pub bottom_cache_pct: &'static str,
```

`english()`:

```rust
bottom_elapsed: "Elapsed",
bottom_uptime: "Up",
bottom_out: "out",
bottom_think: "think",
bottom_ctx: "ctx",
bottom_cache_pct: "cache%",
```

`chinese()`:

```rust
bottom_elapsed: "耗时",
bottom_uptime: "运行",
bottom_out: "输出",
bottom_think: "思考",
bottom_ctx: "ctx",
bottom_cache_pct: "缓存%",
```

Keep `bottom_branch_unknown` / `bottom_model_unknown` / `bottom_tips_log` unchanged.

- [ ] **Step 4: Implement formatters in `bar.rs`**

Replace icon constant and helpers:

```rust
const ICON_TOKENS: &str = "∑ₜₒₖ"; // U+2211 + U+209C U+2092 U+2096
const SEP_ROW1: &str = " │ ";
const SEP_ROW2: &str = "  ";
const BAR_FILLED: char = '■'; // U+25A0
const BAR_EMPTY: char = '·';  // U+00B7
```

Update `render_usage_bar` to push `BAR_FILLED` / `BAR_EMPTY` instead of `█` / `░`.

Replace `format_model_compact` usage path with:

```rust
fn format_model_name(name: &str) -> String {
    if name.is_empty() {
        "-".to_string()
    } else {
        name.to_string()
    }
}

fn format_out_tokens(label: &str, max_tokens: u32) -> Option<String> {
    if max_tokens == 0 {
        None
    } else {
        Some(format!("{label} {}", format_tokens_compact(max_tokens as u64)))
    }
}

fn format_think_segment(
    label: &str,
    effort: Option<&str>,
    budget: Option<u32>,
) -> Option<String> {
    let budget = budget.filter(|b| *b > 0)?;
    let b = format_tokens_compact(budget as u64);
    match effort.filter(|e| !e.is_empty()) {
        Some(level) => Some(format!("{label} {level}({b})")),
        None => Some(format!("{label} {b}")),
    }
}

fn format_cache_pct(hit: u64, miss: u64, label: &str) -> String {
    let total = hit + miss;
    if total == 0 {
        format!("{ICON_CACHE} {label} --")
    } else {
        let pct = hit.saturating_mul(100).checked_div(total).unwrap_or(0);
        format!("{ICON_CACHE} {label} {pct}%")
    }
}

fn format_context_meter(label: &str, used: u32, window: usize) -> String {
    let pct = context_usage_pct(used, window);
    let bar = render_usage_bar(pct as f64);
    format!(
        "{label} {bar} {pct}% {}/{}",
        format_tokens_compact(used as u64),
        format_tokens_compact(window as u64)
    )
}

fn format_token_total(total: u32) -> String {
    format!("{ICON_TOKENS} {total}")
}
```

Delete or stop calling `format_model_compact` and the old `format_context_meter_new` / 3-arg-less `format_cache_pct`. Update any unit tests that still expect `8K/32K`, `▣30%`, `[████░░░░]`, or `∑42`.

- [ ] **Step 5: Run formatter unit tests — expect PASS**

```bash
cargo test -p tact-tui --lib render::bar::tests -- --nocapture
```

Expected: PASS for new formatter tests; fix any leftover old assertions in the same module.

- [ ] **Step 6: Commit**

```bash
git add crates/tui/src/i18n.rs crates/tui/src/render/bar.rs
git commit -m "$(cat <<'EOF'
feat(tui): add readable bottom-bar formatters and i18n labels

EOF
)"
```

---

### Task 2: Rewire `render_bottom_bar`

**Files:**
- Modify: `crates/tui/src/render/bar.rs` (`render_bottom_bar` only for this task’s behavior; tests in Task 3)

- [ ] **Step 1: Rewrite row1 groups**

Use `SEP_ROW1` (` │ `) between segments. Pattern for elapsed:

```rust
DropGroup {
    droppable: false,
    spans: vec![
        Span::styled(ICON_ELAPSED.to_string(), dim),
        Span::styled(format!(" {} {}", msgs.bottom_elapsed, elapsed), primary),
        Span::styled(SEP_ROW1.to_string(), dim),
    ],
},
```

Uptime: same with `ICON_UPTIME` + `msgs.bottom_uptime`, `droppable: true`.

Path: `format!("{}{}", app.workspace_dir, "")` then append `SEP_ROW1` when not last before branch — keep current join style but replace ` · ` with `SEP_ROW1`.

Branch: `⎇` + name, accent; no text label.

Account: keep `build_account_spans`; prefix with `SEP_ROW1` instead of ` · `.

Focus `[Log]` stays non-droppable first segment + `SEP_ROW1`.

- [ ] **Step 2: Rewrite row2 groups**

Build ordered DropGroups (separators = leading `SEP_ROW2` on every group after the first):

1. Model name — `format_model_name`, not droppable  
2. Out — `format_out_tokens(msgs.bottom_out, max)` if `Some`, droppable: false (keep with model)  
3. Think — `format_think_segment(msgs.bottom_think, effort.as_deref(), budget)`, droppable: false  
4. Context meter — `format_context_meter(msgs.bottom_ctx, token_total, window)`, droppable: true  
5. Token total — `format_token_total(token_total)`, droppable: true  
6. Cache — `format_cache_pct(..., msgs.bottom_cache_pct)`, droppable: true  

Wire effort:

```rust
let effort = app.status_bar.model_reasoning_effort.as_deref();
let budget = app.status_bar.model_thinking_budget;
```

Drop order is determined by **which groups are marked droppable and their order from the end** via existing `fit_row_spans` (removes last droppable first). To match spec order (cache → uptime → path → ∑ → ctx), ensure:

- Row1 droppable from end: uptime group before path group in the vec so path is removed after uptime when both droppable… **Actually** `rposition` removes the **last** droppable in the vec first.

So for row1, order groups so the last droppable is cache-equivalent… Row1 droppables: path and uptime. Spec: drop uptime before path → uptime must appear **after** path in the vec (so it is the last droppable). Current polish already has path then uptime — **verify**: polish comment said “uptime dropped before path” but listed path then uptime with both droppable → last droppable is uptime → removed first. Keep path then uptime.

Row2: order droppables so last-to-first removal is cache, then ∑, then ctx:

```text
[model][out][think][ctx droppable][sum droppable][cache droppable]
```

`rposition` removes cache first, then sum, then ctx. Matches spec items 1,4,5 (uptime/path are row1).

- [ ] **Step 3: Manual compile check**

```bash
cargo test -p tact-tui --lib render::bar -- --nocapture
```

Expected: may FAIL on outdated render assertions — that is OK; fix in Task 3. Must compile.

- [ ] **Step 4: Commit**

```bash
git add crates/tui/src/render/bar.rs
git commit -m "$(cat <<'EOF'
feat(tui): rewire bottom bar layout for readability

EOF
)"
```

---

### Task 3: Update render / integration assertions

**Files:**
- Modify: `crates/tui/src/render/bar.rs` (`#[cfg(test)]` render tests)
- Modify: any other test under `crates/tui` that snapshots bottom-bar text containing `∑`, `▣` without label, `8K/32K`, or ` · `

- [ ] **Step 1: Update `bottom_bar_shows_compact_model_with_limits`**

Replace expectation of `128K/32K` with separate out/think when effort is set:

```rust
app.status_bar.model_max_tokens = 128_000;
app.status_bar.model_thinking_budget = Some(32_000);
app.status_bar.model_reasoning_effort = Some("high".into());
// ...
assert!(
    text.contains("mock-model")
        && text.contains("out 128K")
        && text.contains("think high(32K)"),
    "got:\n{text}"
);
```

For Chinese UI tests (if any set lang), use `输出` / `思考`. Default test app language: check `make_app()` — if English, use EN labels above.

- [ ] **Step 2: Update token / cache / meter assertions**

- `∑42` → `∑ₜₒₖ 42`  
- `▣` bare pct → contains `cache%` or `缓存%` depending on lang  
- meter: contains `ctx [` and `■` or `·`, not `█`  
- row1: contains `│`, and `Elapsed` or `耗时`  
- row1: must **not** use ` · ` as primary separator between elapsed and uptime  

- [ ] **Step 3: Add narrow-width drop smoke test**

```rust
#[test]
fn bottom_bar_drops_cache_before_model_on_narrow_width() {
    let mut app = make_app();
    app.status_bar.model_name = "mock-model".into();
    app.status_bar.model_max_tokens = 8_000;
    app.status_bar.model_thinking_budget = Some(32_000);
    app.status_bar.model_reasoning_effort = Some("high".into());
    app.status_bar.token_total = 100;
    app.status_bar.token_cache_hit = 50;
    app.status_bar.token_cache_miss = 50;
    app.model_context_window = 200_000;

    let backend = TestBackend::new(40, 2);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| render_bottom_bar(f, Rect::new(0, 0, 40, 2), &app))
        .unwrap();
    let text = buffer_text(terminal.backend().buffer());
    assert!(
        text.contains("mock-model"),
        "model should remain, got:\n{text}"
    );
    assert!(
        !text.contains("cache%") && !text.contains("缓存%"),
        "cache segment should drop first on narrow width, got:\n{text}"
    );
}
```

Tune width `40` if flake; increase/decrease until cache drops but model remains.

- [ ] **Step 4: Run full tui tests**

```bash
cargo test -p tact-tui -- --nocapture
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/tui/src/render/bar.rs
git commit -m "$(cat <<'EOF'
test(tui): update bottom bar assertions for readability layout

EOF
)"
```

---

### Task 4: Docs sync

**Files:**
- Modify: `docs/token_usage_schema.md` (TUI cache / display paragraph ~line 170)
- Modify: `book/23_chapter_tui.md` § bottom bar (~line 290)
- Modify: `book/23_chapter_tui_zh.md` matching section
- Modify: `book/26_chapter_issue.md` (newest-first entry)
- Modify: `book/26_chapter_issue_zh.md` (same structure)

- [ ] **Step 1: Update `docs/token_usage_schema.md`**

Replace the TUI cache display bullet with:

```markdown
**TUI bottom-bar usage display:** The second row shows:

- **Context meter** — `ctx [■■··] pct used/window`, where `used` is the latest
  main-loop `TokenUsageInfo.total` and `window` is `model_context_window`.
- **Last-call total** — `∑ₜₒₖ {total}` from the **same** `TokenUsageInfo.total`
  (precise integer; droppable when narrow).
- **Cache hit rate** — `▣ 缓存%` / `▣ cache%` plus `pct%` or `--`, from
  `prompt_cache_hit_tokens / (hit + miss)` on that latest call. Counts cover the
  entire prompt (system, tools, history), not only the latest user message.
```

- [ ] **Step 2: Update Ch 23 EN + ZH**

EN example:

```markdown
**Bottom bar** (`render_bottom_bar`, always 2 rows):
- Row 1: `[Log]`, elapsed (`◷ 耗时`/`Elapsed`), uptime (`⊙ 运行`/`Up`), cwd,
  git branch (`⎇`), optional account (`¤ …`). Segments joined with ` │ `.
- Row 2: model name, `输出`/`out`, `思考 high(32K)`/`think …`, `ctx` meter with
  `■`/`·` fill, `∑ₜₒₖ` last-call total, `▣ 缓存%`/`cache%`. Segments joined with
  two spaces. Narrow terminals drop cache → uptime → path → ∑ → ctx first.
```

Mirror in `_zh.md` with the same heading hierarchy.

- [ ] **Step 3: Ch 26 EN + ZH entry (newest first)**

```markdown
### 2026-07-24 — Bottom bar readability restore

**Type:** optimization  
**PR:** (link when opened)

**Symptom / motivation:** After the icon-only polish, the bottom bar was hard to
decode (`8K/32K`, `∑`, `▣`, faint ` · ` separators).

**Decision:** Short i18n labels beside icons; thinking shows effort+budget;
row1 ` │ ` / row2 double-space; cache as `缓存%`/`cache%`; token total `∑ₜₒₖ`;
ctx meter uses `■`/`·` inside `[]`.

**Behavior after:** Readable two-row bar without a legend; same underlying
token/cache numbers.

| Pointer | Path |
|---------|------|
| Spec | `docs/superpowers/specs/2026-07-24-bottom-bar-readability-design.md` |
| Plan | `docs/superpowers/plans/2026-07-24-bottom-bar-readability.md` |
| Code | `crates/tui/src/render/bar.rs`, `crates/tui/src/i18n.rs` |
```

Same section id/hierarchy in `_zh.md`.

- [ ] **Step 4: Commit**

```bash
git add docs/token_usage_schema.md book/23_chapter_tui.md book/23_chapter_tui_zh.md book/26_chapter_issue.md book/26_chapter_issue_zh.md
git add -f docs/superpowers/plans/2026-07-24-bottom-bar-readability.md
git commit -m "$(cat <<'EOF'
docs: sync bottom bar readability across schema and book

EOF
)"
```

---

## Spec coverage checklist

| Spec requirement | Task |
|------------------|------|
| Icon + short labels (elapsed/uptime/out/think/cache%) | 1, 2 |
| Thinking `high(32K)` + budget-only fallback | 1, 2 |
| Row1 ` │ ` / row2 two spaces | 2 |
| `缓存%` / `cache%` | 1, 2 |
| `∑ₜₒₖ` | 1, 2 |
| `■`/`·` in ctx `[]` | 1 |
| Drop order cache → uptime → path → ∑ → ctx | 2, 3 |
| No compute/persist changes | (all) |
| Docs + Ch 26 | 4 |
| Top bar progress glyphs unchanged | 2 note |

## Plan self-review

- No TBD/placeholder steps; commands and code are concrete.
- `format_cache_pct` / `format_context_meter` signatures consistent across tasks.
- `ICON_TOKENS` width assertion included; fallback only if CI fails.
- `make_app()` language: Task 3 must match actual default (EN vs ZH) when asserting label strings — inspect `make_app` before locking asserts.
