# Native Tool Metadata Refactor Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace every native-tool-name semantic branch with self-described typed metadata, typed presentation data, and typed post-call effects while preserving all external tool names, schemas, configuration, and observable behavior.

**Architecture:** Each native `Tool` exposes one static `ToolMetadata`; `ToolRouter` binds that metadata to the handler and resolves the external name once. Agent scheduling, permission, argument formatting, output handling, task behavior, and TUI rendering consume typed policies carried through `ResolvedTool`, `ToolCallResult`, and protocol `ToolPresentationInfo`; MCP remains a separate dynamic conservative boundary.

**Tech Stack:** Rust 2024, `async_trait`, `serde_json`, `schemars`, proc macros (`syn`/`quote`), Tokio, ratatui, existing Tact protocol/TUI crates.

## Global Constraints

- Keep every LLM-facing native tool name, description, and JSON input schema byte-for-byte compatible with the pre-refactor behavior.
- Keep persisted/session tool names and `always_allowed_tools` entries as stable external strings; do not serialize Rust enum variant names.
- Every registered native tool must provide complete explicit metadata; no implicit native fallback is allowed.
- After native router resolution, permissions, scheduling, argument summaries, details, output persistence, task behavior, effects, and TUI behavior must not match native tool-name strings.
- MCP parsing remains isolated to the external `mcp__{server}__{tool}` adapter and uses conservative permission, generic presentation, no effects, and same-server serialization.
- Unknown tool names fail closed before execution and cannot receive native permissions, presentation privileges, or effects.
- Preserve current tool-card visuals and behavior, including task titles, command prefixes, file gutters, ask-user meta labels, subagent transcript popup, ToolMeta model/token fields, and live-preview caps.
- Apply `ToolEffect` only after the handler and any required output persistence both succeed; blocked, denied, malformed, failed, and MCP calls produce no effects.
- Do not add a metadata attribute DSL. Bare `#[tool]` derives the sibling `UPPER_SNAKE_HANDLER_METADATA` constant name.
- Follow TDD for every behavior change: add one focused failing test, run it and confirm the expected failure, then implement the minimum production change.
- Do not include the currently uncommitted thinking-preview bug fix in any metadata-refactor task commit; commit only the exact files listed by each task.

## File Map

**Create**

- `crates/tact/src/tool/metadata.rs` — core native metadata, policies, argument/detail/prompt/resource resolution, effects, and result conversion.
- `docs/superpowers/plans/2026-07-27-tool-metadata-refactor.md` — this implementation plan.

**Modify**

- `crates/tool_refactor_macros/src/lib.rs` — generate `metadata()` and `ToolCallResult` wrappers from bare `#[tool]`.
- `crates/protocol/src/agent.rs`, `crates/protocol/src/lib.rs` — public pure-data tool presentation types carried by step events/results.
- `crates/tact/src/tool/mod.rs`, `crates/tact/src/tool/registry.rs`, `crates/tact/src/tool/test_support.rs` — metadata-aware trait/router/resolution and compatibility helpers.
- Every production file in `crates/tact/src/tool/*.rs` containing `#[tool]` — declare sibling metadata constants and use bare `#[tool]`.
- `crates/tact/src/permission/mod.rs` — accept resolved risk/prompt policy instead of classifying native names.
- `crates/tact/src/mcp/mod.rs` — expose parsed MCP identity at the boundary rather than repeated prefix parsing.
- `crates/tact/src/agent/tool_schedule.rs`, `crates/tact/src/agent/tool_dispatch.rs` — consume resolved metadata, structured results, policies, and effects.
- `crates/tact/src/task/display.rs`, `crates/tact/src/task/mod.rs` — accept `TaskOperation`, never task-tool names.
- `crates/tui/src/widgets/tool_widget.rs`, `crates/tui/src/widgets/state/app/agent.rs`, `crates/tui/src/widgets/state/app/popups.rs` — render only from protocol presentation data.
- All test fixtures constructing `AgentUpdate::StepStarted` or `StepResult` — add explicit generic/specialized presentation fixtures.
- `ARCHITECTURE.md` — document self-described native tools and the MCP boundary.

---

### Task 1: Add protocol-level tool presentation data

- Modify: `crates/protocol/src/agent.rs`, `crates/protocol/src/lib.rs`
- Modify: `crates/tact/src/agent/tool_dispatch.rs` — add generic presentation to temporary test/event literals only until Task 5 supplies resolved presentation.
- Modify: `crates/tact/src/tool/subagent_ui.rs` — add generic presentation to its synthetic step fixtures.
- Modify: `crates/tui/src/handlers/mouse.rs`
- Modify: `crates/tui/src/render/layout.rs`
- Modify: `crates/tui/src/render/log_render_tests.rs`
- Modify: `crates/tui/src/render/popup_scene_tests.rs`
- Modify: `crates/tui/src/render/render_gap_tests.rs`
- Modify: `crates/tui/src/render/scene_tests.rs`
- Modify: `crates/tui/src/widgets/state/app/agent.rs`
- Modify: `crates/tui/src/widgets/state/app/popups.rs`
- Modify: `crates/tui/src/widgets/tool_widget.rs`
- Test: `crates/protocol/src/agent.rs`

**Interfaces:**
- Consumes: existing `AgentUpdate::StepStarted`, `StepResult`.
- Produces:
  - `ToolVisualKind`
  - `ToolDetailKind`
  - `ToolPopupKind`
  - `ToolPresentationInfo`
  - `ToolPresentationInfo::generic(name: impl Into<String>) -> Self`
  - `AgentUpdate::StepStarted::presentation`
  - `StepResult::presentation`

- [ ] **Step 1: Add a failing protocol test for the generic presentation contract**

Append under `#[cfg(test)]` in `crates/protocol/src/agent.rs`:

```rust
#[test]
fn generic_tool_presentation_has_no_native_privileges() {
    let presentation = ToolPresentationInfo::generic("mcp__demo__search");

    assert_eq!(presentation.visual_kind, ToolVisualKind::Generic);
    assert_eq!(presentation.display_name, "mcp__demo__search");
    assert_eq!(presentation.detail, ToolDetailKind::Result);
    assert_eq!(presentation.popup, ToolPopupKind::None);
    assert!(!presentation.keep_full_live_output);
    assert!(!presentation.compact_result_to_meta);
}
```

- [ ] **Step 2: Run the test and verify RED**

Run:

```bash
cargo test -p tact_protocol agent::tests::generic_tool_presentation_has_no_native_privileges -- --exact --nocapture
```

Expected: compile failure because `ToolPresentationInfo` and its enums do not exist.

- [ ] **Step 3: Add the protocol presentation types**

Add before `StepResult` in `crates/protocol/src/agent.rs`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ToolVisualKind {
    #[default]
    Generic,
    FileWrite,
    FileRead,
    FileEdit,
    Command,
    Task,
    Subagent,
    Sleep,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ToolDetailKind {
    #[default]
    None,
    Result,
    InputField(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ToolPopupKind {
    #[default]
    None,
    SubagentTranscript,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolPresentationInfo {
    pub visual_kind: ToolVisualKind,
    pub display_name: String,
    pub keep_full_live_output: bool,
    pub detail: ToolDetailKind,
    pub popup: ToolPopupKind,
    pub compact_result_to_meta: bool,
}

impl ToolPresentationInfo {
    pub fn generic(name: impl Into<String>) -> Self {
        Self {
            visual_kind: ToolVisualKind::Generic,
            display_name: name.into(),
            keep_full_live_output: false,
            detail: ToolDetailKind::Result,
            popup: ToolPopupKind::None,
            compact_result_to_meta: false,
        }
    }
}
```

Add fields:

```rust
pub struct StepResult {
    // existing fields
    pub presentation: ToolPresentationInfo,
}
```

```rust
AgentUpdate::StepStarted {
    // existing fields
    presentation: ToolPresentationInfo,
},
```

Re-export the four new types from `crates/protocol/src/lib.rs`.

- [ ] **Step 4: Update protocol construction fixtures only enough to compile**

At every `StepResult` and `AgentUpdate::StepStarted` literal outside production dispatch, add the new `presentation` field while preserving every existing field and value:

```rust
presentation: ToolPresentationInfo::generic(result.tool.clone()),
```

For a `StepStarted` literal whose tool name is stored in `tool_name`, use:

```rust
presentation: ToolPresentationInfo::generic(tool_name.clone()),
```

For a literal string, pass that exact string to `generic`. Do not introduce special native behavior yet; specialized presentation comes in Task 6.

- [ ] **Step 5: Run protocol and workspace compile tests**

Run:

```bash
cargo test -p tact_protocol
cargo check --workspace --all-targets
```

Expected: protocol tests pass; workspace compiles with generic presentation fixtures.

- [ ] **Step 6: Commit only protocol and fixture compatibility changes**

```bash
git add \
  crates/protocol/src/agent.rs crates/protocol/src/lib.rs \
  crates/tact/src/agent/tool_dispatch.rs crates/tact/src/tool/subagent_ui.rs \
  crates/tui/src/handlers/mouse.rs crates/tui/src/render/layout.rs \
  crates/tui/src/render/log_render_tests.rs crates/tui/src/render/popup_scene_tests.rs \
  crates/tui/src/render/render_gap_tests.rs crates/tui/src/render/scene_tests.rs \
  crates/tui/src/widgets/state/app/agent.rs crates/tui/src/widgets/state/app/popups.rs \
  crates/tui/src/widgets/tool_widget.rs
# Verify the staged list contains no thinking-preview or Ch 26 paths before committing.
git diff --cached --name-only
git commit -m "refactor(protocol): carry tool presentation metadata"
```

---

### Task 2: Define native metadata, policy resolution, and structured tool results

**Files:**
- Create: `crates/tact/src/tool/metadata.rs`
- Modify: `crates/tact/src/tool/mod.rs`
- Test: `crates/tact/src/tool/metadata.rs`

**Interfaces:**
- Consumes: protocol types from Task 1; `CapabilityRisk`; `ToolResources`; JSON input.
- Produces:
  - `ToolMetadata`
  - `PermissionPolicy::resolve(&Value) -> CapabilityRisk`
  - `PermissionPromptPolicy::format(name, &Value) -> String`
  - `ResourcePolicy::resolve(&Value, &Path) -> ToolResources`
  - `ResourcePolicy::recent_paths(&Value) -> Vec<String>`
  - `ArgumentSummaryPolicy::format(&Value) -> String`
  - `DetailPolicy::resolve(&Value, &str) -> Option<String>`
  - `ToolDomain`, `TaskOperation`
  - `ToolPresentation::to_protocol() -> ToolPresentationInfo`
  - `OutputPolicy`, `ToolEffect`, `ToolCallResult`
  - `IntoToolCallResult`

- [ ] **Step 1: Write failing policy tests**

Create `crates/tact/src/tool/metadata.rs` with a test module that specifies the public API before implementation:

```rust
#[cfg(test)]
mod tests {
    use std::path::Path;
    use serde_json::json;
    use super::*;

    #[test]
    fn path_resource_policy_resolves_and_reports_recent_path() {
        let policy = ResourcePolicy::WritePath { field: "path" };
        let input = json!({ "path": "src/lib.rs" });
        let resources = policy.resolve(&input, Path::new("/repo"));

        assert_eq!(resources.writes, vec![Path::new("/repo/src/lib.rs")]);
        assert_eq!(policy.recent_paths(&input), vec!["src/lib.rs"]);
    }

    #[test]
    fn shell_permission_policy_preserves_read_and_high_risk_rules() {
        let policy = PermissionPolicy::ShellCommand { command_field: "command" };
        assert_eq!(policy.resolve(&json!({"command": "git status"})), CapabilityRisk::Read);
        assert_eq!(policy.resolve(&json!({"command": "sudo ls"})), CapabilityRisk::High);
    }

    #[test]
    fn adjacent_native_presentation_maps_to_protocol_without_name_matching() {
        let presentation = ToolPresentation {
            visual_kind: ToolVisualKind::Subagent,
            display_name: "🤖 Subagent",
            live_output: LiveOutputPolicy::FullTranscript,
            detail: DetailPolicy::Result,
            popup: PopupPolicy::SubagentTranscript,
            compact_result_to_meta: false,
        };
        let info = presentation.to_protocol();
        assert!(info.keep_full_live_output);
        assert_eq!(info.popup, ToolPopupKind::SubagentTranscript);
    }

    #[test]
    fn string_converts_to_effect_free_tool_result() {
        let result = "ok".to_string().into_tool_call_result();
        assert_eq!(result.content, "ok");
        assert!(result.effects.is_empty());
    }
}
```

- [ ] **Step 2: Run tests and verify RED**

```bash
cargo test -p tact tool::metadata::tests -- --nocapture
```

Expected: compile failure because metadata types do not exist.

- [ ] **Step 3: Implement the closed policy enums and data types**

Implement the exact types approved by the spec:

```rust
pub struct ToolMetadata {
    pub name: &'static str,
    pub description: &'static str,
    pub permission: PermissionPolicy,
    pub permission_prompt: PermissionPromptPolicy,
    pub resources: ResourcePolicy,
    pub domain: ToolDomain,
    pub presentation: ToolPresentation,
    pub output: OutputPolicy,
    pub argument_summary: ArgumentSummaryPolicy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionPolicy {
    Read,
    Write,
    High,
    ShellCommand { command_field: &'static str },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionPromptPolicy {
    Json,
    Question { field: &'static str },
    Command { field: &'static str },
    Path { field: &'static str },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourcePolicy {
    Independent,
    Barrier,
    ReadPath { field: &'static str },
    WritePath { field: &'static str },
    SharedState { scope: &'static str },
    PatchFiles { patch_field: &'static str, dry_run_field: &'static str },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolDomain {
    Generic,
    Task(TaskOperation),
    Subagent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskOperation { Create, Get, List, Update }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveOutputPolicy { Standard, FullTranscript }
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetailPolicy { None, Result, InputField(&'static str) }
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PopupPolicy { None, SubagentTranscript }
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputPolicy { PersistLargeOutput, KeepInline }
```

Use the complete `ArgumentSummaryPolicy` variants from the approved spec. Keep formatting helpers private to `metadata.rs`; move the current `patch_title`, `body_preview`, truncation, and path extraction logic without behavior changes.

- [ ] **Step 4: Implement presentation conversion and structured results**

```rust
pub struct ToolPresentation {
    pub visual_kind: ToolVisualKind,
    pub display_name: &'static str,
    pub live_output: LiveOutputPolicy,
    pub detail: DetailPolicy,
    pub popup: PopupPolicy,
    pub compact_result_to_meta: bool,
}

impl ToolPresentation {
    pub fn to_protocol(self) -> ToolPresentationInfo {
        ToolPresentationInfo {
            visual_kind: self.visual_kind,
            display_name: self.display_name.to_string(),
            keep_full_live_output: matches!(self.live_output, LiveOutputPolicy::FullTranscript),
            detail: match self.detail {
                DetailPolicy::None => ToolDetailKind::None,
                DetailPolicy::Result => ToolDetailKind::Result,
                DetailPolicy::InputField(field) => ToolDetailKind::InputField(field.to_string()),
            },
            popup: match self.popup {
                PopupPolicy::None => ToolPopupKind::None,
                PopupPolicy::SubagentTranscript => ToolPopupKind::SubagentTranscript,
            },
            compact_result_to_meta: self.compact_result_to_meta,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolEffect {
    CompactHistory { focus: Option<String> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCallResult {
    pub content: String,
    pub effects: Vec<ToolEffect>,
}

impl ToolCallResult {
    pub fn text(content: impl Into<String>) -> Self {
        Self { content: content.into(), effects: Vec::new() }
    }
}

pub trait IntoToolCallResult {
    fn into_tool_call_result(self) -> ToolCallResult;
}

impl IntoToolCallResult for String {
    fn into_tool_call_result(self) -> ToolCallResult {
        ToolCallResult::text(self)
    }
}

impl IntoToolCallResult for &'static str {
    fn into_tool_call_result(self) -> ToolCallResult {
        ToolCallResult::text(self)
    }
}

impl IntoToolCallResult for ToolCallResult {
    fn into_tool_call_result(self) -> ToolCallResult {
        self
    }
}
```

- [ ] **Step 5: Make `ToolResources` constructors crate-visible and wire module exports**

In `tool_schedule.rs`, change `barrier()` and `independent()` to `pub(crate)` so `ResourcePolicy` owns native resolution. In `tool/mod.rs`, add `mod metadata;` and re-export the types consumed by tool modules, agent dispatch, permission, task display, and the proc-macro output.

- [ ] **Step 6: Run focused tests and lint**

```bash
cargo test -p tact tool::metadata::tests -- --nocapture
cargo clippy -p tact --all-targets -- -D warnings
```

Expected: all metadata tests pass and clippy reports no warnings.

- [ ] **Step 7: Commit**

```bash
git add crates/tact/src/tool/metadata.rs crates/tact/src/tool/mod.rs crates/tact/src/agent/tool_schedule.rs
git commit -m "refactor(tool): add typed native metadata policies"
```

---

### Task 3: Make the proc macro and router metadata-aware

**Files:**
- Modify: `crates/tool_refactor_macros/src/lib.rs`
- Modify: `crates/tact/src/tool/mod.rs`
- Modify: `crates/tact/src/tool/test_support.rs`
- Test: `crates/tact/src/tool/mod.rs`
- Test: `crates/tool_refactor_macros/src/lib.rs`

**Interfaces:**
- Consumes: `ToolMetadata`, `ToolCallResult`, `IntoToolCallResult` from Task 2.
- Produces:
  - `Tool::metadata() -> &'static ToolMetadata`
  - `Tool::call(&self, context: ToolContext, input: Value) -> Result<ToolCallResult>`
  - `RegisteredTool`
  - `ResolvedNativeTool<'a>`
  - `ToolRouter::resolve(&str) -> Result<ResolvedNativeTool<'_>>`
  - `ToolRouter::call_result(&ToolContext, &str, Value) -> Result<ToolCallResult>`
  - `ToolRouter::call(&ToolContext, &str, Value) -> Result<String>` (existing compatibility convenience wrapper)

- [ ] **Step 1: Write failing router tests for metadata identity and duplicates**

Replace the test-only `EchoTool` implementation with an `ECHO_METADATA` constant and add:

```rust
#[test]
fn router_resolves_handler_and_metadata_together() {
    let router = ToolRouter::new().route(EchoTool).unwrap();
    let resolved = router.resolve("echo").unwrap();
    assert_eq!(resolved.metadata().name, "echo");
}

#[test]
fn router_rejects_duplicate_native_names() {
    let error = ToolRouter::new()
        .route(EchoTool).unwrap()
        .route(EchoTool).unwrap_err();
    assert!(error.to_string().contains("duplicate native tool name: echo"));
}
```

- [ ] **Step 2: Run and verify RED**

```bash
cargo test -p tact tool::tests::router_resolves_handler_and_metadata_together -- --exact --nocapture
```

Expected: compile failure because route is not fallible and resolve/metadata do not exist.

- [ ] **Step 3: Change `Tool` and `ToolRouter`**

Implement:

```rust
#[async_trait]
pub trait Tool: Send + Sync {
    fn metadata(&self) -> &'static ToolMetadata;
    fn input_schema(&self) -> Value;
    async fn call(&self, context: ToolContext, input: Value) -> Result<ToolCallResult>;

    fn tool_spec(&self) -> ToolSpec {
        let metadata = self.metadata();
        ToolSpec {
            name: metadata.name.to_string(),
            description: Some(metadata.description.to_string()),
            input_schema: self.input_schema(),
        }
    }
}

struct RegisteredTool {
    handler: Box<dyn Tool>,
    metadata: &'static ToolMetadata,
}

#[derive(Clone, Copy)]
pub struct ResolvedNativeTool<'a> {
    registered: &'a RegisteredTool,
}

impl ResolvedNativeTool<'_> {
    pub fn metadata(self) -> &'static ToolMetadata;
    pub async fn call(self, context: ToolContext, input: Value) -> Result<ToolCallResult>;
}
```

`ToolRouter::route` returns `Result<Self>`, rejects empty and duplicate names, and each existing test chain in `crates/tact/src/tool/mod.rs`, `crates/tact/src/tool/task.rs`, and `crates/tact/src/tool/test_support.rs` adds `?`/`unwrap()` at registration. `toolset()` and `subagent_toolset()` use the explicit invariant assertion only at the final built-in assembly boundary.

- [ ] **Step 4: Change proc macro parsing to bare `#[tool]`**

Remove `name`/`description` parsing. Derive the metadata identifier from the function name:

```rust
let metadata_ident = format_ident!("{}_METADATA", fn_ident.to_string().to_uppercase());
```

Generated implementations must contain:

```rust
fn metadata(&self) -> &'static crate::tool::ToolMetadata {
    &#metadata_ident
}

async fn call(
    &self,
    context: crate::tool::ToolContext,
    input: serde_json::Value,
) -> anyhow::Result<crate::tool::ToolCallResult> {
    let input: InputType = serde_json::from_value(input)?;
    let output = handler(context, input).await?;
    Ok(crate::tool::IntoToolCallResult::into_tool_call_result(output))
}
```

Keep current support for stateful `(ToolContext, Input)` and plain named arguments. Add macro unit tests for `to_pascal_case` and new upper-snake metadata identifier generation, including `spawn_subagent -> SPAWN_SUBAGENT_METADATA`.

- [ ] **Step 5: Update `test_support::run_tool` compatibility helper**

Keep its external `Result<String>` contract for existing unit tests:

```rust
let result = ToolRouter::new()
    .route(tool)?
    .call(context, name, input)
    .await?;
Ok(result.content)
```

Add a separate `run_tool_result` helper returning `ToolCallResult` for compact effect tests.

- [ ] **Step 6: Run router/macro tests**

```bash
cargo test -p tact tool::tests -- --nocapture
cargo test -p tool_refactor_macros
```

Expected: tests pass.

- [ ] **Step 7: Commit**

```bash
git add \
  crates/tool_refactor_macros/src/lib.rs crates/tact/src/tool/mod.rs \
  crates/tact/src/tool/test_support.rs
# Verify no crates/tui or book/26_chapter_issue paths are staged.
git diff --cached --name-only
git commit -m "refactor(tool): bind metadata in generated handlers"
```

---

### Task 4: Declare metadata beside every native tool

**Files:**
- Modify: `crates/tact/src/tool/apply_patch.rs`
- Modify: `crates/tact/src/tool/ask_user.rs`
- Modify: `crates/tact/src/tool/background_run.rs`
- Modify: `crates/tact/src/tool/bash.rs`
- Modify: `crates/tact/src/tool/compact.rs`
- Modify: `crates/tact/src/tool/cron.rs`
- Modify: `crates/tact/src/tool/edit_file.rs`
- Modify: `crates/tact/src/tool/load_skill.rs`
- Modify: `crates/tact/src/tool/memory.rs`
- Modify: `crates/tact/src/tool/read_file.rs`
- Modify: `crates/tact/src/tool/sleep.rs`
- Modify: `crates/tact/src/tool/subagent.rs`
- Modify: `crates/tact/src/tool/task.rs`
- Modify: `crates/tact/src/tool/team.rs`
- Modify: `crates/tact/src/tool/worktree.rs`
- Modify: `crates/tact/src/tool/write_file.rs`
- Modify: `crates/tact/src/tool/registry.rs`
- Test: `crates/tact/src/tool/registry.rs`

**Interfaces:**
- Consumes: metadata types and bare `#[tool]` macro.
- Produces: one `UPPER_SNAKE_HANDLER_METADATA` constant per native handler; unchanged main/subagent external tool specs.

- [ ] **Step 1: Add a failing registry compatibility test with the exact tool sets**

In `registry.rs` tests, define the expected sorted main list and exact subagent list:

```rust
const MAIN_TOOL_NAMES: &[&str] = &[
    "apply_patch", "ask_user", "background_run", "bash", "broadcast",
    "check_background", "compact", "cron_create", "cron_delete", "cron_list",
    "edit_file", "list_teammates", "load_skill", "plan_approval", "read_file",
    "read_inbox", "save_memory", "send_message", "shutdown_request",
    "shutdown_response", "sleep", "spawn_subagent", "spawn_teammate",
    "task_create", "task_get", "task_list", "task_update", "worktree_create",
    "worktree_events", "worktree_list", "worktree_run", "worktree_status",
    "write_file",
];

#[test]
fn builtin_metadata_preserves_external_tool_sets() {
    let mut main = toolset().tool_specs().into_iter().map(|s| s.name).collect::<Vec<_>>();
    main.sort();
    assert_eq!(main, MAIN_TOOL_NAMES);
    let mut child = subagent_toolset().tool_specs().into_iter().map(|s| s.name).collect::<Vec<_>>();
    child.sort();
    assert_eq!(child, ["bash", "edit_file", "read_file", "sleep", "write_file"]);
}
```

- [ ] **Step 2: Run and verify RED**

```bash
cargo test -p tact tool::registry::tests::builtin_metadata_preserves_external_tool_sets -- --exact --nocapture
```

Expected: compile failure because current tool modules do not declare metadata constants for bare macro expansion.

- [ ] **Step 3: Add metadata constants and convert attributes to bare `#[tool]`**

For every handler, add a `pub const HANDLER_METADATA: ToolMetadata` immediately before `#[tool]`. The identifier must be the uppercase function name followed by `_METADATA`, for example `READ_FILE_METADATA` and `SPAWN_SUBAGENT_METADATA`. Representative constants:

```rust
pub const READ_FILE_METADATA: ToolMetadata = ToolMetadata {
    name: "read_file",
    description: "Read file contents.",
    permission: PermissionPolicy::Read,
    permission_prompt: PermissionPromptPolicy::Path { field: "path" },
    resources: ResourcePolicy::ReadPath { field: "path" },
    domain: ToolDomain::Generic,
    presentation: ToolPresentation {
        visual_kind: ToolVisualKind::FileRead,
        display_name: "📖 Read",
        live_output: LiveOutputPolicy::Standard,
        detail: DetailPolicy::Result,
        popup: PopupPolicy::None,
        compact_result_to_meta: false,
    },
    output: OutputPolicy::KeepInline,
    argument_summary: ArgumentSummaryPolicy::Path { field: "path" },
};

#[tool]
For each handler, leave the existing function signature and body unchanged. The only source edits in these handler definitions are the sibling metadata constant and replacing the existing attribute arguments (`name = "...", description = "..."`) with the bare `#[tool]` attribute; the proc macro supplies the wrapper metadata and result conversion.
```

```rust
pub const SPAWN_SUBAGENT_METADATA: ToolMetadata = ToolMetadata {
    name: "spawn_subagent",
    description: "Spawn a subagent with fresh context. It shares the filesystem but not conversation history.",
    permission: PermissionPolicy::High,
    permission_prompt: PermissionPromptPolicy::Json,
    resources: ResourcePolicy::Barrier,
    domain: ToolDomain::Subagent,
    presentation: ToolPresentation {
        visual_kind: ToolVisualKind::Subagent,
        display_name: "🤖 Subagent",
        live_output: LiveOutputPolicy::FullTranscript,
        detail: DetailPolicy::Result,
        popup: PopupPolicy::SubagentTranscript,
        compact_result_to_meta: false,
    },
    output: OutputPolicy::PersistLargeOutput,
    argument_summary: ArgumentSummaryPolicy::SubagentPrompt { field: "prompt" },
};
```

Assign all remaining policies by copying the current behavior from the approved spec and the pre-refactor name matches. Do not simplify labels or summaries. `apply_patch` uses `PatchFiles`; task tools use `SharedState` plus the correct `TaskOperation`; ask_user uses `compact_result_to_meta: true`; shell uses command policies; sleep is independent.

- [ ] **Step 4: Make `compact` return its typed effect**

Change only `compact` to:

```rust
pub async fn compact(_ctx: ToolContext, input: CompactInput) -> Result<ToolCallResult> {
    let focus = input.focus;
    let suffix = focus
        .as_ref()
        .map(|focus| format!(" Focus to preserve: {focus}"))
        .unwrap_or_default();
    Ok(ToolCallResult {
        content: format!("Compacting conversation...{suffix}"),
        effects: vec![ToolEffect::CompactHistory { focus }],
    })
}
```

Add a test through `run_tool_result` asserting the content and exact effect.

- [ ] **Step 5: Make registry assembly handle fallible route once**

Define `try_toolset() -> anyhow::Result<ToolRouter>` with the complete main chain:

```rust
fn try_toolset() -> anyhow::Result<ToolRouter> {
    Ok(ToolRouter::new()
        .route(ApplyPatchTool)?
        .route(AskUserTool)?
        .route(BashTool)?
        .route(BackgroundRunTool)?
        .route(CheckBackgroundTool)?
        .route(CronCreateTool)?
        .route(CronDeleteTool)?
        .route(CronListTool)?
        .route(ReadFileTool)?
        .route(SleepTool)?
        .route(WriteFileTool)?
        .route(EditFileTool)?
        .route(LoadSkillTool)?
        .route(SaveMemoryTool)?
        .route(CompactTool)?
        .route(SpawnSubagentTool)?
        .route(TaskCreateTool)?
        .route(TaskGetTool)?
        .route(TaskListTool)?
        .route(TaskUpdateTool)?
        .route(SpawnTeammateTool)?
        .route(ListTeammatesTool)?
        .route(SendMessageTool)?
        .route(BroadcastTool)?
        .route(ReadInboxTool)?
        .route(PlanApprovalTool)?
        .route(ShutdownRequestTool)?
        .route(ShutdownResponseTool)?
        .route(WorktreeCreateTool)?
        .route(WorktreeListTool)?
        .route(WorktreeStatusTool)?
        .route(WorktreeRunTool)?
        .route(WorktreeEventsTool)?)
}

pub fn toolset() -> ToolRouter {
    try_toolset().expect("built-in tool metadata must be valid")
}
```

Build `subagent_toolset()` through the same fallible helper shape, chaining exactly `BashTool`, `ReadFileTool`, `SleepTool`, `WriteFileTool`, and `EditFileTool`; expose the existing infallible function with `expect("subagent tool metadata must be valid")`.

- [ ] **Step 6: Run all native tool and registry tests**

```bash
cargo test -p tact tool -- --nocapture
cargo clippy -p tact --all-targets -- -D warnings
```

Expected: all tool tests pass; exact name sets unchanged.

- [ ] **Step 7: Commit all native declarations as one mechanical compatibility unit**

```bash
git add crates/tact/src/tool
# Explicitly unstage the unrelated thinking files if present:
git restore --staged crates/tui/src/render/cells/thinking.rs crates/tui/src/widgets/state/thinking_state.rs crates/tui/src/widgets/state/mod.rs book/26_chapter_issue.md book/26_chapter_issue_zh.md
git commit -m "refactor(tool): declare builtin metadata beside handlers"
```

---

### Task 5: Move permissions, scheduling, arguments, task display, output, and effects to metadata

**Files:**
- Modify: `crates/tact/src/permission/mod.rs`
- Modify: `crates/tact/src/mcp/mod.rs`
- Modify: `crates/tact/src/agent/tool_schedule.rs`
- Modify: `crates/tact/src/agent/tool_dispatch.rs`
- Modify: `crates/tact/src/task/display.rs`
- Modify: `crates/tact/src/task/mod.rs`
- Test: the same files

**Interfaces:**
- Consumes: `ResolvedNativeTool`, `ToolMetadata` policies, `ToolCallResult`, `ToolEffect`, `TaskOperation`.
- Produces:
  - `ResolvedTool<'a> = Native(ResolvedNativeTool<'a>) | Mcp(ResolvedMcpTool) | Unknown`
  - `PermissionManager::check(name, risk)`
  - `format_permission_prompt(name, policy, input)`
  - metadata-driven `PreparedTool`
  - ordered effect application.

- [ ] **Step 1: Write failing permission tests that no longer pass a native name for classification**

Replace representative tests with:

```rust
#[test]
fn default_mode_allows_resolved_read_capability() {
    let mut manager = PermissionManager::try_new(PermissionMode::Default).unwrap();
    let decision = manager.check("read_file", CapabilityRisk::Read);
    assert_eq!(decision.behavior, PermissionBehavior::Allow);
}

#[test]
fn path_prompt_policy_preserves_file_prompt() {
    let prompt = format_permission_prompt(
        "edit_file",
        PermissionPromptPolicy::Path { field: "path" },
        &json!({"path": "src/lib.rs"}),
    );
    assert_eq!(prompt, "Allow edit_file on src/lib.rs?");
}
```

Keep MCP prefix risk tests under a renamed `normalize_mcp_capability`; delete native name classification tests after equivalent policy tests exist in `metadata.rs`.

- [ ] **Step 2: Run and verify RED**

```bash
cargo test -p tact permission::tests::default_mode_allows_resolved_read_capability -- --exact --nocapture
```

Expected: compile failure because `check` still accepts JSON and derives risk from name.

- [ ] **Step 3: Refactor permission API**

Change:

```rust
pub fn check(&mut self, stable_name: &str, risk: CapabilityRisk) -> PermissionDecision
```

Retain exact behavior ordering: Read auto-allows; High always asks before allow-list; exact stable-name allow-list applies to non-high; then mode behavior. Make `format_permission_prompt` accept `PermissionPromptPolicy` and remove all native builtin strings from `permission/mod.rs`.

- [ ] **Step 4: Expose structured MCP resolution once**

Make the existing private `McpToolName` available through:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedMcpTool {
    pub full_name: String,
    pub server: String,
    pub tool: String,
}

pub fn resolve_tool(&self, name: &str) -> anyhow::Result<Option<ResolvedMcpTool>>;
```

Only names with the MCP prefix are parsed; invalid prefixed names return an error. The MCP dynamic risk classifier consumes `resolved.tool`, and scheduling consumes `resolved.server` without reparsing the full name.

- [ ] **Step 5: Write failing task display tests using `TaskOperation`**

Change task display tests to call:

```rust
format_task_tool_title(TaskOperation::Update, &input, before.as_ref(), after.as_ref())
```

Run:

```bash
cargo test -p tact task::display::tests -- --nocapture
```

Expected: compile failure because APIs still accept `&str`.

- [ ] **Step 6: Refactor task display APIs**

Change `format_task_tool_title`, `format_task_tool_title_with_manager`, and `primary_action` to accept `TaskOperation`. Replace each string branch with exhaustive enum matching. Delete `is_task_tool` and remove its re-export.

- [ ] **Step 7: Write failing dispatch tests against policy APIs**

Replace the current name-driven unit tests with metadata policy tests at the dispatch boundary:

```rust
#[test]
fn detail_policy_uses_result_without_tool_name() {
    assert_eq!(
        step_result_detail(DetailPolicy::Result, &json!({}), "hi\n", &StepStatus::Success),
        Some("hi\n".to_string())
    );
}

#[test]
fn patch_resource_policy_reports_changed_files_without_tool_name() {
    let policy = ResourcePolicy::PatchFiles { patch_field: "patch", dry_run_field: "dry_run" };
    assert_eq!(policy.recent_paths(&json!({"patch": "+++ b/src/a.rs\n", "dry_run": false})), vec!["src/a.rs"]);
}
```

- [ ] **Step 8: Introduce resolved/prepared tool data and migrate pre-flight**

Define:

```rust
enum ResolvedTool<'a> {
    Native(ResolvedNativeTool<'a>),
    Mcp(ResolvedMcpTool),
    Unknown { name: String },
}

struct PreparedTool<'a> {
    id: String,
    stable_name: String,
    input: Value,
    resolved: ResolvedTool<'a>,
    presentation: ToolPresentationInfo,
    domain: ToolDomain,
    output_policy: OutputPolicy,
    effects: Vec<ToolEffect>,
    // existing step/permission/state/task fields
}
```

Resolve native first via `ToolRouter::resolve`, MCP second via `MCPToolRouter::resolve_tool`, otherwise mark Unknown and resolve immediately with `unknown tool: {name}`. Hooks may alter `tool_use.name`; re-resolve after `PreToolUse` returns `Continue` so metadata always corresponds to the actual invoked name.

Compute risk, prompt, resources, summaries, task pre-snapshot, presentation, and output policy from the resolved object. MCP uses generic presentation and conservative dynamic policies.

- [ ] **Step 9: Migrate execution and post-processing**

Native execution calls the resolved handler and returns `ExecResult { content, status, effects }`. Apply `OutputPolicy` before marking success. MCP returns no effects. Post-hook failure suppresses effects. Use `TaskOperation` for task after-snapshot/title logic. Use `DetailPolicy` for success detail; failures still show full output. Apply successful effects in original tool order:

```rust
for effect in effects {
    match effect {
        ToolEffect::CompactHistory { focus } => {
            manual_compact = Some(focus.unwrap_or_default());
        }
    }
}
```

Delete native-name implementations of `recent_file_paths`, `tool_arg_full`, `tool_detail_content`, compact checks, and task checks.

- [ ] **Step 10: Migrate scheduling**

Delete the name-taking helpers `tool_resources(name, input, work_dir)` and `mcp_tool_resources(name)`. Pre-flight stores resolved `ToolResources`; wave scheduling remains unchanged. Preserve synthetic task scope and MCP same-server behavior.

- [ ] **Step 11: Run focused behavior tests**

```bash
cargo test -p tact permission -- --nocapture
cargo test -p tact agent::tool_schedule -- --nocapture
cargo test -p tact agent::tool_dispatch -- --nocapture
cargo test -p tact task::display -- --nocapture
cargo test -p tact compact -- --nocapture
```

Expected: all focused tests pass, including compact blocked/failed tests and exact allow-list behavior.

- [ ] **Step 12: Prove native semantic strings are absent in core consumers**

Run:

```bash
rg -n '"(read_file|write_file|edit_file|bash|spawn_subagent|compact|task_create|task_get|task_list|task_update|ask_user)"' \
  crates/tact/src/permission/mod.rs \
  crates/tact/src/agent/tool_schedule.rs \
  crates/tact/src/agent/tool_dispatch.rs \
  crates/tact/src/task/display.rs
```

Expected: no production-code matches. Test literals are allowed only in test modules; move source guard assertions to a dedicated test if manual output is ambiguous.

- [ ] **Step 13: Commit**

```bash
git add \
  crates/tact/src/permission/mod.rs crates/tact/src/mcp/mod.rs \
  crates/tact/src/agent/tool_schedule.rs crates/tact/src/agent/tool_dispatch.rs \
  crates/tact/src/task/display.rs crates/tact/src/task/mod.rs
# Verify no crates/tui, thinking-preview, or book/26_chapter_issue paths are staged.
git diff --cached --name-only
git commit -m "refactor(agent): dispatch tools through typed metadata"
```

---

### Task 6: Remove all native-name semantics from the TUI

**Files:**
- Modify: `crates/tui/src/widgets/tool_widget.rs`
- Modify: `crates/tui/src/widgets/state/app/agent.rs`
- Modify: `crates/tui/src/widgets/state/app/popups.rs`
- Modify: `crates/tui/src/render/cells/tool.rs` — update its deterministic `ToolRenderOutput` test fixture with `ToolPresentationInfo::generic("fixture")` and any new field required by the render output.
- Modify: TUI scene/mouse/render fixtures constructing step events/results
- Test: `crates/tui/src/widgets/tool_widget.rs`
- Test: `crates/tui/src/widgets/state/app/agent.rs`
- Test: `crates/tui/src/widgets/state/app/popups.rs`

**Interfaces:**
- Consumes: `ToolPresentationInfo` on StepStarted and StepResult.
- Produces: metadata-only card titles, gutters, detail layout, transcript retention, popup eligibility, and ask-user meta behavior.

- [ ] **Step 1: Write failing ToolWidget tests proving names no longer grant behavior**

Add fixtures:

```rust
fn presentation(kind: ToolVisualKind, label: &str) -> ToolPresentationInfo {
    ToolPresentationInfo {
        visual_kind: kind,
        display_name: label.to_string(),
        keep_full_live_output: false,
        detail: ToolDetailKind::Result,
        popup: ToolPopupKind::None,
        compact_result_to_meta: false,
    }
}
```

Add tests:

```rust
#[test]
fn command_visual_kind_controls_title_even_for_unrelated_name() {
    let (theme, msgs) = fixture();
    let widget = ToolWidget::new(&theme, &msgs)
        .with_tool("not_bash")
        .with_presentation(presentation(ToolVisualKind::Command, "$ Shell"))
        .with_arg_summary("echo hi");
    assert_eq!(widget.title_text(), "$ Shell  echo hi");
}

#[test]
fn spawn_subagent_name_without_popup_metadata_has_no_popup_privilege() {
    let info = ToolPresentationInfo::generic("spawn_subagent");
    assert_eq!(info.popup, ToolPopupKind::None);
}
```

- [ ] **Step 2: Run and verify RED**

```bash
cargo test -p tui widgets::tool_widget::tests::command_visual_kind_controls_title_even_for_unrelated_name -- --exact --nocapture
```

Expected: compile failure because `ToolWidget` has no presentation field/builder.

- [ ] **Step 3: Store presentation in widget/render output**

Add `presentation: ToolPresentationInfo` to `ToolWidget` and `ToolRenderOutput`, with generic default only for test/legacy construction. Add:

```rust
pub fn with_presentation(mut self, presentation: ToolPresentationInfo) -> Self {
    self.presentation = presentation;
    self
}
```

`from_step_result` copies `result.presentation`.

- [ ] **Step 4: Replace `ToolDisplayKind` and display-name mappings**

Delete `ToolDisplayKind`, `display_kind`, and native branches in `tool_display_name`. Replace every title, size, gutter, command detail, preview order, detail eligibility, and detail title decision with `self.presentation.visual_kind` and `self.presentation.display_name`.

For Generic only, the Agent must already provide the stable full name as `display_name`; TUI must not synthesize native labels.

- [ ] **Step 5: Replace ask-user and subagent branches**

Use `compact_result_to_meta` instead of `result.tool == "ask_user"`. Use `keep_full_live_output` for full buffers and 8-line preview. Use `popup == SubagentTranscript` for completed transcript transfer and `open_subagent_popup` eligibility. Preserve ToolMeta model/token transfer independently.

- [ ] **Step 6: Carry presentation through start, live rebuild, failure, and finish**

Update `App::on_step_started` signature to receive presentation. Every `ToolWidget::new` rebuild must reuse `active.output.presentation.clone()`. `StepFailed` uses the active presentation. `StepFinished` uses `result.presentation`.

- [ ] **Step 7: Update fixtures with exact presentations**

Tests expecting file/command/task/subagent/sleep behavior must use the corresponding `ToolVisualKind`, label, popup, full-output, and compact-result flags. Generic tests use `ToolPresentationInfo::generic`.

- [ ] **Step 8: Run TUI focused and full tests**

```bash
cargo test -p tui widgets::tool_widget -- --nocapture
cargo test -p tui widgets::state::app::agent -- --nocapture
cargo test -p tui widgets::state::app::popups -- --nocapture
cargo test -p tui
```

Expected: all current visuals and interactions pass.

- [ ] **Step 9: Prove TUI native semantic strings are absent**

```bash
rg -n 'tool_name ==|result\.tool ==|match tool|display_kind\(|"spawn_subagent"|"ask_user"|"read_file"|"write_file"|"edit_file"|"bash"' \
  crates/tui/src/widgets/tool_widget.rs \
  crates/tui/src/widgets/state/app/agent.rs \
  crates/tui/src/widgets/state/app/popups.rs
```

Expected: no production behavior matches; literals may remain only in tests as external names.

- [ ] **Step 10: Commit**

```bash
git add \
  crates/tui/src/widgets/tool_widget.rs \
  crates/tui/src/widgets/state/app/agent.rs \
  crates/tui/src/widgets/state/app/popups.rs \
  crates/tui/src/render/cells/tool.rs \
  crates/tui/src/handlers/mouse.rs crates/tui/src/render/layout.rs \
  crates/tui/src/render/log_render_tests.rs crates/tui/src/render/popup_scene_tests.rs \
  crates/tui/src/render/render_gap_tests.rs crates/tui/src/render/scene_tests.rs \
  crates/tact-ui/tests/mcp_tools.rs crates/tact-ui/tests/harness_advanced.rs \
  crates/tact-ui/tests/recovery_compaction.rs crates/tact-ui/tests/driver_integration.rs \
  crates/tact-ui/tests/subsystem_tools.rs crates/tact-ui/tests/tool_integration.rs \
  crates/tact-ui/tests/permission_integration.rs
# Verify the staged list contains no thinking-preview or Ch 26 paths before committing.
git diff --cached --name-only
git commit -m "refactor(tui): render tools from presentation metadata"
```

---

### Task 7: Compatibility, source guardrails, architecture docs, and final verification

**Files:**
- Modify: `ARCHITECTURE.md`
- Modify: `crates/tact/src/tool/registry.rs` tests
- Modify: `crates/tact/src/agent/tool_dispatch.rs` tests
- Modify: `crates/tui/src/widgets/tool_widget.rs` tests
- Do not modify: `book/26_chapter_issue*.md` unless implementation changes observable behavior beyond the approved compatibility contract.

**Interfaces:**
- Consumes: completed metadata architecture.
- Produces: API compatibility proof, hardcoding guardrail, documented architecture, clean full-workspace verification.

- [ ] **Step 1: Add exact ToolSpec snapshot-style assertions**

For representative tools (`read_file`, `bash`, `spawn_subagent`, `task_update`), assert exact name and description plus stable required schema properties. Compare against constants copied from the pre-refactor definitions; do not regenerate expected values from metadata in the test.

- [ ] **Step 2: Add a source guard test for forbidden native semantic matches**

In a test under `tool/registry.rs`, read the four core consumer files and TUI consumer files with `include_str!`, strip `#[cfg(test)]` sections only if needed, and assert forbidden patterns are absent from production portions:

```rust
const CONSUMERS: &[(&str, &str)] = &[
    ("permission", include_str!("../permission/mod.rs")),
    ("schedule", include_str!("../agent/tool_schedule.rs")),
    ("dispatch", include_str!("../agent/tool_dispatch.rs")),
    ("task display", include_str!("../task/display.rs")),
];
```

Prefer API/type boundaries over brittle global string bans. The guard should target semantic constructs such as `name == "builtin"`, `match name`, and `result.tool ==`, not legitimate logs, tests, or router lookups.

- [ ] **Step 3: Run compatibility and guard tests**

```bash
cargo test -p tact tool::registry -- --nocapture
cargo test -p tact-ui --tests
cargo test -p tui
```

Expected: exact external tool sets/specs pass; source guard passes; integration/TUI tests pass.

- [ ] **Step 4: Update `ARCHITECTURE.md`**

Document:

```text
LLM name -> ToolRouter::resolve -> RegisteredTool(handler + ToolMetadata)
          -> permission/resource/presentation/output policies
          -> ToolCallResult(content + typed effects)
```

State that names remain external stable IDs, native semantics are self-described, `always_allowed_tools` remains string-based, MCP is parsed at its adapter and conservatively defaults, and TUI receives only `ToolPresentationInfo`.

- [ ] **Step 5: Run the complete verification gate**

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo build --workspace
cargo test --workspace
git diff --check
```

Expected: all commands exit 0; no warnings; no failed tests.

- [ ] **Step 6: Review requirements line by line**

Verify and record in the commit/PR summary:

```text
[ ] external names/descriptions/schemas unchanged
[ ] always_allowed_tools exact names unchanged
[ ] all native handlers have metadata
[ ] permission/scheduler/dispatch/task/TUI have no native-name semantics
[ ] compact uses ToolEffect and failure suppresses effects
[ ] MCP same-server scheduling and conservative permission preserved
[ ] unknown tools fail closed
[ ] task titles and subagent/TUI behavior preserved
[ ] full workspace verification passes
```

- [ ] **Step 7: Commit docs and guardrails**

```bash
git add ARCHITECTURE.md crates/tact/src/tool/registry.rs crates/tact/src/agent/tool_dispatch.rs crates/tui/src/widgets/tool_widget.rs
git commit -m "docs: document self-described native tools"
```

- [ ] **Step 8: Request final code review**

Use `superpowers:requesting-code-review` with the approved spec and this plan. Address only verified findings, rerun the full verification gate, then use `superpowers:finishing-a-development-branch` to choose integration/push cleanup.
