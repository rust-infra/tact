# Background Tasks
> Language: [English](./13_chapter_background.md) · [中文](./13_chapter_background_zh.md)

This chapter explains Tact's **asynchronous shell execution**: the `background_run` tool starts a command on a `tokio::spawn` task and returns immediately; `check_background` polls status later. Every task is persisted to disk, so results survive polling order — but not process restarts (see §5). The implementation lives in `crates/tact/src/background.rs` with tool wrappers in `crates/tact/src/tool/background_run.rs`.

Background tasks are the "fire-and-forget" counterpart to the synchronous `bash` tool: same shell, same validation, but the agent's turn does not block on completion.

---

## 1. Tool Surface

| Tool | Input | Output |
|------|-------|--------|
| `background_run` | `command: String` | `"Background task <id> started: <command>"` |
| `check_background` | `task_id: Option<String>` | One task as pretty JSON, or a one-line-per-task listing |

Both tools are in the main `toolset()` only. `check_background` with no `task_id` lists all known tasks sorted by start time; with an unknown id it returns an error (`Unknown background task <id>`).

TUI users do not need the model to call a tool to see background jobs: the **`/background`** slash command lists all tasks, and **`/background <id>`** shows a single task (pretty JSON). It sends `UserCommand::QueryBackground(Option<String>)` to the command driver, which calls the same `SharedBackgroundManager::check` and renders the result into the log as Markdown (`AgentUpdate::MdInfo`) — see [Ch 23](./23_chapter_tui.md) §3.

**Live output (bash-like).** While a task runs, its stdout/stderr stream into the `background_run` tool card in real time (throttled to ~50 ms batches, last ~4 KB kept for the live preview), exactly like the synchronous `bash` card. The card stays in a running state even though the invocation already returned, and closes with ✓/✗, elapsed time, and the final output when the process exits (see §3 and §6).

---

## 2. Data Model

```rust
pub enum BackgroundTaskStatus { Running, Completed, Error }   // snake_case in JSON

pub struct BackgroundTaskRecord {
    pub id: String,                        // 8-hex-digit counter, seeded from epoch millis
    pub status: BackgroundTaskStatus,
    pub command: String,
    pub session_id: String,                // owning agent session; "" outside a session
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub output: String,                    // combined stdout + stderr, capped
}
```

`BackgroundManager` holds the store and the id source:

```rust
records: Arc<dyn BackgroundStore>,   // .tact/tact.db → background_tasks table
next_id: AtomicU64,                  // monotonically increasing id source
```

There is **no in-memory mirror**: the SQLite pool serializes writes and the database is the single source of truth, so `SharedBackgroundManager` wraps `Arc<BackgroundManager>` with no interior mutex — the spawned tokio task writes results back through a cloned store handle.

---

## 3. Execution Lifecycle

```mermaid
sequenceDiagram
    participant Agent
    participant BM as BackgroundManager
    participant Task as tokio::spawn
    participant TUI as TUI card
    participant DB as .tact/tact.db (background_tasks)

    Agent->>BM: background_run("cargo build")
    BM->>BM: validate_shell_command (blocks sudo, rm -rf /, …)
    BM->>DB: upsert record (status: running)
    BM->>Task: spawn sh -c "cargo build" (cwd = work_dir)
    BM-->>Agent: "Background task 018f3a2c started"

    Note over Task: runs concurrently with the agent loop

    loop stdout/stderr streaming
        Task->>TUI: ToolProgress (throttled ~50ms)
    end
    Task->>Task: await exit (timeout 120s, kill_on_drop)
    Task->>TUI: BackgroundTaskFinished (✓/✗ + final output)
    Task->>DB: upsert record (completed/error + output)

    Agent->>BM: check_background("018f3a2c")
    BM->>DB: read record by id
    BM-->>Agent: record as pretty JSON
```

Details worth knowing:

| Aspect | Behavior |
|--------|----------|
| Shell | `sh -c <command>`, cwd = `ToolContext.work_dir` |
| Validation | `crate::shell::validate_shell_command` — same hard blocklist as `bash` (`sudo`, `rm -rf /`, …) |
| Timeout | Fixed 120 seconds; on expiry status becomes `Error` with `"Error: Timeout (120s)"` |
| Live streaming | stdout/stderr are read incrementally and pushed as `AgentUpdate::ToolProgress` (≈50 ms batches, last ~4 KB kept in the live preview); no output is buffered until completion |
| Output cap | First 50,000 chars of stdout+stderr are persisted in the record; the rest is dropped |
| Exit code | Non-zero exit → `Error`; the code itself is not recorded |
| Process cleanup | `kill_on_drop(true)` — the child is killed if the future is dropped |

The spawned task's final `save_record` result is discarded (`let _ =`); a disk failure at that point loses the outcome silently.

---

## 4. ID Generation

```rust
next_id: AtomicU64::new(Utc::now().timestamp_millis() as u64)
```

IDs are the lower 32 bits of an atomic counter formatted as 8 hex digits (`{:08x}`), seeded from the current epoch milliseconds at manager construction. Sequential within a process, unique enough across restarts in practice — but not guaranteed collision-free if two managers start within the same millisecond or the counter wraps.

---

## 5. Crash Recovery on Startup

`BackgroundManager::new` (called at session startup in `headless.rs` / `interactive.rs`) scans the `background_tasks` table and repairs orphans: any record still marked `running` belongs to a process that no longer exists, so it is rewritten as:

```text
status: error
output: "Process interrupted (agent restarted)"
```

This is covered by the `marks_stale_running_tasks_on_startup` unit test. The consequence: background tasks **do not survive restarts** — the manager assumes a fresh process means all previous children are dead (true, since they were spawned in-process with `kill_on_drop`).

---

## 6. Interaction with the Agent Loop

`background_run` returns immediately, so from the scheduler's perspective ([Tasks and Tool Scheduling](./11_chapter_task.md)) it is a cheap call — but note that as a shell-adjacent tool it takes its permission classification from the [Permission Model](./10_chapter_permission.md) under the name `background_run`, not `bash`.

**The TUI gets live progress + a completion event.** The spawned task pushes `AgentUpdate::ToolProgress` into the invocation's tool card while it runs, then `AgentUpdate::BackgroundTaskFinished` when it exits (see [Ch 25](./25_chapter_protocol.md) for the keep-live card contract). The **model/agent** still has no completion push: it must poll `check_background` to see the result in context. A typical pattern the model discovers on its own is `background_run` → continue other work → `check_background` before finishing the turn. The [sleep tool](./07_chapter_tool.md) exists partly to make that polling loop possible.

Unlike synchronous `bash` output, background output is **not** routed through `persist_large_output` ([Context Compaction](./05_chapter_compact.md)) — instead it is hard-capped at 50k chars in the record itself, and the full JSON (including output) lands in context when polled.

---

## 7. Code Map

| File | Role |
|------|------|
| `crates/tact/src/store/background_store/mod.rs` | `BackgroundStore` trait (async: upsert/get/list) |
| `crates/tact/src/store/background_store/sqlite.rs` | `SqliteBackgroundStore` — `background_tasks` table |
| `crates/tact/src/background.rs` | `BackgroundManager`, `SharedBackgroundManager`, record types, spawn logic, startup repair |
| `crates/tact/src/tool/background_run.rs` | `background_run` / `check_background` tools |
| `crates/tact/src/shell.rs` | `validate_shell_command` blocklist shared with `bash` |
| `crates/tact/src/tool/mod.rs` | `ToolContext.background_manager` |
| `crates/tact/src/tool/registry.rs` | Background tools in `toolset()` |
| `crates/tact-ui/src/headless.rs`, `interactive.rs` | Manager constructed from `tact.db` at startup |
| `docs/state_machines.md` | Background job state diagram |

---

## 8. Current Gaps

| Gap | Detail |
|-----|--------|
| Fixed 120s timeout | Not configurable; long builds or test suites always die as `Error: Timeout` |
| No model completion push | The TUI card gets `BackgroundTaskFinished`, but the **model** still has no completion push and must poll `check_background` |
| No cancellation tool | A running task cannot be killed by the model; only timeout or process exit ends it |
| Output interleaving lost | stdout and stderr are concatenated after completion, not merged by time |
| Exit code discarded | Failure reason beyond the combined output text is unavailable |
| Records accumulate | `background_tasks` table is never pruned |
| 50k output cap silently truncates | No `<persisted-output>` spill like synchronous `bash` gets |
| ID collisions possible | 32-bit hex counter seeded by wall clock; no uniqueness check against disk |

---

## Related Docs

- [Tool System](./07_chapter_tool.md) — `ToolContext`, `toolset()`, and the synchronous `bash` counterpart
- [Permission Model](./10_chapter_permission.md) — how `background_run` is gated
- [Context Compaction](./05_chapter_compact.md) — the output-spill mechanism background tasks bypass
- [Store and Persistence](./01_chapter_store.md) — the `background_tasks` SQLite table
- [docs/state_machines.md](../docs/state_machines.md) — background job states
- [ARCHITECTURE.md](../ARCHITECTURE.md) — §7 background tasks row
