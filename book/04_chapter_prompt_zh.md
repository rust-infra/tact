# Tact 中的系统提示词

> 语言：[中文](./04_chapter_prompt_zh.md) · [English](./04_chapter_prompt.md)

本章说明 Tact 如何构建**系统提示词**——在任意用户消息之前交给 LLM 的初始指令——以及其结构如何在每轮仍反映最新记忆与项目上下文的同时，保持对缓存友好。

---

## 1. 为何需要结构化系统提示词

系统提示词不是一条规则字符串。它由多个信息源组装而成：

| 节 | 说明 |
|----|------|
| **Role** | Agent 是谁（例如「You are a coding agent operating in /path/to/project」）。 |
| **Skills** | 当前可用能力。 |
| **Guidelines** | 完成任务的最佳实践（软约束，例如先理解目标再行动）。 |
| **Constraints** | Agent 必须遵守的硬性操作限制（例如用工具而非猜测、何时停止）。 |
| **CLAUDE.md** / **AGENTS.md** | 工作区可选项目指令文件（默认仅 `AGENTS.md`；见 `[agent].instruction_sources`）。 |
| **Memory** | 以往对话学到的持久事实。 |
| **Dynamic context** | 实时计算的项目快照（文件树、近期变更等）。 |

若全部揉进一段嘈杂段落，LLM 更难遵守重要规则。可预测的分节布局使行为在多种模型（Claude、OpenAI、Kimi、DeepSeek 等）间更一致。

---

## 2. 模板布局

模板位于：

```text
crates/tact/src/prompt/system_prompt_template.md
```

由 [Tera](https://keats.github.io/tera/) 根据 `crates/tact/src/prompt/mod.rs` 中组装的数据渲染。

最终输出顺序如下：

```markdown
# Your role
...

# Available skills
...

# Guidelines you need to follow
- ...

# Constraints that must be adhered to
- ...

# Memory guidance
...

# Additional context

<claude_md>    {# optional #}
<additional>   {# AGENTS.md #}

=== DYNAMIC_BOUNDARY ===

## Memory
...

## Dynamic context
...
```


| 节 | 来源 | 稳定性 |
|----|------|--------|
| `role` | 硬编码 agent 身份 | 静态 |
| `skills_available` | skill 注册表 | 基本静态 |
| `claude_md` | `CLAUDE.md`（可选；在 `# Additional context` 下） | 每会话静态 |
| `guidelines` / `constraints` | agent 默认值 | 静态 |
| `memory_guidance` | 常量提示文本 | 静态 |
| `additional` | `AGENTS.md`（默认；在 `# Additional context` 下） | 每会话静态 |
| `memory` | `MemoryManager` | 动态 |
| `dynamic_context` | 目录快照 / 近期文件 | 动态 |


`=== DYNAMIC_BOUNDARY ===` **之上**的节很少变化。**之下**的节（`memory`、`dynamic_context`）每轮可能变化。

---

## 3. 代码地图

### 3.1 模板与渲染

- `crates/tact/src/prompt/system_prompt_template.md` — Tera 模板。
- `crates/tact/src/prompt/mod.rs` — `SystemPrompt` builder、`Prompt` 包装与 `render()`。

`SystemPrompt` 为 builder：

```rust
let prompt = SystemPrompt::builder()
    .role("You are a coding agent operating in /home/xxxx/Projects/tact.")
    .skills_available("- rust-skills: Comprehensive Rust coding guidelines")
    .add_guideline("Think before you act")
    .add_constraint("Never expose secrets")
    .memory("User prefers Rust over Python")
    .dynamic_context("Current file tree ...")
    .build()?;

let rendered = prompt.to_prompt().render()?;
```

### 3.2 接入 Agent

实际内容在 `Agent::build_system_prompt`（`crates/tact/src/agent/mod.rs`）中组装：

```rust
let prompt = SystemPrompt::builder()
    .role(format!("You are a coding agent operating in {}.", workdir.display()))
    .guidelines([...])
    .constraints([...])
    .skills_available(self.tool_context.skill_registry.describe_available())
    .memory(self.load_memory_prompt()?)
    .claude_md(cached_md_section(&mut cached_claude_md, || assemble_claude_md_prompt(workdir, &instruction_sources)))
    .additional(cached_md_section(&mut cached_agents_md, || assemble_agents_md_prompt(workdir, &instruction_sources)))
    .dynamic_context(load_dynamic_context(workdir, &mut self.runtime.cached_dir_snapshot))
    .memory_guidance(MEMORY_GUIDANCE.trim())
    .build()?;
```

`build_system_prompt()` 在**每个任务**开始时调用一次，位于 `agent_loop` 顶部、回合循环开始之前。同一渲染字符串在该任务内每次 LLM 请求复用，使提示词在回合间字节稳定，利于前缀 KV 缓存。`memory` 与 `dynamic_context` 在下一任务开始时重新求值；启用的指令文件（`AGENTS.md` / `CLAUDE.md`）与目录快照**每会话组装一次**并缓存。

### 3.3 指令文件来源（`instruction_sources`）

默认仅注入 **`AGENTS.md`**。`CLAUDE.md` 通过 `tact.toml` 可选启用。二者渲染在同一 `# Additional context` 节（启用时 CLAUDE 块在前，随后 AGENTS）。

```toml
[agent]
# 默认 — 仅 AGENTS.md
instruction_sources = ["agents_md"]

# Claude Code 风格 — 两套文件
instruction_sources = ["agents_md", "claude_md"]

# 细粒度 CLAUDE 路径
instruction_sources = ["agents_md", "claude_md_user", "claude_md_project"]
```


| Key | 文件 |
|-----|------|
| `agents_md` | `<workdir>/AGENTS.md`、可选 `<cwd>/AGENTS.md` |
| `claude_md` | 下面三个 CLAUDE 路径全部 |
| `claude_md_user` | `~/.claude/CLAUDE.md` |
| `claude_md_project` | `<workdir>/CLAUDE.md` |
| `claude_md_subdir` | cwd ≠ workdir 时的 `<cwd>/CLAUDE.md` |


---

## 4. 静态 vs 动态模式

`AgentSystemPrompt` 有两种变体：


| 变体 | 行为 | 用例 |
|------|------|------|
| `Static` | 每次返回相同字符串 | 测试、演示或完全手动控制 |
| `Dynamic` | 每个任务开始时重新渲染模板 | 正常运行；保持上下文与记忆新鲜 |


正常 Tact 用法（`tact-ui` / headless）中，agent 以 `Dynamic` 模式启动。

---

## 5. 动态边界与 KV 缓存

LLM 提供商可在多次调用间**缓存**长提示词的前缀。系统提示词顶部的稳定节（role、guidelines、constraints、CLAUDE.md）非常适合缓存。

这一行：

```text
=== DYNAMIC_BOUNDARY ===
```

是系统提示词内的视觉标记。其**之上**的一切应在任务间保持相同，以便提供商复用缓存前缀。其**之下**（`memory`、`dynamic_context`）允许变化。

因 `build_system_prompt()` 在每个任务开始时运行，仅边界之后的后缀需要重新求值。若提供商支持提示词缓存，可在不支付整段指令块全价的情况下获得新鲜上下文。

---

## 6. 自定义系统提示词

### 6.1 添加 guidelines 或 constraints

在 `Agent::build_system_prompt` 中使用 builder 方法：

```rust
.constraints([
    "Always run cargo check after editing Rust files",
    "Prefer edit_file over write_file for small changes",
])
```

### 6.2 注入额外 markdown（`AGENTS.md`）

`Agent::build_system_prompt` 将会话**首次**渲染时的项目 `AGENTS.md`（cwd ≠ workdir 时还有 cwd 相对副本）载入模板 `additional` 槽位，后续回合复用缓存字符串。

也可通过 builder 追加任意 markdown：

```rust
.additional("# Project conventions\n\nUse anyhow::Result everywhere.")
```

### 6.3 替换整个模板

构造 `SystemPrompt` 时传入自定义模板字符串：

```rust
SystemPrompt::from(include_str!("my_template.md"))
    .role("...")
    .build()?
```

若希望受益于前缀缓存，请保留 `=== DYNAMIC_BOUNDARY ===` 约定。

---

## 7. 输出示例

真实会话快照（默认 `instruction_sources = ["agents_md"]`，存在项目 `AGENTS.md`，发现五个 skills，若干 `[feedback]` 记忆）：

- **role** — 在 `/Users/rg/Projects/tact` 的 coding agent
- **skills_available** — 五个 skill 摘要 + 斜杠 / `load_skill` 说明
- **guidelines** / **constraints** — tact 内置默认
- **memory_guidance** — 何时调用 `save_memory`
- **additional** — 项目 `AGENTS.md`（渲染在 `# Additional context` 下）
- **memory** — 持久化 `.tact/memory/*.md` 内容
- **dynamic_context** — 日期、workdir、模型、平台、目录快照

渲染后的提示词类似：

```markdown
# Your role

You are a coding agent operating in /Users/rg/Projects/tact.

# Available skills

- code-reviewer: Code review specialist
- demo-test: Test skill loading
- echo-args: Test skill for slash arguments — echoes $ARGUMENTS back and replies briefly
- english-tutor: English tutoring assistant
- shell-master: Shell command specialist

When a user message already contains a `<skill name="…">…</skill>` block, the user slash-invoked that skill — follow those instructions directly and do not call `load_skill` for the same skill. If the block includes an `ARGUMENTS:` line (Claude Code convention when the skill has no `$ARGUMENTS` placeholder), that line is the user's slash-command arguments for this invocation; apply the skill to fulfill them.

# Guidelines you need to follow

- Try to understand how to complete the task well before completing it.

# Constraints that must be adhered to

- Think step by step
- Think before you act; respond with your thoughts before calling tools
- Do not make up any assumptions, use tools to get the information you need
- Use the provided tools to interact with the system and accomplish the task
- If you are stuck, or otherwise cannot complete the task, respond with your thoughts and stop
- If the task is completed, or otherwise cannot continue, like requiring user feedback, stop.
- When editing files, always re-read the file first if its content may have changed since you last read it
- For multi-line changes, prefer apply_patch; for exact string replacements, use edit_file (replace_all=true to change every occurrence in the file)
- If a tool result was compacted and you need the details, re-run the relevant tool (e.g., read_file)
- For small edits to existing files, prefer edit_file over write_file; use write_file only for new files or complete rewrites

# Memory guidance

When to save memories:
- User states a preference ("I like tabs", "always use pytest") -> type: user
- User corrects you ("don't do X", "that was wrong because...") -> type: feedback
- You learn a project fact that is not easy to infer from current code alone
  (for example: a rule exists because of compliance, or a legacy module must
  stay untouched for business reasons) -> type: project
- You learn where an external resource lives (ticket board, dashboard, docs URL)
  -> type: reference

When NOT to save:
- Anything easily derivable from code (function signatures, file structure, directory layout)
- Temporary task state (current branch, open PR numbers, current TODOs)
- Secrets or credentials (API keys, passwords)

# Additional context

## AGENTS.md instructions

### From project root (AGENTS.md)

# Tact agent notes

- Run tests with `cargo test -p tact` / `cargo test -p tui` for focused crates.
- Prefer `edit_file` for small changes; use `apply_patch` for multi-line hunks.
- Book chapters live under `book/`; keep `04_chapter_prompt.md` aligned with `system_prompt_template.md`.
- User-facing docs and examples are English unless the task asks otherwise.

=== DYNAMIC_BOUNDARY ===

## Memory

# Memories (persistent across sessions)

## [feedback]
### batch_edit_min_files: batch_edit should only be used when editing 3+ distinct files
batch_edit should only be used when the edits span 3 or more distinct files. For edits touching fewer than 3 different files, prefer individual edit_file calls instead. This avoids the overhead of batch validation when simple single-file edits suffice.

This rule is also documented in `docs/agent_guidelines.md`.

### edit_file_lazy_diff_feedback: edit_file diff preview should be opt-in and lazy-loaded
User feedback: `edit_file` should support a `show_diff` (or similar) parameter so the user can opt in to a diff preview card. The card should lazy-load: show a clickable placeholder first; on click, run `git diff` and open/expand the result, instead of running diff automatically on every edit.

### edit_file_lazy_diff_feedback_v2: edit_file diff preview uses new_text, lazy-loads git diff on click
User feedback: `edit_file` should support `show_diff` (or similar) for optional diff preview. Suggested UX: preview the card with `new_text` (already in args, no extra cost); on click, open/expand and run `git diff` for the full diff, avoiding automatic diff on every edit.

### log_selection_should_be_character_level: Log text selection should be character-level, not line-only
Log panel selection is currently line-based: `log_selection` stores `(start_line, end_line)`, highlights whole lines, and copies full lines. That prevents selecting part of a line (e.g. mid-line to mid-line on another row) and feels awkward when drag-selecting while scrolling. User wants character/column-level selection.

### tool_card_double_click_detail_area_only: Tool card double-click popup should be limited to the detail card area
Tool Card double-click to open a popup should only apply inside the detail card (bordered preview: title, preview body, bottom hint). Clicks on the header status/title rows (the two header lines) should not open the popup.

## Dynamic context

Current date: 2026-07-15
Working directory: /Users/rg/Projects/tact
Model: deepseek-v4-pro
Platform: macos

## Project structure

  book/
  crates/
  docs/
  scripts/
  skills/
book
  output/
  prompts/
  scripts/
  templates/
book/output
  chm/
  mcp/
book/output/chm
  html/
book/scripts
  chm/
  lib/
crates
  protocol/
  tact-ui/
  tact/
  tact_llm/
  tool_refactor_macros/
  tui/
crates/protocol
  src/
crates/tact
  src/
crates/tact-ui
  src/
  tests/
crates/tact-ui/tests
  harness/
crates/tact/src
  agent/
  config/
  cron/
  hook/
  lsp/
  mcp/
  memory/
  notifications/
  permission/
  prompt/
  skill/
  store/
  task/
  tool/
  worktree/
crates/tact_llm
  src/
crates/tool_refactor_macros
  src/
crates/tui
  src/
crates/tui/src
  handlers/
  render/
  widgets/
docs
  superpowers/
docs/superpowers
  plans/
  specs/
skills
  demo-test/
```

随后经 `CreateMessageParams::with_system(&system)` 附加到 LLM 请求，再接对话历史。

---

## 8. 测试

prompt 模块单元测试覆盖：

- 填充时各节均渲染，
- 空节被省略，
- 静态节出现在 `=== DYNAMIC_BOUNDARY ===` 之前，
- 动态上下文不输出 XML 标签。

运行：

```bash
cargo test -p tact prompt
```

---

## 相关文件

- 模板：`[crates/tact/src/prompt/system_prompt_template.md](../crates/tact/src/prompt/system_prompt_template.md)`
- Builder / 渲染逻辑：`[crates/tact/src/prompt/mod.rs](../crates/tact/src/prompt/mod.rs)`
- Agent 接线：`[crates/tact/src/agent/mod.rs](../crates/tact/src/agent/mod.rs)`（`Agent::build_system_prompt`）
- Memory manager：`[crates/tact/src/memory/mod.rs](../crates/tact/src/memory/mod.rs)` — 见 [持久化记忆](./03_chapter_memory_zh.md)
- Skill 注册表：`[crates/tact/src/skill/mod.rs](../crates/tact/src/skill/mod.rs)` — 见 [Skill 注册表](./02_chapter_skill_zh.md)
- 动态上下文加载：`[crates/tact/src/agent/mod.rs](../crates/tact/src/agent/mod.rs)`（`fn load_dynamic_context`）
