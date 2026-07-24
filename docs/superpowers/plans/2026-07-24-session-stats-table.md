# Session Stats Table Layout — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Reformat end-of-session `SessionStats::summary()` with comfy-table into a metrics table plus a tools table, without changing the `String` / `AgentUpdate` contract.

**Architecture:** Add `comfy-table` to the workspace and `tact` crate. Rewrite `summary()` to emit banner + head metrics table + optional tools table + trailing metrics table (total/avg tool time, cache, reasoning). Plain UTF8 boxes, no ANSI colors, `force_no_tty()`.

**Tech Stack:** Rust, comfy-table

**Spec:** `docs/superpowers/specs/2026-07-24-session-stats-table-design.md`

---

## File map

| File | Role |
|------|------|
| Modify `Cargo.toml` | Workspace dep `comfy-table = "7"` |
| Modify `crates/tact/Cargo.toml` | `comfy-table = { workspace = true }` |
| Modify `crates/tact/src/stats.rs` | Helpers + rewrite `summary()` + tests |
| Modify `docs/token_usage_schema.md` | Session Stats Display example |
| Modify `book/26_chapter_issue.md` + `_zh.md` | Newest-first optimization entry |

---

### Task 1: Dependency + failing summary shape test

**Files:**
- Modify: `Cargo.toml`, `crates/tact/Cargo.toml`, `crates/tact/src/stats.rs`

- [ ] **Step 1: Add dependency**

In workspace `Cargo.toml` under `[workspace.dependencies]`:

```toml
comfy-table = "7"
```

In `crates/tact/Cargo.toml` under `[dependencies]`:

```toml
comfy-table = { workspace = true }
```

- [ ] **Step 2: Write failing test for table headers**

Append to `crates/tact/src/stats.rs` tests module:

```rust
#[test]
fn summary_uses_metric_and_tool_tables() {
    let mut s = SessionStats::default();
    s.prompt_count = 1;
    s.tool_counts.insert("bash".into(), 2);
    s.tool_success_counts.insert("bash".into(), 2);
    s.tool_failure_counts.insert("bash".into(), 0);
    s.tool_total_durations_ms.insert("bash".into(), 1500);
    s.tool_timing_counts.insert("bash".into(), 2);
    s.tool_durations_ms.extend([1000, 500]);

    let text = s.summary();
    assert!(text.contains("Metric"), "missing metrics header:\n{text}");
    assert!(text.contains("Value"), "missing metrics Value header:\n{text}");
    assert!(text.contains("Tool calls"), "missing Tool calls label:\n{text}");
    assert!(text.contains("Count(s/f)"), "missing tools Count header:\n{text}");
    assert!(text.contains("bash"), "missing tool row:\n{text}");
    assert!(text.contains("Total"), "missing Total row or Total column:\n{text}");
}
```

- [ ] **Step 3: Run test — expect FAIL**

Run: `cargo test -p tact --lib stats::tests::summary_uses_metric_and_tool_tables`

Expected: FAIL (old `writeln!` layout has no `Metric` / `Count(s/f)` headers)

- [ ] **Step 4: Commit deps + failing test**

```bash
git add -f Cargo.toml crates/tact/Cargo.toml crates/tact/src/stats.rs
git commit -m "test: expect Session Stats comfy-table headers"
```

---

### Task 2: Implement `summary()` with comfy-table

**Files:**
- Modify: `crates/tact/src/stats.rs`

- [ ] **Step 1: Add helpers above `impl SessionStats`**

```rust
use comfy_table::{Attribute, Cell, CellAlignment, ContentArrangement, Table};
use comfy_table::presets::UTF8_FULL;

fn new_stats_table() -> Table {
    let mut table = Table::new();
    table
        .load_preset(UTF8_FULL)
        .set_content_arrangement(ContentArrangement::Disabled)
        .force_no_tty();
    table
}

fn fmt_tool_wall_ms(total_ms: u64) -> String {
    if total_ms >= 1000 {
        format!("{:.1}s", total_ms as f64 / 1000.0)
    } else {
        format!("{total_ms}ms")
    }
}

fn fmt_count_sf(count: u64, success: u64, failure: u64) -> String {
    format!("{count} ({success}/{failure})")
}
```

(Do not use `Attribute`/colors on cells.)

- [ ] **Step 2: Rewrite `summary()` per spec layout (b)**

Structure:

1. Banner line  
2. Head metrics table: Elapsed … Compactions  
3. If tools non-empty: blank line, `Tool calls`, tools table (Total row then sorted tools)  
4. Trailing metrics table (only if any of: tool timings, cache, reasoning): Total/Avg tool time, cache lines, reasoning  
5. Closing banner  

Head metrics builder sketch:

```rust
let mut head = new_stats_table();
head.set_header(vec!["Metric", "Value"]);
head.add_row(vec![
    Cell::new("Elapsed"),
    Cell::new(fmt_duration(self.start_time.elapsed())).set_alignment(CellAlignment::Right),
]);
// … LLM API calls, Total LLM time, prompt/response chars, thinking, compactions
// Right-align the Value column via column_mut(1) after rows if preferred
```

Tools table:

```rust
let mut tools = new_stats_table();
tools.set_header(vec!["Tool", "Count(s/f)", "Total", "Avg"]);
// Total row: Count(s/f) filled; Total/Avg empty strings
// Per-tool rows: sorted by name; Avg as "{avg_ms:.0}ms"
```

Trailing metrics: same header `Metric`/`Value`, only rows that apply.

Right-align numeric columns:

```rust
if let Some(col) = head.column_mut(1) {
    col.set_cell_alignment(CellAlignment::Right);
}
if let Some(col) = tools.column_mut(1) {
    col.set_cell_alignment(CellAlignment::Right);
}
// columns 2 and 3 likewise for tools
```

Assemble:

```rust
let mut out = String::new();
let _ = writeln!(out, "── Session Stats ─────────────────────────────");
let _ = writeln!(out, "{head}");
if has_tools {
    let _ = writeln!(out);
    let _ = writeln!(out, "Tool calls");
    let _ = writeln!(out, "{tools}");
}
if has_trailing {
    let _ = writeln!(out, "{trail}");
}
let _ = writeln!(out, "─────────────────────────────────────────────");
out
```

Keep label strings identical to today (`Prompt chars sent`, `Response chars rcvd`, etc.).

- [ ] **Step 3: Run tests**

Run: `cargo test -p tact --lib stats::`

Expected: all PASS including `summary_uses_metric_and_tool_tables`

- [ ] **Step 4: Commit**

```bash
git add crates/tact/src/stats.rs
git commit -m "feat: render Session Stats with comfy-table"
```

---

### Task 3: Docs sync

**Files:**
- Modify: `docs/token_usage_schema.md`
- Modify: `book/26_chapter_issue.md`, `book/26_chapter_issue_zh.md`

- [ ] **Step 1: Update Session Stats Display example**

Replace the fenced example under `## Session Stats Display` with a two-table sketch matching the new layout (Metric/Value + Tool calls table). Note that cache/reasoning lines remain conditional.

- [ ] **Step 2: Append Ch 26 EN + ZH entries (newest-first after Purpose)**

English fields: Date `2026-07-24`, type `optimization`, Spec path, Symptom (space-aligned tool rows), Decision (comfy-table two-table layout), Behavior after, Pointers (`stats.rs`, token_usage_schema).

Chinese mirror with same section id / hierarchy.

- [ ] **Step 3: Commit**

```bash
git add -f docs/token_usage_schema.md book/26_chapter_issue.md book/26_chapter_issue_zh.md
git commit -m "docs: Session Stats comfy-table layout"
```

---

## Spec coverage checklist

| Spec requirement | Task |
|------------------|------|
| comfy-table dep | Task 1 |
| Two-table layout (b) | Task 2 |
| Tool columns Count(s/f)/Total/Avg + Total row | Task 2 |
| `force_no_tty`, no ANSI | Task 2 |
| `summary() -> String` unchanged | Task 2 |
| Header/shape tests | Task 1–2 |
| token_usage_schema + Ch 26 | Task 3 |
