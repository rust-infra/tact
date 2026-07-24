# 工程问题与优化日志

> Language: [English](./26_chapter_issue.md) · [中文](./26_chapter_issue_zh.md)

本章是一份**按时间倒序的优化与 bugfix日志**，记录有用户可见或 API 可见行为变化的改动。它不是教程：每条写清问题、决策与代码 / 设计文档位置，避免后续重复踩坑。

相关流程：`AGENTS.md`（何时追加条目）、`docs/superpowers/specs/`（设计）、`docs/superpowers/plans/`（实现计划）。

---

## 0. 目的

| 目标 | 说明 |
|------|------|
| 连续性 | 记录*为什么*改，而不只是*改了哪些文件* |
| 交叉引用 | 指向设计 spec、PR，以及讲解子系统的 book 章节 |
| 控制膨胀 | 每个已交付的行为变更一条；纯重构、仅测试改动不记 |

### 条目模板

最新条目在前。每条应包含：

1. **日期 / ID** — `YYYY-MM-DD` 与可选 PR 号  
2. **类型** — `optimization` · `bugfix` · `removal` · `docs`  
3. **现象 / 动机** — 改前错在哪里或代价是什么  
4. **决策** — 最终契约（不必展开全部否决方案）  
5. **改后行为** — agent / 用户可依赖的可观察规则  
6. **指针** — 代码路径、spec、相关 book 章节  

---

## 1. 2026-07-24 — 额外 `skill_dirs` + 项目本地 `.tact/skills`

| 字段 | 值 |
|------|-----|
| **类型** | optimization |
| **Spec** | `docs/superpowers/specs/2026-07-24-extra-skill-dirs-design.md` |

**现象 / 动机：** 原先只有固定 skill 根；无法挂共享 / vendor 目录。旧的
`<workdir>/skills/` 也落在 `.tact/` 之外。

**决策：** `<workdir>/skills/` 改为 `<workdir>/.tact/skills/`。新增可选
`[agent].skill_dirs = [...]`（相对 workdir；`~` 展开）。加载顺序：
`.tact/skills` → `~/.tact/skills` → `~/.agents/skills` → `.claude/skills` →
配置额外目录 → 插件 cache。缺失目录软跳过。

**改后行为：** 配置可追加 skill 根并覆盖同名独立 skill。不再扫描裸
`<workdir>/skills/`。

**指针：** `crates/tact/src/consts.rs`、`crates/tact/src/skill/mod.rs`、
`crates/tact/src/config/types.rs`、`tact.example.toml`、第 2 章。

---

## 2. 2026-07-24 — `/skills` 列表改用 tui-markdown（不用 pipe 表）

| 字段 | 值 |
|------|-----|
| **类型** | bugfix |

**现象 / 动机：** `/skills` 经 `format_table` 画 Skill/Description 表。长
frontmatter 描述使行宽超过 log 面板，视觉换行把 `|` 列拆碎，难以阅读。

**决策：** 保留标题块与空行分隔。输出易换行的 markdown（`**\`name\`**` + 描述
段落），经 `render_markdown_tui` / tui-markdown 渲染。此处**不用** GFM 表（与
Session Stats 不同）：目录描述对 log 固定列宽来说太宽。

**改后行为：** `/skills` 每个 skill 一块名称 + 描述；任意面板宽度下自然折行。
命名空间名（`plugin:skill`）不变。

**指针：** `crates/tui/src/handlers/mod.rs`（`show_skills_command`、
`skills_list_markdown`）。

---

## 3. 2026-07-24 — Session Stats 用 GFM 表格 + tui-markdown 渲染

| 字段 | 值 |
|------|-----|
| **类型** | bugfix |
| **Spec** | `docs/superpowers/specs/2026-07-24-session-stats-table-design.md` |

**现象 / 动机：** `/stats` 把 comfy-table UTF8 框线文本丢进 `render_markdown_tui`。
软换行变空格，整张表挤成一行再 wrap，弹窗里乱成一团。

**决策：** 保持 `SessionStats::summary() -> String`。输出 **GFM pipe 表格**
（数值列右对齐）。TUI 继续走 `render_markdown_tui` /
[tui-markdown](https://github.com/joshka/tui-markdown) 的表格渲染（Unicode 框线）。
移除 `comfy-table` 依赖。CLI / headless 打印同一份 markdown 源。

**改后行为：** Session Statistics 弹窗显示对齐框线表；退出摘要为 GFM markdown。
计数与显隐规则不变。

**指针：** `crates/tact/src/stats.rs`、
`crates/tui/src/widgets/state/app/agent.rs`、`docs/token_usage_schema.md`。

---

## 4. 2026-07-24 — Session Stats 用 comfy-table 排版

| 字段 | 值 |
|------|-----|
| **类型** | optimization |
| **Spec** | `docs/superpowers/specs/2026-07-24-session-stats-table-design.md` |
| **Plan** | `docs/superpowers/plans/2026-07-24-session-stats-table.md` |
| **被取代** | §3（GFM + tui-markdown） |

**现象 / 动机：** 会话结束时的 Tool calls 行靠空格对齐，工具名与耗时变长后列错位。

**决策：** 保持 `SessionStats::summary() -> String`。先输出 Metric/Value 表，再按需输出 Tool calls 表（`Tool | Count(s/f) | Total | Avg`），最后用尾部 Metric/Value 表放工具汇总 / cache / reasoning。*（最初用 `comfy-table` UTF8 框线；与 TUI markdown 冲突，见 §3。）*

**改后行为：** 计数与显隐规则不变；排版改为对齐表格。

**指针：** `crates/tact/src/stats.rs`、`docs/token_usage_schema.md`（Session Stats Display）。

---

## 5. 2026-07-24 — `/model` 从 `/v1/models` 补充配置

| 字段 | 值 |
|------|-----|
| **类型** | optimization |
| **Spec** | `docs/superpowers/specs/2026-07-24-openai-models-api-design.md` |
| **Plan** | `docs/superpowers/plans/2026-07-24-openai-models-api.md` |

**现象 / 动机：** `/model` 需要手写维护 `models = [...]` 列表；而 providers 已经提供了 `GET /v1/models`。

**决策：** Config 保持优先；API 附加不在 config 中的 id；冲突时 config 保持；每个 `(base_url, api_key)` 在首次 `/model` 时仅获取一次；跳过 Anthropic；失败时降级为仅用 config 或空提示。

**改后行为：** 见第 21 章 `/model` 节。

**指针：** `crates/tact_llm/src/models.rs`、`crates/tui/src/handlers/select.rs`、第 21 章、第 22 章（账户类查询）。

---

## 6. 2026-07-24 — `read_file` 分页与删除 `batch_read`

| 字段 | 值 |
|------|-----|
| **类型** | optimization + removal |
| **PR** | [#50](https://github.com/rust-infra/tact/pull/50) |
| **Spec** | `docs/superpowers/specs/2026-07-24-read-file-pagination-design.md` |
| **Plan** | `docs/superpowers/plans/2026-07-24-read-file-pagination.md` |

### 6.1 现象

`read_file` 用 `read_to_string` 整文件读入，再以 `chars().take(50000)` **静默**丢掉尾部。这与按行的 `offset` / `limit` 语义冲突，模型没有续读信号（幻觉风险见 [第 20 章](./20_chapter_hallucination_zh.md)），并与 dispatch 层的 `persist_large_output`（30k 字符 → `<persisted-output>`）形成双重、不一致的大小策略。

`batch_read` 是第二套多文件 API，另有 200k 字符硬顶，并在调度 / recent-file 上重复特例。

### 6.2 决策

1. 删除 `batch_read`。多文件并行读取改为同一 wave 内多个 `read_file`。  
2. 用 Tokio `BufReader` 按行流式读取（不为整页缓冲整文件）。  
3. 在 `read_file.rs` 用带前缀的常量封顶：

```rust
const READ_FILE_MAX_OUTPUT_TOKENS: usize = 25_000;
const READ_FILE_DEFAULT_MAX_LINES: usize = 2_000;
```

Token 估算：现有 `approx_token_count`（`ceil(UTF-8 字节数 / 4)`）。  
4. 不限制单行字符数（单行本身超预算则报错，绝不静默砍半行）。  
5. **未显式**指定范围 / 走默认页且未读完时，返回带引导的标记：

```text
[PARTIAL view — lines {start}-{end}; continue with offset={next}]

{joined lines}
```

6. **显式**传了 `offset` 和/或 `limit` 仍超 token 预算 → **报错**（不静默返回少于请求的范围）。  
7. `run_native_tool` 在 `name == "read_file"` 时 **跳过** `persist_large_output`。  
8. 工具 `description` 保持简短——限制在运行时强制，不在 schema 文案里重复。

### 6.3 改后行为

| 场景 | 结果 |
|------|------|
| 小文件、无参数 | 全文，无 PARTIAL |
| 超过 2000 行、无参数 | 前 2000 行 + PARTIAL（`offset=2001`） |
| 隐式读取触达 token 预算 | 已装下的完整行 + PARTIAL 与下一 `offset` |
| 显式范围超 token 预算 | `Err`，提示缩小 `limit` / 区间 |
| 单行本身超预算 | `Err`（无法靠行 offset 恢复行内后缀） |
| offset 越过 EOF | 空字符串 |
| 大 `read_file` vs bash / MCP | `read_file` 不会包 `<persisted-output>`；其它工具仍可能 |

### 6.4 指针

| 区域 | 路径 |
|------|------|
| 实现 | `crates/tact/src/tool/read_file.rs` |
| persist 豁免 | `crates/tact/src/agent/tool_dispatch.rs`（`run_native_tool`） |
| 工具注册 | `crates/tact/src/tool/registry.rs`（无 `BatchReadTool`） |
| 近似 token | `crates/tact/src/utils/truncate.rs` |
| 工具章 | [第 7 章](./07_chapter_tool_zh.md) |
| 压缩 / spill | [第 5 章](./05_chapter_compact_zh.md)、`docs/compaction.md` |

---

## Related Docs

- [工具系统](./07_chapter_tool_zh.md)
- [上下文压缩](./05_chapter_compact_zh.md)
- [Agent 循环中的幻觉](./20_chapter_hallucination_zh.md)
- [AGENTS.md](../AGENTS.md) — 含本章的文档同步触发条件
