# Tool Execution Contracts Implementation Plan

Status: **implemented (Phase 1a, 2026-10-25), uncommitted until checkpoint**.
Ledger: `.superpowers/sdd/2026-10-25-tool-execution-contracts/progress.md`.

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the kernel-owned tool-execution contracts and a tested `ToolExecutor` pipeline, without wiring the production Agent yet.

**Architecture:** `tact_kernel` gains `Tool::call`, `ToolCallContext`, `ToolDescriptorSnapshot`, dynamic `ToolSource`, and a `ToolExecutor` that runs security → PreToolUse → permission/ask → one-use ticket → tool → PostToolUse. The ports are fake-able; production adapters and the legacy/MCP bridge land in Phase 1b.

**Tech Stack:** Rust 2024, `tact_kernel`, `tact_protocol`, `async-trait`, `tokio` (sync/macros/rt), `serde_json`.

**Spec:** `docs/superpowers/specs/2026-10-25-plugin-first-architecture-design.md`

## Global Constraints

- Work on `feat/plugin-first-architecture`; never commit directly to `main`.
- This plan ends with one checkpoint commit (`feat(kernel): phase 1a tool execution contracts`); do not push.
- `tact_kernel` dependency whitelist is unchanged: `tact_protocol`, `anyhow`, `async-trait`, `serde`, `serde_json`, `strum`, `tokio`.
- Do not modify the production Agent path in this plan. No behavior changes for users.
- Run one Cargo invocation at a time. Watch every test fail before implementing it.
- `scripts/check-rust.sh` must pass before the checkpoint commit.

## Review Focus

1. **Preflight order** — security guard → PreToolUse → recheck final input → permission/ask → ticket.
2. **Permission denial and hook block** — neither may invoke the tool or consume a ticket.
3. **Ticket one-use** — `execute` rejects reused, missing, or mismatched tickets.
4. **Ask path** — an Ask decision must go through `InteractionBroker`; rejection must not invoke the tool.
5. **Snapshot generation** — a stale generation must fail preflight before any side effect.

## File Structure

Create:

- `crates/tact_kernel/src/tool/context.rs` — `ToolOrigin`, `Workspace`, `CancelToken`, `RedactionPolicy`, `EventSink`, `ToolEvent`, `Interaction*`, `ToolCallContext`.
- `crates/tact_kernel/src/tool/source.rs` — `ToolSource`, `ToolDescriptorSnapshot`.
- `crates/tact_kernel/src/tool/executor.rs` — ports, `ToolRequest`, `ToolPreflight`, `ToolExecutor`.

Modify:

- `crates/tact_kernel/src/tool/mod.rs` — wire modules/re-exports.
- `crates/tact_kernel/src/capability/tool.rs` — add `Tool::call`.
- `crates/tact_kernel/src/catalog.rs` — source registration, generation, descriptor snapshot.
- `crates/tact_kernel/tests/conformance.rs` — fake tools implement `call`.
- `crates/tact_kernel/Cargo.toml` — dev-only `tokio` features already present.
- `docs/plugin-architecture.md` — record Phase 1a status.

### Task 1: Add neutral tool-call context and execution ports

**Files:**
- Create: `crates/tact_kernel/src/tool/context.rs`
- Create: `crates/tact_kernel/src/tool/executor.rs` (ports only; executor in Task 3)
- Modify: `crates/tact_kernel/src/tool/mod.rs`
- Modify: `crates/tact_kernel/src/capability/tool.rs`
- Test: `crates/tact_kernel/src/tool/context.rs` unit tests

**Interfaces:**
- Produces:

```rust
pub enum ToolOrigin { Main, Subagent }
pub struct Workspace { root: PathBuf }
pub struct CancelToken(Arc<AtomicBool>);
pub enum RedactionPolicy { Off, Basic, Credential }

pub enum ToolEvent {
    Started { call_id: String, tool_name: String },
    Finished { call_id: String, content: String },
    Failed { call_id: String, error: String },
}
pub trait EventSink: Send + Sync { fn emit(&self, event: ToolEvent); }

pub enum InteractionRequest {
    Confirm { prompt: String },
    Select { prompt: String, options: Vec<String> },
    Input { prompt: String },
}
pub enum InteractionResponse {
    Approved, Rejected, Cancelled, Text(String), Selected(Vec<String>),
}
#[async_trait]
pub trait InteractionBroker: Send + Sync {
    async fn request(&self, request: InteractionRequest) -> anyhow::Result<InteractionResponse>;
}

pub struct ToolCallContext {
    pub run_id: String,
    pub tool_use_id: String,
    pub session_id: Option<String>,
    pub active_goal_id: Option<String>,
    pub origin: ToolOrigin,
    pub workspace: Workspace,
    pub events: Arc<dyn EventSink>,
    pub interaction: Arc<dyn InteractionBroker>,
    pub cancel: CancelToken,
    pub redaction: RedactionPolicy,
}
```

- `Tool` gains:

```rust
#[async_trait]
pub trait Tool: Send + Sync {
    fn descriptor(&self) -> ToolDescriptor;
    async fn call(&self, ctx: ToolCallContext, input: serde_json::Value)
        -> anyhow::Result<ToolCallResult>;
}
```

- [ ] **Step 1: Write failing context tests**

Assert: `Workspace::new("/tmp")` exposes `root()`; `CancelToken::new().is_cancelled()` is false; `CancelToken::cancel()` makes it true; `ToolCallContext` can be built with fake `EventSink`/`InteractionBroker`; `RedactionPolicy` variants are distinct.

- [ ] **Step 2: Run them to verify RED**

Run: `cargo test -p tact_kernel tool::context::tests`

Expected: compile error because `context` does not exist.

- [ ] **Step 3: Implement the context/ports**

Keep `ToolCallContext` cloneable via `Arc` handles; `CancelToken` wraps `Arc<AtomicBool>` and is `Clone`. `ToolEvent`/`Interaction*` are plain data; no `tact_llm`.

- [ ] **Step 4: Add `Tool::call` and update conformance fakes**

Add a default-free `call` method to `Tool`. Update `EchoTool`/`NamelessTool` in `tests/conformance.rs` to return `ToolCallResult::text` and ignore the context.

- [ ] **Step 5: Verify**

Run: `cargo test -p tact_kernel tool::context::tests && cargo test -p tact_kernel --test conformance`

Expected: context tests pass; conformance still 9/9.

### Task 2: Add descriptor snapshots and dynamic sources

**Files:**
- Create: `crates/tact_kernel/src/tool/source.rs`
- Modify: `crates/tact_kernel/src/tool/mod.rs`
- Modify: `crates/tact_kernel/src/catalog.rs`
- Test: `crates/tact_kernel/src/catalog.rs` unit tests

**Interfaces:**
- Produces:

```rust
#[async_trait]
pub trait ToolSource: Send + Sync {
    fn id(&self) -> &str;
    async fn refresh(&self) -> anyhow::Result<()>;
    fn tools(&self) -> Vec<Arc<dyn Tool>>;
}

pub struct ToolDescriptorSnapshot {
    pub generation: u64,
    pub tools: Vec<ToolDescriptor>,
}
```

- `ToolCatalog` gains `add_source`, `refresh_sources`, `descriptor_snapshot(&CapabilityFilter)`, and a shared generation counter bumped on commit/refresh.
- Snapshot tools are sorted by name; disabled tool IDs are absent; duplicate source names keep the first source and skip the later one.

- [ ] **Step 1: Write failing snapshot tests**

Create a `FixedSource` returning one tool named `remote`; assert `descriptor_snapshot` contains `echo` and `remote`, is sorted, and generation increases after `refresh_sources`. Add a filter disabling `tool:remote`; assert `remote` is absent. Add a second source returning `echo`; assert only one `echo` descriptor survives.

- [ ] **Step 2: Run RED**

Run: `cargo test -p tact_kernel catalog::snapshot_tests`

Expected: compile error because the API does not exist.

- [ ] **Step 3: Implement snapshot/source support**

Use `Vec<Arc<dyn ToolSource>>` and `Arc<AtomicU64>` in `ToolCatalog`. `refresh_sources` awaits every source then bumps the generation. `descriptor_snapshot` gathers native tools + source tools, filters `tool:<name>`, sorts by name, and de-duplicates by first name.

- [ ] **Step 4: Verify**

Run: `cargo test -p tact_kernel catalog::snapshot_tests`

Expected: all snapshot tests pass.

### Task 3: Add `ToolExecutor` with preflight/execute and one-use tickets

**Files:**
- Create: `crates/tact_kernel/src/tool/executor.rs` (extend Task 1 ports)
- Modify: `crates/tact_kernel/src/tool/mod.rs`
- Test: `crates/tact_kernel/src/tool/executor.rs` unit tests

**Interfaces:**
- Produces:

```rust
pub enum PermissionDecision {
    Allow,
    Deny(String),
    Ask(String),
}
#[async_trait]
pub trait PermissionService: Send + Sync {
    async fn decide(&self, request: PermissionRequest) -> anyhow::Result<PermissionDecision>;
}
pub struct PermissionRequest {
    pub descriptor: ToolDescriptor,
    pub input: serde_json::Value,
}

pub enum HookControl { Continue, Block(String) }
pub struct PreToolUseEvent { pub tool_name: String, pub input: serde_json::Value }
pub struct PostToolUseEvent {
    pub tool_name: String,
    pub input: serde_json::Value,
    pub result: ToolCallResult,
}
#[async_trait]
pub trait HookService: Send + Sync {
    async fn pre_tool_use(&self, event: &mut PreToolUseEvent) -> anyhow::Result<HookControl>;
    async fn post_tool_use(&self, event: &mut PostToolUseEvent) -> anyhow::Result<HookControl>;
}

pub trait SecurityService: Send + Sync {
    fn guard(&self, descriptor: &ToolDescriptor, input: &serde_json::Value) -> anyhow::Result<()>;
}

pub struct ToolRequest {
    pub call_id: String,
    pub tool_name: String,
    pub input: serde_json::Value,
    pub snapshot_generation: u64,
}

pub struct ToolPreflight { /* private fields */ }

pub struct ToolExecutor { /* private fields */ }
impl ToolExecutor {
    pub fn new(
        catalog: Arc<ToolCatalog>,
        filter: CapabilityFilter,
        security: Arc<dyn SecurityService>,
        permissions: Arc<dyn PermissionService>,
        hooks: Arc<dyn HookService>,
    ) -> Self;
    pub async fn descriptor_snapshot(&self) -> anyhow::Result<ToolDescriptorSnapshot>;
    pub async fn preflight(
        &self,
        request: ToolRequest,
        ctx: ToolCallContext,
    ) -> anyhow::Result<ToolPreflight>;
    pub async fn execute(&self, prepared: ToolPreflight) -> anyhow::Result<ToolCallResult>;
}
```

- Preflight order: generation check → resolve descriptor → security guard → `pre_tool_use` → re-guard final input → permission decision → if Ask, `InteractionRequest::Confirm` (reject/cancel is a permission error) → mint one-use ticket.
- Execute: consume ticket first; on missing/reused/mismatched ticket return an error without calling the tool; emit `Started`; call; emit `Finished`/`Failed`; run `post_tool_use`; a PostToolUse block rewrites the result to a failed tool output (matching current hook semantics).

- [ ] **Step 1: Write failing executor tests**

Use a recording fake for each port and a `RecordingTool` that increments a call count. Assert:
- Order is security, pre-hook, security (final input), permission, tool, post-hook.
- `PermissionDecision::Deny` does not call the tool and emits no `Started`.
- `HookControl::Block` does not consult permission or call the tool.
- `InteractionResponse::Rejected` on Ask does not call the tool.
- A successful execute consumes the ticket; a second execute fails with `ticket` in the message.
- A stale `snapshot_generation` fails before security.
- A PreToolUse input rewrite is what the tool receives (and the second security guard sees it).

- [ ] **Step 2: Run RED**

Run: `cargo test -p tact_kernel tool::executor::tests`

Expected: compile error because `ToolExecutor` does not exist.

- [ ] **Step 3: Implement the executor and ticket store**

Keep `ToolPreflight` opaque: private `tool`, `descriptor`, `input`, `ctx`, `ticket`. Use a `Mutex<HashSet<u64>>` for live tickets; `execute` removes the ticket before invoking. Never call the tool before permission/ticket checks. Use `ctx.events` for `Started`/`Finished`/`Failed`.

- [ ] **Step 4: Verify**

Run: `cargo test -p tact_kernel tool::executor::tests`

Expected: all executor tests pass.

### Task 4: Wire CI/docs and checkpoint

**Files:**
- Modify: `docs/plugin-architecture.md`
- Modify: `docs/superpowers/plans/2026-10-25-tool-execution-contracts.md` (this file)
- Test: full gate

- [ ] **Step 1: Update status docs**

Record Phase 1a as implemented: kernel execution contracts and fake-tested `ToolExecutor`, production Agent not wired. Explicitly say Phase 1b owns `LegacyToolAdapter`, `McpToolSourceAdapter`, and the Agent cutover.

- [ ] **Step 2: Run the workspace gate**

Run:
- `cargo test -p tact_kernel`
- `cargo test -p tact --test plugin_contracts`
- `bash scripts/check-architecture.sh`
- `bash scripts/check-module-boundaries.sh`
- `cargo fmt -- --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `scripts/check-rust.sh` with the proxy exported

Expected: all green.

- [ ] **Step 3: Commit**

```bash
git add crates/tact_kernel docs/plugin-architecture.md docs/superpowers/plans/2026-10-25-tool-execution-contracts.md
git commit -m "feat(kernel): phase 1a tool execution contracts"
```

## Self-Review Notes

- Scope: kernel contracts and executor only; production Agent remains untouched.
- Review Focus mapping: order/deny/block/ask/ticket/stale generation all in Task 3 tests.
- Follow-on Phase 1b: permission/security/hook adapters over existing `PermissionManager`/hook engine, `LegacyToolAdapter`, `McpToolSourceAdapter`, Agent dispatch cutover, and behavior-parity suites.
