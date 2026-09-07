# Subagent Sticky Tab（重新实现）实现计划

> 日期：2026-09-07 · 设计：`docs/superpowers/specs/2026-09-07-subagent-sticky-tab-design.md`
> 目标：在当前组件化架构上，像 Tasks 一样为子代理加 Log 下方 sticky tab（双域主机 Tasks | Subagent），
> 由 `SubagentsChanged` 全量快照驱动，不回滚 98a133f。

## 执行顺序总览

1. T1：协议类型 + `SubagentManager` 进程内 known 集合 + 发射 helper 与各发射点。
2. T2：agent_tui_kit 状态/组件/纯渲染（SubagentPanel + 双域 host）。
3. T3：tui app 层（registry、construct、render_ctx、sticky host 包装、mouse/normal 交互）。
4. T4：测试（protocol / tact / kit / tui）。
5. T5：`cargo check --workspace --all-targets` + 定向测试（单进程）。
6. T6：双语文档同步 + Ch 26 条目。
7. T7：收尾核查（grep / diff review）。

---

## T1 协议与 tact 发射

### `crates/protocol/src/agent.rs`

- 新增（放 `TaskStatusSnapshot` 附近，镜像其风格）：

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SubagentStatusSnapshot {
    #[default] Running, Completed, Failed, Cancelled,
}
impl SubagentStatusSnapshot { pub fn marker(self) -> &'static str; } // "▶" "✓" "✗" "⏹"

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SubagentRunSnapshot {
    pub child_id: String,
    pub status: SubagentStatusSnapshot,
    pub summary_first: String,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
}
```

- `AgentUpdate` 增：

```rust
/// Subagent runs for the sticky 总览 (本会话进程启动过的运行).
SubagentsChanged { runs: Vec<SubagentRunSnapshot> },
```

- `dispatch_components` / `shell_handle` 的穷尽匹配处（`agent.rs`）同步处理新变体。
- `AgentUpdate` 现有 test 补 destructure 测试。

### `crates/tact/src/subagent.rs`

- `SubagentManager` 增 `known: Mutex<HashSet<String>>`（`SubagentManager::new` 初始化空集合）。
- 方法：
  - `pub fn note_started(&self, child_id: &str)`；
  - `pub async fn ui_snapshot(&self) -> Vec<SubagentRunSnapshot>`：遍历 known →
    `records.get()` → 映射；Running 优先，其余按 started_at 倒序；
    `MAX_SUBAGENT_SNAPSHOT = 20`（Running 全保留）。
- 常量 `pub const MAX_SUBAGENT_SNAPSHOT: usize = 20;`
- helper：

```rust
pub async fn emit_subagents_changed(
    ui_tx: &Option<tokio::sync::mpsc::UnboundedSender<tact_protocol::AgentUpdate>>,
    manager: &SubagentManager,
)
```

- `SharedSubagentManager` 转发 `note_started` / `ui_snapshot`。

### `crates/tact/src/tool/subagent.rs`（spawn / cancel）

- `spawn_subagent`：在 `ctx.subagent_manager.start(child_id.clone()).await?` 后
  `ctx.subagent_manager.note_started(&child_id)`，随后
  `crate::subagent::emit_subagents_changed(&ctx.ui_tx, &manager_or_shared).await?`（sync 路径 ctx 仍借用
  ui_tx；async detached task 里用克隆的 `ui_tx` / `manager` 在落库后、`SubagentFinished` 前发射）。
- `cancel_subagent` 工具：`cancel()` 后发射。

### `crates/tact-ui/src/driver.rs`

- `UserCommand::CancelSubagent` 分支：`cancel()` 后
  `crate::subagent::emit_subagents_changed(&ui_tx, &subagent_manager).await`。

> 注意 borrow：`spawn_subagent` 中 `manager` 是 `SharedSubagentManager`（Arc 内层），helper 可接受
> `&SubagentManager`（经 `.inner`）或直接接受 `&SharedSubagentManager`——实现时取后者更省事，
> helper 签名按实际调用统一。

## T2 agent_tui_kit

### `state/subagent_panel.rs`（新）

镜像 `state/task_panel.rs`：

- `SubagentPanelState { snapshot, session_seen, visible, expanded, scroll, max_visible(默认 10) }`
- `apply_snapshot(&mut self, runs)`：可见性规则（见 spec §3.4）：
  - `session_seen = true`；
  - 首次（`!session_seen` 之前，即进入时 was_seen=false）：`visible = !runs.is_empty()`；
    `expanded = visible`；
  - 之后：`visible = has_running(&runs) || expanded`；全完成且 `!expanded` → `visible=false`。
- `has_running`、`format_subagent_lines(runs, scroll, max_visible)`、标题行函数。
- 行格式 / 分组：Running → Completed → Failed → Cancelled；`{marker} {short} {summary_first}
  [⏱ dur]`；截断 `⋯ +N more · scroll ▼`。short = child_id 前 8 字符。
- duration 复用现有 `format_duration`（started/finished 毫秒）。

### `components/subagent_panel.rs`（新）

镜像 `components/task_panel.rs`：`on_update` 只认 `SubagentsChanged`；`priority()=40`；
`render()=0`；Deref 到 state。`components/mod.rs` 导出，`Cargo` 无需改（同 crate）。

### `state/mod.rs`（kit）

- 追加 `pub mod subagent_panel;` + `pub use subagent_panel::SubagentPanelState;`
  （检查现有 `state/mod.rs` 导出风格后按同式添加）。
- `i18n.rs`：`Messages` 增 `subagents_sticky_title`（EN `"Subagent"` / 中文 `"子代理"`），两语言表补值。

### `render/sticky_host.rs`（新，纯渲染）

- `pub fn sticky_host_visible(ctx: &RenderCtx) -> bool` = task 或 subagent 可见。
- `pub fn sticky_host_content_height(ctx) -> usize`：折叠时 1；展开 = 标题(1)+hairline(1)+
  **活动 tab** body 行数；活动 tab 判定从 `ctx.mouse`（见下）。
- `pub fn render_sticky_host(frame, area, ctx)`：边框/背景规则完全沿用现有
  `render_task_panel`（LEFT|RIGHT|BOTTOM + bg）；标题行按可见域渲染 tab 标签；
  tab 命中矩形经 `RenderCommand::SetStickyTabAreas`（或扩展已有 SetCancelButtonArea 通道）回写
  `MouseState`；正文渲染活动 tab 域（Tasks 走现有 task_panel 行函数，Subagent 走新行函数）。

> 取舍：为最小化 diff，保留 `render/task_panel.rs` 现有入口作薄转发；真正新渲染函数放
> `render/sticky_host.rs`，`tui` 侧 app 包装函数（现 `render/task_panel.rs`）改调 host。

### `render/ctx.rs`

- `RenderCtx` 增 `pub subagent_panel: &'a SubagentPanelState`、`pub active_sticky_tab: StickyTab`、
  `pub sticky_tab_areas: &'a [(StickyTab, Rect)]` 之类（若走 RenderCommand 则在 mouse 字段内）。
- `StickyTab` 枚举放 `state/ui_types.rs`（kit）或 `state/mouse_state.rs`——实现时选类型位置最顺处
  （倾向 `ui_types.rs`，`MouseState` 引用它）。

## T3 tui app 层

- `widgets/state/mod.rs`：`pub(crate) mod subagent_panel { pub(crate) use agent_tui_kit::state::subagent_panel::*; }`。
- `widgets/state/app/registry.rs`：`SubagentPanelComponent` 访问器 `subagent_panel()` / `_mut()`。
- `widgets/state/app/construct.rs`：push `SubagentPanelComponent`；`MouseState` 初始 active tab。
- `widgets/state/app/agent.rs`：`shell_handle` / `dispatch_components` 穷尽匹配补
  `SubagentsChanged`（组件注册表处理即可，shell 无需额外副作用——除非要保持任务 DAG 那样的联动，
  v1 无）。
- `widgets/state/app/config.rs` `render_ctx()`：填 `subagent_panel` / active tab / tab areas。
- `render/task_panel.rs`（app 包装）：改判 host（`sticky_host_visible` / `sticky_host_content_height`
  / `render_sticky_host`），保留 `task_panel` 文件名以减少 import 噪音。
- `render/layout.rs`：`render_main_area` 改用 host 的 visible/height 函数（其它不变）。
- `handlers/mouse.rs`：`panel_hit` 增加 sticky 命中细分；点击逻辑区分 tab 区（切 active tab / 展开）
  与概要区（折叠活动域）；滚动按 active tab 域滚动对应 `scroll`。
- `handlers/normal.rs`：`j/k` 滚动改按 active tab 域。
- `render/log.rs` 每帧 `sticky_tab_areas` 刷新（仿 `subagent_cancel_btn_areas`）或由 host render
  通过 RenderCommand 写回——以现有每帧刷新模式为准。

## T4 测试

见 spec §5。要点清单：

- protocol：marker / destructure。
- tact `subagent.rs` tests：`note_started` → `ui_snapshot` 含该 child；旧 DB Running 行（orphan）不进
  snapshot；排序（Running 在前）；上限 20。
- kit `components/subagent_panel.rs` + `state/subagent_panel.rs`：apply / 忽略其它 update /
  可见性矩阵 / 行格式化分组 / 截断 / 标题计数。
- tui render tests：单 Tasks 无回归、单 Subagent、双 tab 切换 + hairline、bg 不变量测试
  （仿 `expanded_tasks_sticky_puts_a_rule_between_tabs_and_body`）。
- mouse/normal handler tests：点击 tab 切换 / 折叠；滚动走 active tab 域。

## T5 验证

- `cargo check --workspace --all-targets`（单进程）。
- 串行定向测试（每次一个 cargo 命令）：
  `cargo test -p tact-proto`（或 protocol crate 名，以 Cargo.toml 为准）、
  `cargo test -p tact --lib subagent::`、`cargo test -p agent_tui_kit`、`cargo test -p tact-ui`（相关）。
- 手工冒烟（可选）：造两个后台子代理观察 sticky 出现/展开/收起/隐藏。

## T6 文档同步（双语）

- `book/23_chapter_tui.md` / `_zh.md`：sticky 相关小节与代码图表格。
- `book/12_chapter_subagent.md` / `_zh.md`：§6 或 §11 增一行 sticky 总览指针。
- `book/26_chapter_issue*.md`：newest-first `feat` 条目（date 2026-09-07，链接 spec/plan、
  `subagent.rs`、Ch 23/12；动机为后台 fan-out 总览缺失；明确不推翻 98a133f）。

## T7 收尾

- re-grep：无 `AgentUpdate::Subagent {` 旧包裹残留；`SubagentsChanged` 在 protocol/kit/tui 三处穷尽。
- `git diff` 复核：改动集中于 protocol、`subagent.rs`、`driver.rs`、kit state/component/render、
  tui app/handlers/render + 双语 docs + spec/plan。
