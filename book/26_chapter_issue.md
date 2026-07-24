# Engineering Issue Log

> Language: [English](./26_chapter_issue.md) · [中文](./26_chapter_issue_zh.md)

This chapter is a **chronological log of optimizations and bug fixes** that changed user-visible or API-visible behavior. It is not a tutorial: each entry records the problem, the decision, and where the code / design docs live so future work does not rediscover the same trade-offs.

Related process docs: `AGENTS.md` (when to append here), `docs/superpowers/specs/` (design), `docs/superpowers/plans/` (implementation plans).

---

## 0. Purpose

| Goal | Detail |
|------|--------|
| Continuity | Capture *why* a change landed, not only *what* files moved |
| Cross-link | Point at design specs, PRs, and book chapters that teach the subsystem |
| Avoid churn | Prefer one entry per shipped behavior change; do not log pure refactors or test-only edits |

### Entry template

Newest entries first. Each entry should include:

1. **Date / ID** — `YYYY-MM-DD` and optional PR number  
2. **Type** — `optimization` · `bugfix` · `removal` · `docs`  
3. **Symptom / motivation** — what was wrong or expensive before  
4. **Decision** — the chosen contract (not discarded alternatives in full)  
5. **Behavior after** — observable rules agents and users rely on  
6. **Pointers** — code paths, specs, related book chapters  

---

## 1. 2026-07-24 — Session Stats rendered with comfy-table

| Field | Value |
|-------|-------|
| **Type** | optimization |
| **Spec** | `docs/superpowers/specs/2026-07-24-session-stats-table-design.md` |
| **Plan** | `docs/superpowers/plans/2026-07-24-session-stats-table.md` |

**Symptom / motivation:** End-of-session Tool calls rows used ad-hoc space
padding, so columns drifted as names and timings grew.

**Decision:** Keep `SessionStats::summary() -> String`. Render a head
Metric/Value table, an optional Tool calls table
(`Tool | Count(s/f) | Total | Avg`), then a trailing Metric/Value table for
tool aggregates / cache / reasoning. Use `comfy-table` with UTF8 boxes, no
ANSI colors, `force_no_tty()`.

**Behavior after:** Same counters and visibility rules; layout is aligned
tables instead of free-form lines.

**Pointers:** `crates/tact/src/stats.rs`, `docs/token_usage_schema.md`
(Session Stats Display).

---

## 2. 2026-07-24 — `/model` supplements config from `/v1/models`

| Field | Value |
|-------|-------|
| **Type** | optimization |
| **Spec** | `docs/superpowers/specs/2026-07-24-openai-models-api-design.md` |
| **Plan** | `docs/superpowers/plans/2026-07-24-openai-models-api.md` |

**Symptom / motivation:** `/model` required a hand-maintained `models = [...]`
list; providers already expose `GET /v1/models`.

**Decision:** Config remains primary; API appends missing ids; conflicts keep
config; fetch once per `(base_url, api_key)` on first `/model`; Anthropic skipped;
failures soft-fail to config-only / empty hint.

**Behavior after:** See Ch 21 `/model` section.

**Pointers:** `crates/tact_llm/src/models.rs`, `crates/tui/src/handlers/select.rs`,
Ch 21, Ch 22 (account-style queries).

---

## 3. 2026-07-24 — `read_file` pagination and `batch_read` removal

| Field | Value |
|-------|-------|
| **Type** | optimization + removal |
| **PR** | [#50](https://github.com/rust-infra/tact/pull/50) |
| **Spec** | `docs/superpowers/specs/2026-07-24-read-file-pagination-design.md` |
| **Plan** | `docs/superpowers/plans/2026-07-24-read-file-pagination.md` |

### 3.1 Symptom

`read_file` loaded the whole file with `read_to_string`, then silently discarded the tail with `chars().take(50000)`. That conflicted with line-based `offset` / `limit`, gave the model no recovery signal (hallucination risk — see [Ch 20](./20_chapter_hallucination.md)), and competed with dispatch-level `persist_large_output` (30k characters → `<persisted-output>`).

`batch_read` was a second multi-file API with its own 200k-character hard cap, duplicating schedule / recent-file special cases.

### 3.2 Decision

1. Delete `batch_read`. Parallel multi-file reads use concurrent `read_file` waves.  
2. Stream lines with Tokio `BufReader` (no whole-file buffer for the page).  
3. Bound pages with prefixed constants in `read_file.rs`:

```rust
const READ_FILE_MAX_OUTPUT_TOKENS: usize = 25_000;
const READ_FILE_DEFAULT_MAX_LINES: usize = 2_000;
```

Token estimate: existing `approx_token_count` (`ceil(UTF-8 bytes / 4)`).  
4. No per-line character limit (a single oversized line errors; never silent mid-line cut).  
5. Incomplete **implicit** / default pages return a leading marker:

```text
[PARTIAL view — lines {start}-{end}; continue with offset={next}]

{joined lines}
```

6. **Explicit** `offset` and/or `limit` that still exceed the token budget → **error** (do not silently return less than requested).  
7. `run_native_tool` **skips** `persist_large_output` when `name == "read_file"`.  
8. Tool `description` stays short — limits are enforced at runtime, not duplicated in the schema blurb.

### 3.3 Behavior after

| Case | Result |
|------|--------|
| Small file, no args | Full content, no PARTIAL |
| File longer than 2000 lines, no args | First 2000 lines + PARTIAL with `offset=2001` |
| Token budget hit on implicit read | Complete lines that fit + PARTIAL with next `offset` |
| Explicit range over token budget | `Err` asking to reduce `limit` / shrink the section |
| Single line alone over budget | `Err` (cannot recover via line offset) |
| Offset past EOF | Empty string |
| Large `read_file` vs bash / MCP | `read_file` never gets `<persisted-output>`; others still may |

### 3.4 Pointers

| Area | Path |
|------|------|
| Implementation | `crates/tact/src/tool/read_file.rs` |
| Persist exemption | `crates/tact/src/agent/tool_dispatch.rs` (`run_native_tool`) |
| Tool registration | `crates/tact/src/tool/registry.rs` (no `BatchReadTool`) |
| Approx tokens | `crates/tact/src/utils/truncate.rs` |
| Tool chapter | [Ch 7](./07_chapter_tool.md) |
| Compaction / spill | [Ch 5](./05_chapter_compact.md), `docs/compaction.md` |

---

## Related Docs

- [Tool System](./07_chapter_tool.md)
- [Context Compaction](./05_chapter_compact.md)
- [Hallucination in Agent Loops](./20_chapter_hallucination.md)
- [AGENTS.md](../AGENTS.md) — documentation sync triggers including this chapter
