# Native Tool Metadata 去硬编码设计

> Date: 2026-07-27
> Status: approved — awaiting written-spec review
> Related: `crates/tact/src/tool/`, `crates/tact/src/agent/tool_dispatch.rs`, `crates/tact/src/agent/tool_schedule.rs`, `crates/tact/src/permission/mod.rs`, `crates/tui/src/widgets/`
> Plan: 用户审阅本 spec 后由 writing-plans 生成

## Goal

一次性移除所有 **native tool** 在权限、并发调度、输出处理、Agent 生命周期、task 领域展示和 TUI 展示中基于工具名字符串的业务语义分支。工具名仍是 LLM 协议、路由、持久化和用户配置中的稳定标识；内部行为改由工具本身声明的结构化 metadata、公开 presentation 信息和结构化 effect 决定。

本改造必须保持对外工具名称、description、JSON schema、历史 session 兼容性，以及 `always_allowed_tools` 的配置格式和语义不变。MCP 工具继续是动态外部工具，在 MCP adapter 边界采用结构化解析与保守 fallback。

## Problem

当前 native tool 名称同时承担以下角色：

1. LLM API 的 tool identifier；
2. `ToolRouter` 的 lookup key；
3. 权限风险分类依据；
4. 文件/共享状态资源调度依据；
5. 大输出持久化、详情和参数摘要策略依据；
6. `compact` 等 Agent 生命周期特殊行为的触发条件；
7. task 展示和执行前 snapshot 的领域分类；
8. subagent transcript、popup 和预览行数的 TUI 分类。

这些语义分散在 `tool_schedule.rs`、`permission/mod.rs`、`tool_dispatch.rs`、`task/display.rs` 和 TUI 中的多个 `match name` / `name == "..."` 分支。新增、改名或拆分 native tool 时，需要人工同步多处字符串匹配；遗漏会造成权限过宽、并发不安全、生命周期错误或 UI 退化。

## Decision summary

| 选择 | 决定 |
|------|------|
| Native tool 语义 | 由每个 tool 自身的 `ToolMetadata` 显式声明，作为内部唯一来源 |
| Router | 注册时绑定 handler 和 metadata；按名称 lookup 一次后传递解析结果 |
| Tool handler | 默认仍返回 `Result<String>`；宏包装为 `ToolCallResult`，仅需要 effect 的 tool 使用结构化结果 |
| Agent 内部副作用 | 使用封闭的 `ToolEffect` enum，替换按工具名触发的特殊后处理 |
| Task 语义 | 使用 `ToolDomain::Task(TaskOperation)`，不再将 task tool 名称传入展示/dispatch 逻辑 |
| TUI 语义 | Agent 将 metadata 映射为 protocol 的 `ToolPresentationInfo`；TUI 不导入 core metadata，也不匹配 native 名称 |
| LLM tool name/schema | 完全不变；名称仍是外部协议及 router lookup key |
| 永久授权配置 | `always_allowed_tools` 继续按原始工具名称存储和查询 |
| MCP | 在 MCP adapter 结构化解析 server/tool；按动态、保守策略执行 |
| 未知名称 | fail closed：不执行、不产生 effect、不提供 native UI 特权 |

---

## 1. Core types and ownership

### 1.1 `Tool` self-description

`Tool` 从只提供名称、说明和 handler，演进为自描述 handler：

```rust
pub trait Tool: Send + Sync {
    fn metadata(&self) -> &'static ToolMetadata;
    fn input_schema(&self) -> Value;

    async fn call(
        &self,
        context: ToolContext,
        input: Value,
    ) -> Result<ToolCallResult>;

    fn tool_spec(&self) -> ToolSpec {
        let metadata = self.metadata();
        ToolSpec {
            name: metadata.name.to_owned(),
            description: Some(metadata.description.to_owned()),
            input_schema: self.input_schema(),
        }
    }
}
```

`name()` 和 `description()` 不再是独立来源。对外的 tool name 与 description 均由 metadata 提供，确保 LLM spec 和内部身份一致。

`#[tool]` proc macro 按 handler 函数名生成同一模块的 metadata 常量引用，例如
`read_file` 对应 `READ_FILE_METADATA`，`cron_create` 对应
`CRON_CREATE_METADATA`。所有现有 `#[tool(name = ..., description = ...)]` 改为 bare
`#[tool]`；宏不再保存第二份 name/description。缺少约定常量将是编译错误，确保工具
实现无法绕过 metadata。

绝大多数 handler 保持现有业务签名：

```rust
pub async fn read_file(ctx: ToolContext, input: ReadFileInput) -> Result<String>;
```

`#[tool]` proc macro 生成 wrapper，在成功后构造 `ToolCallResult::text(output)`。只有需要通知 Agent 生命周期的工具（本次为 `compact`）需要生成或返回非空 `effects` 的结构化结果。

### 1.2 `ToolMetadata`

定义于 `crates/tact/src/tool/metadata.rs`，并由每一个 native tool 模块在本地声明其常量。例如：

```text
tool/read_file.rs  → READ_FILE_METADATA
tool/write_file.rs → WRITE_FILE_METADATA
tool/subagent.rs   → SPAWN_SUBAGENT_METADATA
tool/task.rs       → TASK_*_METADATA
```

核心形状：

```rust
pub struct ToolMetadata {
    pub name: &'static str,
    pub description: &'static str,
    pub permission: PermissionPolicy,
    pub resources: ResourcePolicy,
    pub domain: ToolDomain,
    pub presentation: ToolPresentation,
    pub output: OutputPolicy,
    pub argument_summary: ArgumentSummaryPolicy,
    pub permission_prompt: PermissionPromptPolicy,
}
```

这不是中心化 `match name` registry。定义、schema、handler 和业务语义在同一工具模块内，减少新增工具的多点维护。

### 1.3 `ToolRouter` registration and resolution

router 注册 native tool 时，从 tool 获取 metadata，以 `metadata.name` 作为唯一 lookup key，并将 handler 与 metadata 绑定：

```rust
pub struct RegisteredTool {
    handler: Box<dyn Tool>,
    metadata: &'static ToolMetadata,
}
```

注册必须拒绝空名称和重复名称。每个 native tool 必须提供完整 metadata；不存在 native tool 的隐式默认 metadata。新增 native tool 时，开发者必须显式决定权限、资源、展示、输出与领域分类。

运行时完成一次名字解析：

```text
LLM ToolUse { id, name, input }
  → ToolRouter::resolve(name)
  → RegisteredTool { handler, metadata }
  → 后续阶段消费 typed metadata，而非再次匹配 name
```

名称依然保留在 metadata 中，用于 LLM 协议、日志、持久化、用户可见标题和 allow-list 查询。

### 1.4 `ToolCallResult` and `ToolEffect`

```rust
pub struct ToolCallResult {
    pub content: String,
    pub effects: Vec<ToolEffect>,
}

pub enum ToolEffect {
    CompactHistory { focus: Option<String> },
}
```

普通工具返回 `ToolCallResult::text(...)`。`compact` 在成功时返回 `CompactHistory` effect，Agent post-processing 仅匹配该 enum，而不再匹配 `"compact"`。

effect 约束：

- 只由 native tool 返回；MCP 不产生 effect；
- 只在 handler 成功后应用；
- 被 pre-hook 阻止、权限拒绝、参数反序列化失败或 handler 失败时不应用；
- 多个 effect 按模型原始 tool-use 顺序应用；
- 不引入字符串型 effect、任意 callback 或由 output 文本推断 effect。

---

## 2. Metadata policies

### 2.1 Permission

```rust
pub enum PermissionPolicy {
    Read,
    Write,
    High,
    ShellCommand { command_field: &'static str },
}
```

- `Read`、`Write`、`High` 直接提供 capability risk；
- `ShellCommand` 使用通用命令分类器，维持当前只读/写入/高危 shell 命令的规则，但 `PermissionManager` 不再识别 `"bash"`；
- 原始稳定工具名仍用于 `always_allowed_tools` 查询和写入，因此现有配置（如 `always_allowed_tools = ["bash", "write_file"]`）不需迁移。

权限层接收已解析 native tool 的 metadata 与 input，而不是用 builtin 名称推导风险。

### 2.2 Permission prompt

权限确认文案同样是 native tool 语义，不能继续由
`format_permission_prompt(name, input)` 匹配 `ask_user`、shell 或文件工具名称。
metadata 额外声明：

```rust
pub enum PermissionPromptPolicy {
    Json,
    Question { field: &'static str },
    Command { field: &'static str },
    Path { field: &'static str },
}
```

- `ask_user` 使用 `Question { field: "question" }`，维持不显示 options JSON 的现有文案；
- `bash` 使用 `Command { field: "command" }`；
- read/write/edit 使用 `Path { field: "path" }`；
- 其余 native tool 使用 `Json`；
- MCP 与 unknown 使用通用 JSON 提示。

通用 formatter 接收稳定显示名称、`PermissionPromptPolicy` 和 input。它不得接收
native tool name 再做 builtin 匹配。

### 2.3 Resources and scheduling

```rust
pub enum ResourcePolicy {
    Independent,
    Barrier,
    ReadPath { field: &'static str },
    WritePath { field: &'static str },
    SharedState { scope: &'static str },
}
```

策略在通用 resolver 中将 input 转为 `ToolResources`：

- `read_file`：`ReadPath { field: "path" }`；
- `write_file`、`edit_file`：`WritePath { field: "path" }`；
- task 系列：`SharedState { scope: "__tact_tasks__" }`；
- `sleep`：`Independent`；
- shell、`apply_patch`、`spawn_subagent`、worktree/cron/其他不可精确界定副作用的 native tool：`Barrier`。

该 resolver 同时提供 metadata 声明路径的最近文件提取，消除调度与 `recent_file_paths` 对同一工具名称的重复解释。原有文件冲突判定、路径规范化和 wave 算法保持不变。

### 2.4 Domain

```rust
pub enum ToolDomain {
    Generic,
    Task(TaskOperation),
    Subagent,
}

pub enum TaskOperation {
    Create,
    Get,
    List,
    Update,
}
```

`ToolDomain::Task` 用于：

- 在 `Get` / `Update` 前，从 `task_id` 捕获执行前 snapshot；
- task 工具的标题、依赖和状态变更展示；
- task 专用的摘要宽度；
- task shared-state 资源策略的语义归属。

`task/display.rs` 改为接收 `TaskOperation`/`ToolDomain`，不再接收 arbitrary `&str` 并匹配 task 名称。

`ToolDomain::Subagent` 表达 subagent 的领域身份；它不替代 presentation policy，而是供需要领域级上下文的 Agent/TUI state 使用。

### 2.5 Presentation and output

内部 metadata：

```rust
pub struct ToolPresentation {
    pub live_output: LiveOutputPolicy,
    pub detail: DetailPolicy,
    pub popup: PopupPolicy,
}

pub enum LiveOutputPolicy {
    Standard,
    FullTranscript,
}

pub enum DetailPolicy {
    None,
    Result,
    InputField(&'static str),
}

pub enum PopupPolicy {
    None,
    SubagentTranscript,
}

pub enum OutputPolicy {
    PersistLargeOutput,
    KeepInline,
}
```

`ArgumentSummaryPolicy` 是输入标题/摘要的独立策略，避免让 presentation 混合输入格式化职责：

```rust
pub enum ArgumentSummaryPolicy {
    Json,
    Path { field: &'static str },
    Command { field: &'static str },
    SleepMilliseconds { field: &'static str },
    Patch,
    Question { field: &'static str },
    OptionalIdentifier { field: &'static str, empty: &'static str },
    Cron { cron_field: &'static str, prompt_field: &'static str },
    Memory { name_field: &'static str, type_fields: &'static [&'static str] },
    Focus { field: &'static str },
    SubagentPrompt { field: &'static str },
    NameAndRole { name_field: &'static str, role_field: &'static str },
    RecipientAndBody { recipient_field: &'static str, body_field: &'static str, fallback: &'static str },
    InboxOwner { field: &'static str },
    WorktreeRun { name_field: &'static str, command_field: &'static str },
    Limit { field: &'static str },
    Task,
}
```

每个 variant 封装当前 `tool_arg_full` 的既有格式（包括 patch 首行/dry-run、40 字符 body
预览、`check_background` 的 `all`、以及 worktree 名称组合），因此 Agent 不再识别
native 工具名称。`Task` 委托 `TaskOperation` 的领域 formatter。

`ResourcePolicy` 另设 `PatchFiles { patch_field: &'static str, dry_run_field: &'static str }`
variant，用于维持 `apply_patch` 成功后从 unified diff `+++` 行记录最近文件的现有行为；它
对调度仍是 barrier。

主要 native 行为：

| Tool category | Detail | Live output | Popup | Output |
|---|---|---|---|---|
| file read / shell | result | standard | none | read 保持 inline；shell 保持现有大输出策略 |
| `write_file` | input `content` | standard | none | 现有策略不变 |
| `edit_file` | input `new_text` | standard | none | 现有策略不变 |
| `spawn_subagent` | result | full transcript | subagent transcript | 现有策略不变 |
| ordinary mutation | none | standard | none | 现有策略不变 |

`read_file` 使用 `OutputPolicy::KeepInline`，替换 dispatch 中对其名称的特殊判断；其他工具沿用其当前的结果持久化行为。

---

## 3. Protocol and TUI boundary

TUI 不依赖 `tact` crate 内部 metadata。因此 Agent 把 presentation 映射为 protocol crate 中仅含纯数据的类型：

```rust
pub struct ToolPresentationInfo {
    pub visual_kind: ToolVisualKind,
    /// 已解析的 UI 标签，例如 "$ Bash"、"📖 Read" 或 "Cron"。
    pub display_name: String,
    pub keep_full_live_output: bool,
    pub detail: ToolDetailKind,
    pub popup: ToolPopupKind,
    pub compact_result_to_meta: bool,
}

pub enum ToolVisualKind {
    Generic,
    FileWrite,
    FileRead,
    FileEdit,
    Command,
    Task,
    Subagent,
    Sleep,
}

pub enum ToolDetailKind {
    None,
    Result,
    InputField(String),
}

pub enum ToolPopupKind {
    None,
    SubagentTranscript,
}
```

`ToolPresentationInfo` 必须表达当前 `ToolWidget` 由 native name 推导的全部行为：

- `visual_kind` 决定 file diff gutter、read/plain gutter、command `$` transcript 前缀、command output 尾部预览、detail card 标题、成功详情卡显示资格、sleep 标题格式和 task/subagent 标题规则；
- `display_name` 取代 `tool_display_name(name)` 的 native-name mapping。Generic/MCP/unknown 可使用其稳定的完整 protocol name；
- `keep_full_live_output` 决定完整 transcript 缓冲和 subagent 的 8 行而非标准 3 行 live preview；
- `detail` 决定 result、某个 input field 或不生成详情；
- `popup` 决定是否允许打开 subagent transcript popup；
- `compact_result_to_meta` 仅为 `ask_user` 设置，维持将成功回答压缩到 meta row、而非额外 detail card 的现有行为。

`StepStarted` / `StepFinished` 携带这份已解析的 presentation 数据，TUI state 和 `ToolRenderOutput` 将其保留。TUI 只能使用 presentation info 决定：

- 是否保存完整的 live transcript；
- detail 取 result、某个 input field 或不生成；
- 是否允许打开 subagent transcript popup；
- 预览截断、标题、gutter 和 command transcript 格式；
- 是否压缩 ask-user 结果到 meta row。

工具名仍可在日志、协议和 Generic card 标题中显示，但 TUI 不得以 native name 决定任何行为。近期加入的 `ToolMeta`（子 agent model/token）继续通过 `tool_id` 更新 tool card，并在 active/completed output 中保留，和 presentation 无冲突。

---

## 4. Dispatch data flow

```text
LLM ToolUse { id, name, input }
  → resolve to Native / MCP / Unknown
  → pre-flight: permission, resource policy, domain-specific snapshot, StepStarted(presentation)
  → schedule waves
  → execute handler with per-invocation ToolContext
  → ToolCallResult { content, effects }
  → output policy: inline or persist large output
  → presentation policy: detail and live-output state
  → post-processing: ordered ToolEffect application, StepFinished
  → LLM ToolResult
```

Native resolution produces a typed `ResolvedNativeTool` containing the registered handler and static metadata. Once resolved, Agent code must use metadata/domain/presentation/effects rather than re-match the original name.

`compact` exact timing remains: only a successful handler returns `CompactHistory`; its result still becomes the ordinary LLM `ToolResult`, and the Agent then records/applies manual compaction. Hook-blocked, permission-denied, malformed or failed compact invocations do not alter history.

---

## 5. MCP and unknown-tool boundary

### 5.1 MCP

MCP tools remain dynamic. At the MCP adapter boundary, the existing external protocol name is parsed once:

```text
mcp__{server}__{tool}
```

into a structure equivalent to:

```rust
struct ResolvedMcpTool {
    full_name: String,
    server: String,
    tool: String,
}
```

This name parsing is permitted because it adapts an external MCP naming protocol, not native tool business semantics.

MCP behavior is conservative:

| Dimension | MCP behavior |
|---|---|
| Permission | dynamic MCP policy; never gain native `Read` status merely by being unknown |
| Scheduling | same server serializes via synthetic scope; different servers may run concurrently |
| UI | standard tool card and result detail |
| Live output/popup | standard preview; no special popup |
| Effects | none |
| Allow-list | full stable MCP name remains usable |

Existing MCP read-prefix classification, if retained, is isolated to the MCP dynamic-policy boundary and does not classify native tools.

### 5.2 Unknown name

A name that is not registered native and not resolved by MCP fails closed:

- it is not executed;
- it returns an unknown-tool error;
- it produces no Agent effect;
- it receives no native presentation privilege;
- it is not written to the allow-list;
- it does not enter an execution wave.

This handles hallucinated model calls safely. Native tools cannot silently fall into such a fallback: static registration/tests require their metadata.

---

## 6. Migration map

| Existing location | Replacement |
|---|---|
| `tool_schedule::tool_resources(name, ...)` | `ResolvedNativeTool.metadata.resources.resolve(...)`; MCP adapter server scope |
| `permission::classify_risk(name, input)` | `metadata.permission.resolve(input)`; MCP dynamic boundary policy |
| `format_permission_prompt(name, input)` | stable display name + `metadata.permission_prompt`; generic MCP/unknown JSON prompt |
| `run_native_tool` special `read_file` branch | `metadata.output` |
| `recent_file_paths(name, input)` | `metadata.resources` path extraction |
| `tool_arg_full(name, input)` | `metadata.argument_summary` |
| success detail selection by name | `metadata.presentation.detail` |
| task pre-snapshot and display branches | `ToolDomain::Task(TaskOperation)` |
| compact name check | `ToolEffect::CompactHistory` |
| subagent transcript/popup name checks in TUI | protocol `ToolPresentationInfo.popup` / `keep_full_live_output` |
| `ToolWidget` display-kind, display-name, gutter, command-detail and ask-user name checks | protocol `ToolPresentationInfo.visual_kind`, `display_name`, `detail` and `compact_result_to_meta` |
| subagent preview length name check | `ToolPresentationInfo.keep_full_live_output` |

Native builtin strings may remain only in:

1. the declaring tool module's metadata and/or existing tool macro name argument;
2. router lookup code;
3. tests, docs, prompts and user-visible config;
4. the MCP external-name adapter.

They must not remain in Agent dispatch semantics, permission classification, scheduler classification, task display semantics or TUI behavior branches.

---

## 7. Error handling and invariants

### Invariants

1. Every registered native handler has complete, unique metadata.
2. A native tool's LLM spec name and router key are `metadata.name`.
3. Native semantics are determined only by metadata, domain and effect types after resolution.
4. `always_allowed_tools` uses the raw, stable protocol name.
5. Unknown names fail before execution and cannot grant capabilities or effects.
6. MCP cannot produce native effects or special native presentation.
7. An unrecognized/new native tool cannot become accidentally parallel or automatically read-authorized: it cannot register without explicit metadata.

### Failure behavior

- Bad input field referenced by a policy: resolver returns a conservative result appropriate to that stage; for resources it is a barrier, for UI it falls back to no detail, and handler deserialization remains the authoritative invocation error.
- Duplicate native name: router construction fails deterministically rather than silently replacing a handler.
- Metadata-to-protocol conversion: total mapping with safe generic presentation fallback for malformed/nonrepresentable internal data; valid static native metadata should make this unreachable in normal execution.
- Large-output persistence failure: preserve current execution failure/result semantics. A `ToolEffect` applies only after both the handler and any output persistence required by `OutputPolicy` succeed; therefore persistence failure suppresses effects. `compact` uses `KeepInline`, so its successful effect has no persistence dependency.

---

## 8. Test plan and acceptance criteria

### 8.1 Registration and API compatibility

1. Main `toolset()` and `subagent_toolset()` expose exactly the pre-refactor native tool name sets.
2. Every registered native tool has complete metadata and a unique name.
3. Generated `ToolSpec` name, description and JSON schema remain unchanged.
4. The subagent restricted tool set remains exactly bash/read/write/edit/sleep.

### 8.2 Permission

1. Native metadata produces Read for `read_file`, Write for write/edit, High for `spawn_subagent`.
2. Shell read/write/high-risk commands retain their current classifications through `ShellCommand` policy.
3. `Question`/`Command`/`Path`/`Json` permission prompt policies retain existing ask-user, shell, file and generic confirmation text without native-name matching.
4. Existing `always_allowed_tools = ["bash"]` continues to work without migration.
5. MCP retains conservative dynamic-policy behavior.
6. Unknown names cannot bypass authorization or execution.

### 8.3 Scheduler

1. Independent/read path/writable path/shared-state/barrier metadata produces the existing wave behavior.
2. Same-path read/write serializes; pure reads can overlap; disjoint writes retain existing behavior.
3. All task operations share the task synthetic scope.
4. Native barriers run alone.
5. Same MCP server serializes and distinct servers may overlap.

### 8.4 Dispatch, output and effects

1. `KeepInline` makes `read_file` skip large-output persistence without a name comparison.
2. Existing persist-large-output behavior remains for relevant other tools.
3. Result/input-field/no-detail presentation policies generate the existing details.
4. `CompactHistory` is applied only after successful compact execution.
5. Blocked, denied or failed compact calls do not compact history.
6. MCP and unknown results cannot emit effects.

### 8.5 Task and TUI

1. Task create/get/list/update titles, state and dependency display remain unchanged.
2. Task update/get pre-execution snapshots remain correct.
3. `ToolVisualKind` and `display_name` retain current native card titles, diff/plain gutters, command transcript prefix/output layout, sleep formatting and task/subagent formatting without TUI name matching.
4. Successful ask-user answers continue to compact into the meta row through `compact_result_to_meta`.
5. Running and completed subagent cards retain full transcript, model and token metadata.
6. Popup eligibility derives only from `ToolPopupKind::SubagentTranscript`.
7. A generic presentation with a similar-looking protocol name cannot acquire subagent transcript behavior.
8. Standard tools retain current preview/detail behavior.

### 8.6 Absence-of-hardcoding guardrails

The test/review suite must establish that these components no longer contain native builtin-name semantic matching:

- `permission/mod.rs`;
- `agent/tool_schedule.rs`;
- `agent/tool_dispatch.rs` for native output/detail/summary/effect decisions;
- task display APIs;
- TUI behavior decisions.

This guard is primarily structural API design: consumer functions accept `ToolMetadata`, `ToolDomain`, `ToolPresentationInfo` or `ToolEffect`, rather than arbitrary native tool names. Targeted source checks may supplement it, but behavior and type boundaries are the authoritative guarantee.

## 9. Documentation scope

This is an internal refactor. Native tool names, schemas, user configuration and observable behavior remain unchanged, so it does not require a Ch 26 issue-log entry or user-facing book updates by itself.

The design is recorded in this spec. The implementation plan will identify any necessary `ARCHITECTURE.md` update if its native tool-routing overview would otherwise become inaccurate. If implementation reveals an observable behavior change, the normal bilingual documentation and Ch 26 requirements apply in the same change.

## 10. Non-goals

- Renaming tools or changing JSON schemas;
- migrating `always_allowed_tools` to Rust enum names;
- making MCP tool semantics statically known;
- adding arbitrary plugin-defined native effects;
- redesigning scheduler conflict rules or permission modes;
- changing the visual style of tool cards, subagent popup contents, task titles or output retention behavior;
- introducing an attribute-macro metadata DSL. Metadata remains explicit Rust data near the tool handler.
