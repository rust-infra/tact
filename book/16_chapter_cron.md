# Cron Scheduling
> Language: [English](./16_chapter_cron.md) · [中文](./16_chapter_cron_zh.md)

This chapter explains how Tact lets the agent **register scheduled prompts**: cron expressions, prompt text, and metadata persisted in `<workdir>/.tact/tact.db` (the `cron_tasks` table). The model can create, list, and delete these records through native tools; the storage layer is wired into every main-agent session via `ToolContext`.

**Important scope note:** as of this writing, Tact persists scheduled tasks but does **not** yet run a background tick loop that evaluates cron expressions and injects prompts into `agent_loop`. The `recurring` and `durable` flags are stored and shown in listings; they are reserved for future runtime behaviour. See [§8 Current Gaps](#8-current-gaps).

---

## 1. What Cron Scheduling Is For

Cron in Tact is **not** a shell job runner (that is [background tasks](../crates/tact/src/background.rs) via `background_run` / `check_background`). It is a registry of **prompts the agent should receive on a schedule**:

| Concept | Meaning in code |
|---------|-----------------|
| `cron` | Cron expression string (stored as-is; not validated or parsed today) |
| `prompt` | User message text to inject when the schedule fires |
| `recurring` | `true` → recurring schedule; `false` → one-shot (metadata only today) |
| `durable` | `true` → survive session restarts; `false` → session-scoped (metadata only today) |

The agent uses `cron_create` during a turn when the user asks for reminders, daily check-ins, or other time-based follow-ups. Until a runtime scheduler exists, those entries are durable **records** the agent (or a future daemon) can query with `cron_list`.

---

## 2. Architecture Overview

```mermaid
graph TB
    subgraph Entry["Session startup (tui.rs)"]
        DB[(tact.db)]
        CS[CronScheduler]
        SCS[SharedCronScheduler]
        DB --> CS
        CS --> SCS
    end

    subgraph Agent["Main agent loop"]
        TC[ToolContext.cron_scheduler]
        TR[ToolRouter]
        SCS --> TC
        TC --> TR
    end

    subgraph Tools["Native tools"]
        CC[cron_create]
        CL[cron_list]
        CD[cron_delete]
        TR --> CC
        TR --> CL
        TR --> CD
    end

    subgraph Store["On disk"]
        TBL[cron_tasks table]
        CC --> TBL
        CL --> TBL
        CD --> TBL
    end

    subgraph Missing["Not implemented yet"]
        TICK[Scheduler tick / cron parser]
        LOOP[Inject prompt into agent_loop]
    end

    TBL -.-> TICK
    TICK -.-> LOOP
```

Sub-agents (`subagent_toolset`) do **not** receive cron tools — only the main agent's full `toolset()` includes them.

---

## 3. Data Model

Defined in `crates/tact/src/cron/mod.rs`:

```rust
pub struct ScheduledTaskRecord {
    pub id: String,
    pub cron: String,
    pub prompt: String,
    pub recurring: bool,
    pub durable: bool,
    pub session_id: String,  // owning agent session; "" outside a session
    pub created_at: i64,     // Unix timestamp (UTC, millis)
}
```

Each task is one row in the `cron_tasks` table (`crates/tact/src/store/cron_store/sqlite.rs`). IDs come from `INTEGER PRIMARY KEY AUTOINCREMENT` and are surfaced as 8-hex-digit strings (`format!("{rowid:08x}")`) — the same wire contract as the legacy JSON index.

---

## 4. Persistence

| Item | Value |
|------|-------|
| Database | `<workdir>/.tact/tact.db` (shared with sessions, tasks, background) |
| Table | `cron_tasks(id INTEGER PRIMARY KEY AUTOINCREMENT, cron TEXT, prompt TEXT, recurring INTEGER, durable INTEGER, session_id TEXT, created_at INTEGER)` |
| Backend | `SqliteCronStore` (sqlx pool, `busy_timeout` 5 s) |
| Index | `idx_cron_tasks_session_id` on `session_id` |
| Init | `CREATE TABLE IF NOT EXISTS` on first open |

`CronScheduler::new` is called with `tact_path.session_db_path()` in `headless.rs` / `interactive.rs`. The legacy `cron/scheduled_tasks.json` file is **no longer read**; old entries are left on disk and ids restart at `00000001`.

---

## 5. Scheduler Lifecycle

### Construction

Both `crates/tact-ui/src/headless.rs` and `interactive.rs` build the scheduler once per process:

```text
db_path = tact_path.session_db_path()              // <workdir>/.tact/tact.db
cron_scheduler = SharedCronScheduler::new(CronScheduler::new(&db_path).await?)
tool_context = ToolContext { cron_scheduler, work_dir, … }
agent = Agent::new(client, tool_context, toolset(), …)
```

There is no separate cron daemon or tokio task spawned today. The scheduler exists for the lifetime of the agent process and is shared across all tool calls through `ToolContext` (cloneable via `Arc<CronScheduler>` — the SQLite pool serializes writes, so no mutex is needed).

### `CronScheduler` vs `SharedCronScheduler`

| Type | Role |
|------|------|
| `CronScheduler` | Async CRUD facade over `Box<dyn CronStore>` |
| `SharedCronScheduler` | `Arc<CronScheduler>`; tools and tests call the same async methods |

---

## 6. Agent Tools

Implemented in `crates/tact/src/tool/cron.rs` and registered in `toolset()` (`crates/tact/src/tool/registry.rs`).

### `cron_create`

**Input:**

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `cron` | string | required | Cron expression |
| `prompt` | string | required | Prompt to inject when the schedule fires |
| `recurring` | bool | `false` | Recurring vs one-shot |
| `durable` | bool | `false` | Durable vs session-scoped |

**Output:** Pretty-printed JSON of the new `ScheduledTaskRecord`.

### `cron_list`

**Input:** empty object.

**Output:** One line per task (sorted by id), or `"No scheduled tasks."`:

```text
00000000 0 9 * * * [recurring/session]: Daily standup summary
```

Tags in brackets: `recurring` or `one-shot`, plus `/durable` or `/session`.

### `cron_delete`

**Input:** `{ "id": "<task id>" }`.

**Output:** `"Deleted scheduled task {id}"`, or error if id not found.

These tools are **independent** barriers in the tool scheduler (no row conflicts on distinct ids). They do not touch `work_dir` directly — only the `cron_tasks` table in `tact.db`.

---

## 7. Cron vs Background Tasks

Both are workspace-scoped managers injected through `ToolContext`, but they solve different problems:

```mermaid
flowchart LR
    subgraph Cron["Cron (this chapter)"]
        C1[Store prompt + cron expr]
        C2[CRUD via agent tools]
        C3[No runtime executor yet]
    end

    subgraph BG["Background tasks"]
        B1[Run shell command async]
        B2[tokio::spawn + poll]
        B3[check_background for output]
    end
```

| | Cron | Background |
|---|------|------------|
| Module | `cron/mod.rs` | `background.rs` |
| Persists | Scheduled prompts | Shell commands + stdout/stderr |
| Executes today | No | Yes (`background_run`) |
| Sub-agent access | No | No |

---

## 8. Current Gaps

The following are **not** in the codebase yet; documenting them avoids confusion with README marketing copy:

1. **No cron evaluator** — expressions are opaque strings; nothing parses or validates them.
2. **No tick loop** — no task wakes up on a timer to call `agent_loop` with stored prompts.
3. **`recurring` / `durable` unused at runtime** — only persisted and displayed by `cron_list`.
4. **No integration with session store** — firing a prompt would need new wiring (TUI event, headless trigger, or sidecar process).
5. **No automatic cleanup** — one-shot tasks are not removed after a hypothetical fire.

When a runtime is added, likely touch points are: a tokio interval in `tui.rs` or a dedicated module reading `cron_tasks`, plus a path to enqueue user messages into the active agent (similar to user input in interactive mode).

---

## 9. Code Map

| File | Role |
|------|------|
| `crates/tact/src/cron/mod.rs` | `ScheduledTaskRecord`, `CronScheduler`, `SharedCronScheduler` |
| `crates/tact/src/store/cron_store/mod.rs` | `CronStore` trait (async: create/delete/list) |
| `crates/tact/src/store/cron_store/sqlite.rs` | `SqliteCronStore` — `cron_tasks` table, 8-hex public ids |
| `crates/tact/src/tool/cron.rs` | `cron_create`, `cron_list`, `cron_delete` tool handlers |
| `crates/tact/src/tool/mod.rs` | `ToolContext.cron_scheduler` |
| `crates/tact/src/tool/registry.rs` | Cron tools in `toolset()` |
| `crates/tact-ui/src/headless.rs`, `interactive.rs` | Construct scheduler and pass it into `Agent` |
| `crates/tact/src/store/mod.rs` | `StoreRoot` / JSON store (teammates, worktrees) |
| `crates/tact/src/tool/test_support.rs` | Test `ToolContext` with temp `tact.db` |

---

## Related Docs

- [ARCHITECTURE.md](../ARCHITECTURE.md) — Sub-agents, team, tasks, worktrees table (Cron row)
- [Tasks and Tool Scheduling](./11_chapter_task.md) — how tool calls run once the model acts (orthogonal to cron firing)
- [crates/tact/tact.md](../crates/tact/tact.md) — domain managers and `.tact/` layout
- [docs/state_machines.md](../docs/state_machines.md) — background task lifecycle (contrast with cron)
