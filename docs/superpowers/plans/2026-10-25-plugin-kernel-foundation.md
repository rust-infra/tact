# Plugin Kernel Foundation Implementation Plan

Status: **implemented (Phase 0, 2026-10-25), uncommitted**. Task-by-task
evidence is in `.superpowers/sdd/2026-10-25-plugin-kernel-foundation/progress.md`;
the full gate passed via `scripts/check-rust.sh`.

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a buildable `tact_kernel` crate that owns plugin lifecycle, capability IDs/filtering, registration catalogs, and the shared metadata types, without changing production behavior.

**Architecture:** `tact_kernel` is a new leaf crate above `tact_protocol`. Existing `tact` code keeps its public paths through re-exports; `tact` gains a one-way dependency on the kernel. This plan freezes registration and metadata contracts only. Tool execution, hooks, interaction and views are introduced by their owning phase plans.

**Tech Stack:** Rust 2024, Cargo workspace, `anyhow`, `async-trait`, `serde`, `serde_json`, `tokio` (sync only), `tact_protocol`.

**Spec:** `docs/superpowers/specs/2026-10-25-plugin-first-architecture-design.md`

## Scope and follow-on plans

This plan implements Phase 0 of the spec: the plugin boundary, capability identity/filter, registration catalogs, and shared metadata/result contracts. It deliberately does **not** implement `ToolExecutor`, `HookService`, `InteractionBroker`, `AgentService`, commands' execution methods or view execution. Those belong to the phase that first needs them:

- Phase 1: tool execution pipeline (`2026-10-25-tool-execution-pipeline.md`)
- Phase 2: domain plugins and prompt sections (`2026-10-25-domain-plugins.md`)
- Phase 3: runtime plugins and host (`2026-10-25-runtime-plugins-and-host.md`)
- Phase 4: views, commands and interaction (`2026-10-25-views-commands-interaction.md`)
- Phase 5: cleanup and enforcement (`2026-10-25-plugin-cleanup.md`)

This plan is complete when `tact_kernel` builds and tests alone, `tact` still passes its existing tool/metadata tests through re-exports, and the dependency guard is green. No user-facing behavior changes.

## Global Constraints

- Work on `feat/plugin-first-architecture`; never commit directly to `main`.
- Do **not** run `git commit` or `git push` in this plan unless the human explicitly asks. The plan intentionally carries no commit steps; leave a clean, reviewable diff after each task.
- `tact_kernel` may depend only on `tact_protocol`, `anyhow`, `async-trait`, `serde`, `serde_json`, `strum` and `tokio` (`sync` feature). It must not depend on `tact`, `tui`, `tact-ui`, `tact_llm`, `sqlx`, `rmcp` or `ratatui`.
- Run one Cargo invocation at a time; Cargo's `target/` lock is shared.
- Filter tests while iterating; widen only after the focused test passes.
- Keep production behavior unchanged in this plan. The only acceptable test changes are import paths and neutral image/effect types.
- Existing configuration keys, plugin marketplace behavior, TUI rendering and headless output must not change.
- Before pushing (not part of this plan unless asked): export the proxy and run `scripts/check-rust.sh`; also run `no_proxy=127.0.0.1,localhost,::1` when applicable.

## Review Focus

These are the failure modes this plan must pin, in order:

1. **Host registration failure** (Task 6) — duplicate plugin/capability IDs or a plugin failing mid-register must not leave partial state or a usable host.
2. **Disabled capability** (Tasks 3 and 6) — a filtered tool/command/prompt/view is absent from both lookup and descriptors, and the skip is visible as a diagnostic.
3. **Compatibility re-exports** (Task 7) — existing `tact::tool::*` imports must keep compiling after types move to `tact_kernel`.
4. **Neutral image/effect types** (Task 5) — `tact_kernel` must not acquire a `tact_llm` dependency, while a `read_image` result still reaches the provider as a base64 image.
5. **Permission policy method resolution** (Task 4) — moving the enum must not silently change `resolve`/`sensitive` behavior at existing call sites.

Each is pinned by a test in the task that owns the code.

## File Structure

Create:

- `crates/tact_kernel/Cargo.toml` — kernel package and dependency whitelist.
- `crates/tact_kernel/src/lib.rs` — module tree and crate documentation.
- `crates/tact_kernel/src/plugin.rs` — `PluginId`, `PluginKind`, `PluginManifest`, `Plugin`, `PluginHost`, `Registrar`, `Runtime`.
- `crates/tact_kernel/src/capability.rs` — `CapabilityKind`, `CapabilityId`, `CapabilityFilter`, `SkippedCapability`.
- `crates/tact_kernel/src/catalog.rs` — tool/command/prompt/view catalogs.
- `crates/tact_kernel/src/tool/metadata.rs` — policy and presentation metadata.
- `crates/tact_kernel/src/tool/resources.rs` — `ToolResources`, `ResourcePolicy` resolution.
- `crates/tact_kernel/src/tool/result.rs` — `ToolCallResult`, `ToolEffect`, `YieldReason`, `ToolImage`, `IntoToolCallResult`.
- `crates/tact_kernel/src/tool/descriptor.rs` — owned `ToolDescriptor`.
- `crates/tact_kernel/src/permission.rs` — `CapabilityRisk`.
- `crates/tact_kernel/tests/conformance.rs` — fake plugin conformance test.
- `crates/tact/tests/plugin_contracts.rs` — compatibility imports from `tact`.
- `scripts/check-architecture.sh` — crate-edge guard.
- `scripts/check-module-boundaries.sh` — intra-`tact` import guard (with a current-debt allowlist).

Modify:

- `Cargo.toml` — add the workspace member.
- `crates/tact/Cargo.toml` — depend on `tact_kernel`.
- `crates/tact/src/tool/metadata.rs` — re-export moved types; keep classification methods behind a local extension trait.
- `crates/tact/src/tool/mod.rs` — re-export the moved result/metadata types.
- `crates/tact/src/permission/mod.rs` — re-export `CapabilityRisk`; host the permission-policy extension trait.
- `crates/tact/src/agent/tool_schedule.rs` — use the kernel `ToolResources`.
- `crates/tact/src/tool/read_image.rs` — construct neutral `ToolImage`.
- `crates/tact/src/agent/tool_dispatch.rs` — convert `ToolImage` at the message boundary; update effect matches.
- `crates/tact/src/tool/goal.rs` — rename the yield effect variant.
- `scripts/check-rust.sh` — include `tact_kernel` tests and run the guards.
- `ARCHITECTURE.md` — add the kernel crate to the workspace table and dependency graph.

## Tasks

### Task 1: Add the kernel crate and dependency guard

**Files:**
- Create: `crates/tact_kernel/Cargo.toml`
- Create: `crates/tact_kernel/src/lib.rs`
- Create: `scripts/check-architecture.sh`
- Modify: `Cargo.toml`

**Interfaces:**
- Produces: workspace package `tact_kernel`; `tact_kernel` library target.
- Produces: `scripts/check-architecture.sh` exits non-zero when `tact_kernel` is missing or has a forbidden dependency.

- [ ] **Step 1: Write the failing dependency guard**

Create `scripts/check-architecture.sh` with a Python body that runs:

```bash
cargo metadata --no-deps --format-version 1
```

It must fail with `missing workspace package: tact_kernel` when the package is absent, and with `forbidden dependency: <crate>` when a `tact_kernel` dependency is not in the whitelist.

- [ ] **Step 2: Run the guard to verify it fails**

Run: `bash scripts/check-architecture.sh`

Expected: non-zero, output contains `missing workspace package: tact_kernel`.

- [ ] **Step 3: Create the crate and register it**

Create `crates/tact_kernel/Cargo.toml` with `name = "tact_kernel"`, workspace version/edition/license, and only the allowed dependencies. Add `"crates/tact_kernel"` to `Cargo.toml`'s `members`. Create `src/lib.rs` with crate docs and no modules yet.

- [ ] **Step 4: Run the guard and compile**

Run: `bash scripts/check-architecture.sh && cargo check -p tact_kernel`

Expected: guard prints the whitelist and exits 0; `cargo check` succeeds.

### Task 2: Add plugin identity and manifest types

**Files:**
- Create: `crates/tact_kernel/src/plugin.rs`
- Modify: `crates/tact_kernel/src/lib.rs`
- Test: `crates/tact_kernel/src/plugin.rs` unit tests

**Interfaces:**
- Produces:

```rust
pub struct PluginId(String);
impl PluginId {
    pub fn new(value: impl Into<String>) -> anyhow::Result<Self>;
    pub fn as_str(&self) -> &str;
}

pub enum PluginKind {
    Policy,
    Infrastructure,
    Runtime,
    Capability,
    View,
}

pub struct PluginManifest {
    pub id: PluginId,
    pub name: &'static str,
    pub version: &'static str,
    pub kind: PluginKind,
    pub description: &'static str,
}
```

- [ ] **Step 1: Write the failing identity tests**

Tests must assert:
- `PluginId::new("")`, `"a b"`, `"a\nb"` return `Err`.
- `PluginId::new("tool.bash").unwrap().as_str() == "tool.bash"`.
- `PluginManifest` exposes the exact ID and `PluginKind::Capability`.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p tact_kernel plugin::tests::plugin_id_rejects_blank_or_control -- --nocapture`

Expected: compile error because `PluginId` does not exist.

- [ ] **Step 3: Implement the types**

Reject empty strings, any whitespace, and ASCII control characters. Keep the error message specific (`plugin id cannot be empty`, `plugin id cannot contain whitespace`). Add `#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]` to `PluginId`; add `Debug, Clone, Copy, PartialEq, Eq` to `PluginKind`.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p tact_kernel plugin::tests`

Expected: all identity tests pass.

### Task 3: Add capability identity and filtering

**Files:**
- Create: `crates/tact_kernel/src/capability.rs`
- Modify: `crates/tact_kernel/src/lib.rs`
- Test: `crates/tact_kernel/src/capability.rs` unit tests

**Interfaces:**
- Produces:

```rust
pub enum CapabilityKind {
    Tool,
    Command,
    PromptSection,
    View,
    Agent,
    Chat,
    Session,
    Permission,
    Security,
    Hook,
}

pub struct CapabilityId {
    pub kind: CapabilityKind,
    pub name: String,
}

pub struct CapabilityFilter { /* disabled: BTreeSet<CapabilityId> */ }
impl CapabilityFilter {
    pub fn new(disabled: impl IntoIterator<Item = CapabilityId>) -> Self;
    pub fn allows(&self, id: &CapabilityId) -> bool;
    pub fn disabled(&self) -> impl Iterator<Item = &CapabilityId>;
}

pub struct SkippedCapability {
    pub plugin: PluginId,
    pub capability: CapabilityId,
}
```

- `CapabilityId: FromStr` accepts `<kind>:<name>` where kind is one of `tool`, `command`, `prompt`, `view`, `agent`, `chat`, `session`, `permission`, `security`, `hook`.
- `Display` round-trips the same string.

- [ ] **Step 1: Write failing tests**

Tests must assert:
- `"tool:bash".parse::<CapabilityId>()` has `CapabilityKind::Tool`, name `bash`, and displays `"tool:bash"`.
- `"unknown:x"` and `"tool:"` fail.
- A filter built from `["tool:bash"]` allows `tool:read_file`, rejects `tool:bash`, and iterates exactly the disabled ID.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p tact_kernel capability::tests`

Expected: compile error because the types do not exist.

- [ ] **Step 3: Implement the IDs and filter**

Use `BTreeSet<CapabilityId>`; derive `Ord` on `CapabilityId` (kind order is the enum order above). Return a specific parse error for an unknown kind, an empty name, or a missing colon.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p tact_kernel capability::tests`

Expected: all filter and round-trip tests pass.

### Task 4: Move metadata, policy and scheduling-resource contracts

**Files:**
- Create: `crates/tact_kernel/src/tool/mod.rs`
- Create: `crates/tact_kernel/src/tool/metadata.rs`
- Create: `crates/tact_kernel/src/tool/resources.rs`
- Create: `crates/tact_kernel/src/permission.rs`
- Create: `crates/tact/src/permission/policy_ext.rs`
- Modify: `crates/tact_kernel/src/lib.rs`
- Modify: `crates/tact/Cargo.toml`
- Modify: `crates/tact/src/tool/metadata.rs`
- Modify: `crates/tact/src/tool/mod.rs`
- Modify: `crates/tact/src/permission/mod.rs`
- Modify: `crates/tact/src/agent/tool_schedule.rs`
- Test: kernel metadata tests + existing `tact` permission tests

**Interfaces:**
- Moves to `tact_kernel`:
  - `ToolMetadata`, `PermissionPolicy`, `PermissionPromptPolicy`, `ResourcePolicy`, `ArgumentSummaryPolicy`, `ToolDomain`, `TaskOperation`, `LiveOutputPolicy`, `DetailPolicy`, `PopupPolicy`, `ToolPresentation`, `OutputPolicy`.
  - `CapabilityRisk`.
  - `ToolResources`; `ResourcePolicy::resolve` and `recent_paths` move with it.
  - `PermissionPolicy::target` and the patch-path parsers
    (`extract_patch_paths`, `patch_targets`, `patch_target_paths`).
  - Only `resolve` and `sensitive` stay in the extension trait; both may call
    the kernel's `patch_targets` helper when the policy is `PatchPaths`.
- Keeps in `tact`:
  - `PermissionPolicyExt` with `resolve(&self, input) -> CapabilityRisk` and
    `sensitive(&self, input, scanner) -> Option<Hit>`.
  - The security scanner, sensitive-path registry and shell read-only classifier.
- Re-exports:
  - `tact::tool::{ToolMetadata, PermissionPolicy, ...}` and `tact::permission::CapabilityRisk` continue to resolve.
  - `tact::agent::tool_schedule::ToolResources` continues to resolve.

- [ ] **Step 1: Write failing kernel tests for the moved data**

In `tact_kernel/src/tool/metadata.rs` tests, assert:
- `ToolMetadata::read_json("echo", "Echo", "Echo").permission == PermissionPolicy::Read`.
- `ToolMetadata::path_read("read_file", "Read", "Read File", "path").permission == PermissionPolicy::ReadPath { path_field: "path" }`.
- `ResourcePolicy::WritePath { field: "path" }.resolve(json!({"path":"src/lib.rs"}), Path::new("/repo"))` yields one write at `/repo/src/lib.rs`.
- `patch_target_paths("+++ b/src/lib.rs\n") == vec!["src/lib.rs"]`.

Run: `cargo test -p tact_kernel tool::metadata::tests`

Expected: compile error because the types do not exist in the kernel.

- [ ] **Step 2: Move the definitions**

Move the listed types and constructors into `tact_kernel`. In `tact/src/tool/metadata.rs`, replace the moved definitions with `pub use tact_kernel::tool::{...};`. Keep the module path `tact::tool::metadata` compiling. Move `ToolResources` and `ResourcePolicy` methods to the kernel; in `tact/src/agent/tool_schedule.rs`, use `tact_kernel::tool::ToolResources` (re-export it for old paths).

- [ ] **Step 3: Move policy classification behind `PermissionPolicyExt`**

Create `tact/src/permission/policy_ext.rs` with the trait and the moved helper functions (`target_is_workspace_relative`, `path_target_is_sensitive`, `patch_targets_are_sensitive`). Move `PermissionPolicy::target` and the patch-path parsers (`extract_patch_paths`, `patch_targets`, `patch_target_paths`) to the kernel: they only parse JSON/patch paths and have no security dependency. Keep only `resolve` and `sensitive` in `PermissionPolicyExt`; `sensitive` calls the kernel's public `patch_targets` helper for `PatchPaths`. Add the trait import to `agent/tool_dispatch.rs` and to permission tests so existing method-call syntax resolves.

- [ ] **Step 4: Run focused tests**

Run:
- `cargo test -p tact_kernel tool::metadata::tests`
- `cargo test -p tact --lib permission::tests`
- `cargo test -p tact --lib agent::tool_schedule::tests`

Expected: all pass; the policy extension tests preserve the existing `resolve`/`sensitive` assertions.

### Task 5: Move result, effect and neutral image types

**Files:**
- Create: `crates/tact_kernel/src/tool/result.rs`
- Modify: `crates/tact_kernel/src/tool/mod.rs`
- Modify: `crates/tact/src/tool/metadata.rs`
- Modify: `crates/tact/src/tool/mod.rs`
- Modify: `crates/tact/src/tool/read_image.rs`
- Modify: `crates/tact/src/tool/goal.rs`
- Modify: `crates/tact/src/agent/tool_dispatch.rs`
- Test: kernel result tests + `tact` read_image/tool_dispatch tests

**Interfaces:**
- Produces in `tact_kernel::tool`:

```rust
pub struct ToolImage {
    pub mime_type: String,
    pub data: String,
}

pub enum YieldReason {
    NeedsUser,
}

pub enum ToolEffect {
    CompactHistory { focus: Option<String> },
    YieldTurn { reason: YieldReason },
}

pub struct ToolCallResult {
    pub content: String,
    pub effects: Vec<ToolEffect>,
    pub image: Option<ToolImage>,
}

pub trait IntoToolCallResult {
    fn into_tool_call_result(self) -> ToolCallResult;
}
```

- `ToolCallResult::text`, `text_image(ToolImage)` and the `String`/`&'static str` implementations move.
- `tact::tool::GoalYieldReason` remains available as a type alias to `tact_kernel::tool::YieldReason` for source compatibility.
- `Agent` converts `ToolImage` to `tact_llm::ImageSource` at the message boundary; the kernel never imports `tact_llm`.

- [ ] **Step 1: Write failing result tests**

In `tact_kernel/src/tool/result.rs` tests, assert:
- `ToolCallResult::text("ok").image.is_none()`.
- `ToolCallResult::text_image("ok", ToolImage { mime_type: "image/png".into(), data: "AA==".into() }).image == Some(...)`.
- `ToolEffect::YieldTurn { reason: YieldReason::NeedsUser }` pattern-matches.
- `"ok".into_tool_call_result().content == "ok"`.

Run: `cargo test -p tact_kernel tool::result::tests`

Expected: compile error because the types do not exist.

- [ ] **Step 2: Move the result types and update `read_image`**

Replace `ToolCallResult`'s `image` field with `Option<ToolImage>`. In `read_image.rs`, construct `ToolImage { mime_type: media_type, data: b64 }`. In `tool_dispatch.rs`, change `ExecResult.image` to `Option<ToolImage>` and add:

```rust
fn to_image_source(image: ToolImage) -> tact_llm::ImageSource {
    tact_llm::ImageSource {
        type_: "base64".to_string(),
        media_type: image.mime_type,
        data: image.data,
    }
}
```

Call it where `ContentBlock::Image` is built. Rename `ToolEffect::GoalYield` to `ToolEffect::YieldTurn` and update the two goal-tool constructions and the dispatch match. Keep `AgentLoopStop::GoalYield` unchanged.

- [ ] **Step 3: Add the compatibility alias and imports**

In `tact/src/tool/mod.rs`, re-export the kernel result types and add `pub type GoalYieldReason = YieldReason;`. Update `agent/mod.rs`, `tool/goal.rs` and `agent/tool_dispatch.rs` to import the alias where the old name remains in use.

- [ ] **Step 4: Run focused tests**

Run:
- `cargo test -p tact_kernel tool::result::tests`
- `cargo test -p tact --lib tool::read_image`
- `cargo test -p tact --lib agent::tool_dispatch`
- `cargo test -p tact --lib agent::tests`

Expected: all pass; the read-image test still proves the base64 image is attached to the provider message.

### Task 6: Add descriptor contracts, catalogs, Registrar and PluginHost

**Files:**
- Create: `crates/tact_kernel/src/tool/descriptor.rs`
- Create: `crates/tact_kernel/src/capability/tool.rs`
- Create: `crates/tact_kernel/src/capability/command.rs`
- Create: `crates/tact_kernel/src/capability/prompt.rs`
- Create: `crates/tact_kernel/src/capability/view.rs`
- Create: `crates/tact_kernel/src/catalog.rs`
- Modify: `crates/tact_kernel/src/plugin.rs`
- Modify: `crates/tact_kernel/src/lib.rs`
- Test: `crates/tact_kernel/tests/conformance.rs`

**Interfaces:**
- Produces:

```rust
pub struct ToolDescriptor {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
    /// Owned policy fields; `ToolMetadata` itself is static and cannot carry
    /// runtime-discovered MCP names/display strings.
    pub policy: ToolPolicy,
}

pub trait Tool: Send + Sync {
    fn descriptor(&self) -> ToolDescriptor;
}

pub enum CommandScope {
    Runtime,
    View(String),
}

pub struct CommandMeta {
    pub name: String,
    pub aliases: Vec<String>,
    pub summary: String,
    pub usage: String,
    pub scope: CommandScope,
    pub needs_args: bool,
}

pub trait Command: Send + Sync {
    fn meta(&self) -> &CommandMeta;
}

pub struct PromptContext {
    pub workspace: PathBuf,
    pub session_id: Option<String>,
}

pub trait PromptSection: Send + Sync {
    fn id(&self) -> &'static str;
    fn priority(&self) -> i32;
    fn render(&self, ctx: &PromptContext) -> Option<String>;
}

pub trait View: Send + Sync {
    fn id(&self) -> &'static str;
}

pub trait Plugin: Send + Sync {
    fn manifest(&self) -> PluginManifest;
    fn register(&self, registrar: &mut Registrar<'_>) -> anyhow::Result<()>;
    async fn shutdown(&self) -> anyhow::Result<()> { Ok(()) }
}
```

- `Registrar::new(plugin: &PluginId, catalogs: &Catalogs, batch: &mut ContributionBatch, filter: &CapabilityFilter)`.
- `Registrar` methods: `tool`, `command`, `prompt_section`, `view`; each consults `CapabilityFilter` and records a `SkippedCapability` when disabled.
- `PluginHost::new(filter: CapabilityFilter)` and `PluginHost::skipped()`.
- `PluginHost::install(plugin)` stages contributions, commits only on success, and returns an error containing the plugin ID on failure.
- `PluginHost::seal()` validates catalogs (duplicate IDs were rejected at install) and returns `Runtime { tools, commands, prompts, views, plugins, skipped }`.
- `Runtime::shutdown()` calls plugin `shutdown` in reverse install order and is idempotent; `Runtime::skipped()` exposes registration diagnostics.
- Execution methods (`Tool::call`, `Command::run`, `View::run`) are intentionally added by Phase 1/4; this phase freezes registration only.

- [ ] **Step 1: Write the conformance test first**

Create `crates/tact_kernel/tests/conformance.rs` with fake `EchoTool`, `EchoCommand`, `EchoPrompt`, `EchoView` and `EchoPlugin`. Assert:
- After `install` + `seal`, each catalog returns the fake contribution.
- Installing a second plugin with the same tool name returns `Err` containing the second plugin ID, and the first catalog still has exactly one tool.
- A filter disabling `tool:echo` leaves `tools.get("echo") == None` and records a `SkippedCapability`.
- A plugin whose `register` returns `Err` leaves the host unusable; `seal` returns `Err`, and the previously installed plugin's shutdown was called.
- `shutdown` calls plugins in reverse install order.

Run: `cargo test -p tact_kernel --test conformance`

Expected: compile error because the host/catalogs do not exist.

- [ ] **Step 2: Implement the catalogs**

Use `BTreeMap` for tools/commands/views and a `Vec`/`BTreeMap` for prompt sections. Reject duplicate names/aliases/section IDs at `Registrar` time with an error that names the capability and the owning plugin. Keep `ToolDescriptor` owned; do not expose handler references through `descriptor()` beyond the trait object already held by the catalog.

- [ ] **Step 3: Implement staging and host lifecycle**

`PluginHost` holds installed plugin trait objects, manifests and catalogs. `Registrar` writes to a `ContributionBatch` (vectors of staged contributions and skipped diagnostics). `install` is `async`; on success it commits the batch and stores the plugin; on failure it drops the batch, calls the plugin's `shutdown`, and marks the host poisoned. `seal` returns `Runtime` and moves the plugin objects into it. `Runtime::shutdown` takes `&mut self`, sets an idempotent flag, iterates plugins in reverse and awaits each `shutdown`.

- [ ] **Step 4: Run the conformance test**

Run: `cargo test -p tact_kernel --test conformance`

Expected: all conformance cases pass.

### Task 7: Wire `tact` to the kernel and preserve public imports

**Files:**
- Modify: `crates/tact/src/lib.rs`
- Modify: `crates/tact/src/tool/mod.rs`
- Modify: `crates/tact/src/tool/metadata.rs`
- Modify: `crates/tact/src/permission/mod.rs`
- Modify: `crates/tact/src/agent/tool_schedule.rs`
- Create: `crates/tact/tests/plugin_contracts.rs`
- Modify: `scripts/check-rust.sh`

**Interfaces:**
- `tact` depends on `tact_kernel` with a path dependency.
- Old paths remain valid:
  - `tact::tool::{ToolMetadata, PermissionPolicy, ResourcePolicy, ToolCallResult, ToolImage, ToolResources}`
  - `tact::permission::CapabilityRisk`
  - `tact::agent::tool_schedule::ToolResources`
- `scripts/check-rust.sh` runs `cargo test -p tact_kernel` and the compatibility test.

- [ ] **Step 1: Write the compatibility test first**

Create `crates/tact/tests/plugin_contracts.rs` importing the old paths and asserting:

```rust
use tact::permission::CapabilityRisk;
use tact::tool::{PermissionPolicy, ToolCallResult, ToolImage, ToolMetadata};

#[test]
fn moved_contracts_keep_their_public_paths() {
    assert_eq!(ToolMetadata::read_json("echo", "Echo", "Echo").permission, PermissionPolicy::Read);
    assert_eq!(CapabilityRisk::Read, CapabilityRisk::Read);
    assert_eq!(ToolCallResult::text("ok").content, "ok");
    let _ = ToolImage { mime_type: "image/png".into(), data: "AA==".into() };
}
```

Run: `cargo test -p tact --test plugin_contracts`

Expected: compile error until the re-exports and dependency exist.

- [ ] **Step 2: Confirm the dependency and add re-exports**

`tact_kernel = { path = "../tact_kernel" }` is already present from Task 4; do not add a second entry. Re-export the moved types from the modules named above. Keep the existing `tact::tool::metadata` module path as a re-export shim rather than deleting it.

- [ ] **Step 3: Update the CI test list**

Add `-p tact_kernel` to the `cargo test` invocation in `scripts/check-rust.sh` and add `cargo test -p tact --test plugin_contracts --quiet`.

- [ ] **Step 4: Run the compatibility test and workspace check**

Run:
- `cargo test -p tact --test plugin_contracts`
- `cargo check --workspace`

Expected: both pass.

### Task 8: Add module-boundary guard and architecture docs

**Files:**
- Create: `scripts/check-module-boundaries.sh`
- Modify: `scripts/check-architecture.sh`
- Modify: `scripts/check-rust.sh`
- Modify: `ARCHITECTURE.md`
- Create: `docs/plugin-architecture.md`
- Test: the two guard scripts + `cargo test -p tact_kernel --test conformance`

**Interfaces:**
- `scripts/check-architecture.sh` enforces the crate whitelist from Task 1, including dev/test dependencies.
- `scripts/check-module-boundaries.sh` checks the critical `tact` module edges, with an explicit allowlist for known Phase-0 debt. It must fail if `tact/src/agent/` imports `crate::skill`, `crate::memory` or `crate::mcp` outside the recorded allowlist. The allowlist is removed as Phase 2 moves those calls.
- `scripts/check-rust.sh` runs both scripts and the kernel/compat tests.
- `ARCHITECTURE.md` lists `tact_kernel` as the kernel crate and shows `tact -> tact_kernel`.
- `docs/plugin-architecture.md` is a short pointer to the approved spec, the kernel/extension boundary, and the Phase-0 contract status. It must not claim Phase 1+ behavior is implemented.

- [ ] **Step 1: Write the failing module-boundary check**

Create `scripts/check-module-boundaries.sh` with Python that scans `crates/tact/src/agent/*.rs` for the three forbidden imports. Run it before adding the allowlist; it should fail with the current files named.

- [ ] **Step 2: Add the Phase-0 allowlist**

Record the exact current files/imports in the script as `PHASE0_ALLOWLIST`; make the script fail on any new violation. Run it again.

Expected: exits 0, printing the allowlist size.

- [ ] **Step 3: Wire CI and docs**

Add both guard scripts to `scripts/check-rust.sh`. Add the kernel row and dependency edge to `ARCHITECTURE.md`. Write `docs/plugin-architecture.md` with the boundary rules, the dependency whitelist, the Phase-0 status and links to the spec and this plan.

- [ ] **Step 4: Run the phase gate**

Run:
- `bash scripts/check-architecture.sh`
- `bash scripts/check-module-boundaries.sh`
- `cargo test -p tact_kernel`
- `cargo test -p tact --test plugin_contracts`
- `cargo check --workspace`

Expected: all pass. Do not run the full `scripts/check-rust.sh` until the human authorizes the commit/checkpoint; if they do, run it once at the end with the proxy exported.

## Follow-on plans

After this plan lands, write and execute the next plan from the same spec. Do not start Phase 1 in this plan. The first Phase 1 task must add `ToolExecutor`, `HookService`, `InteractionBroker` and the neutral tool-call context; the first production cutover must send native and MCP calls through the executor.

## Self-Review Notes

- Spec coverage for Phase 0: plugin mechanism, capability IDs/filter, metadata/policy/result contracts, registration catalogs, compatibility re-exports, architecture/module guards.
- Deferred by scope: execution traits and services (Phase 1/3/4), domain plugin migration (Phase 2), command/view execution (Phase 4), docs/book chapter sync (Phase 5).
- Review Focus mapping: duplicate IDs and rollback in Task 6; filter in Tasks 3/6; re-exports in Task 7; image/effect in Task 5; permission method resolution in Task 4.
