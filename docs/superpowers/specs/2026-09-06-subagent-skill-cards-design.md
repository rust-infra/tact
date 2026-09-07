# 子代理技能卡（Subagent Skill Cards）设计

> 日期：2026-09-06 · 状态：草稿待批准（先写设计文档）
> 关联：`crates/tact/src/tool/subagent.rs`、`crates/tact/src/consts.rs`（`TactPath`）、`book/12_chapter_subagent*.md`、`book/26_chapter_issue*.md`
> 背景提交：`8c74f4e`（移除声明式 agent definitions，分支 `feat/plugin-compat`）

## 1. 背景与动机

`8c74f4e` 删除了声明式 agent-def（`.tact/agents/*.md` + 插件 `agents/*.md` + `spawn_subagent.agent`），
理由是插件兼容把它做复杂了：frontmatter 的 tools/model/permissionMode、`plugin:<name>` 命名空间与歧义消解、
安装期 `agent_count` 记账、两处插件列表 UI——复杂度与「复用 worker 身份」的价值不成比例。

删除后暴露的真实缺口：**子代理没有可复用的「角色/方法」注入**。现在所有子代理都是
固定通用 system prompt + 五件套，想给某个 worker 稳定的专业视角只能靠每次在 `prompt` 里手写。

本设计用**隔离的、极简的「技能卡」**补上这个缺口，并刻意规避被删功能的三类复杂度：

- 与主 agent 的 skill 系统**物理隔离**（不同目录、不进 `SkillRegistry`、不进主 agent 可见列表）；
- 只消费「正文 = 角色文本」，**不解析/不执行** tools / model / permissionMode 类 frontmatter；
- 无插件命名空间、无启动期注册表、无工具集过滤——spawn 时按需读一个文件。

## 2. 范围

### 在内（v1）

- 一个专用目录：`~/.tact/subagent/`（用户级，跨项目共享；与 memory/plugins 同级语义）。
- 该目录下扁平 `<name>.md` 文件 = 一张「技能卡」；可选 frontmatter `name` / `description`；正文即角色文本。
- `spawn_subagent` 新增可选输入 `skill: <name>`：把命中文件的**正文**拼进子代理 Static system prompt（hook 之前）。
- 未知名 → spawn 报错并列出可用技能卡（fail-closed，不静默降级）。

### 不在内（v1 明确不做，文档化为限制）

| 不做 | 原因 |
|---|---|
| tools / model / permissionMode frontmatter 生效 | 被删 agent-def 的核心复杂度，角色只需文本 |
| 项目级 `.tact/subagent/` 或其它额外根 | 与用户指定的单一 `~/.tact/subagent/` 冲突，留作后续扩展 |
| 插件提供的技能卡 / `plugin:<name>` | 子代理已与插件解耦（`8c74f4e` 决策延续） |
| 子目录递归 / 多语言文件名 | 扁平单文件最可预测 |
| 启动期注册表 / `/subagent-reload` | spawn 按需读盘即天然热加载，无需共享状态 |
| resume 的技能卡一致性持久化/校验 | v1 文档写明「后续轮重传同一 `skill`」 |
| 子代理运行时自己加载技能卡（load_skill 式） | 属另一特性（方案 B），不混入 |

## 3. 设计

### 3.1 目录与文件格式

```
~/.tact/subagent/reviewer.md
~/.tact/subagent/fixer.md
~/.tact/subagent/architect.md
```

`~/.tact/subagent/<name>.md`：

```markdown
---
name: reviewer
description: Adversarial code-review role for subagents
---

You are a principal reviewer. Assess diffs for correctness, test coverage,
and scope creep. Verdicts: Critical / Important / Minor.
```

- 目录不存在 → 视为 0 张卡（不报错）。
- frontmatter 解析失败 → fail-open：按栅栏切分，正文 = 闭合栅栏之后的内容（与 skill 的容错策略一致）；无栅栏则整文件当正文。
- **注册 key = 文件名 stem**；frontmatter 的 `name` 字段被忽略（避免 key 与文件名不一致导致查不到文件），`description` 用于未知名报错的列表展示。
- 只读**顶层** `*.md`；正文进 system prompt。

### 3.2 命名解析

- 注册 key = **文件名 stem**（`reviewer.md` → `reviewer`），与 frontmatter `name` 无关。
- `spawn_subagent { skill: "reviewer" }` 精确匹配该 key；不做模糊/后缀回退（无歧义来源，同 `load_skill` 语义）。

### 3.3 `spawn_subagent` 输入与装配时序

`SubagentInput` 增加：

```rust
/// Attach an isolated subagent skill card (~/.tact/subagent/<name>.md); its
/// body is appended to the child's system prompt.
#[schemars(description = "Name of a subagent skill card to attach as the role.")]
#[serde(default)]
pub skill: Option<String>,
```

spawn handler 内的装配顺序（`subagent.rs`）：

1. 构造通用 system prompt（现模板，不变）；
2. 若 `skill: Some(name)`：`~/.tact/subagent/<name>.md` → 读 + 拆 frontmatter → 正文追加到 system prompt；
   未知名 → `bail!` 并列出可用卡（格式对齐 `describe_available`）；
3. `SubagentStart` hooks 仍最后执行（可追加 / Block）。

system prompt 最终形态（wrapper 复用 `<skill>` 标签，模型已熟悉该呈现格式）：

```
You are a coding subagent at <work_dir>. Complete the given task, then summarize your findings.

<skill name="reviewer">
You are a principal reviewer. …
</skill>
```

### 3.4 实现位置（无新模块，无新共享状态）

- 全部逻辑收敛在 `crates/tact/src/tool/subagent.rs`：
  - `fn subagent_skill_dir() -> Option<PathBuf>`：`TactPath::home_tact_dir()?.join("subagent")`；
  - `fn parse_skill_card_frontmatter(text) -> (SkillCardFrontmatter, String)`：极简栅栏切分（fail-open）；
  - `fn read_skill_card(dir, name) -> Option<SkillCard>`（`SkillCard { description, body }`）：单文件读，key = 文件 stem；
    名字必须是纯文件 stem（无分隔符 / `.`/`..` / NUL），否则 `None`（目录逃逸即被拒绝，与 `safe_path` 同向的包含性约束）；
  - `fn format_skill_card_line(stem, desc)` / `fn list_skill_cards(dir) -> Vec<String>`：`- stem: <单行截断 description>`，
    用于未知名报错与目录注入；`list_skill_cards` 用 `fs::metadata(entry.path())`（跟随 symlink），与 `read_skill_card` 行为一致；
  - `fn apply_skill_card(prompt, dir, name) -> Result<()>`：命中拼 `<skill>` 块，未知名 fail-closed；
  - `fn annotate_spawn_subagent_skill_catalog(&mut ToolRouter)`：把可用清单追加进 spawn 工具 description（见 §3.7）。
- 目录经 `HOME` 解析。**发现清单注入**时无 `HOME` → no-op（无卡可列）；但 **spawn 时显式请求 `skill:`**
  而无 `HOME` → fail-closed 报错（与未知名同向，避免静默降级）。
- 不新增 `ToolContext` 字段、不触碰 `SkillRegistry` / 插件侧；主 agent 的 Dynamic system prompt 不动，
  唯一可见通道是 spawn 工具描述中的只读清单。

### 3.5 边界语义

- `prompt` 仍必填 = 任务；`skill` 可选 = 角色。二者不互相替代。
- 同步 / 异步 / worktree / `[agent.subagent]` 模型覆盖均不受影响（技能卡只改 system 文本）。
- `resume`：角色一致性靠「重传同一 `skill`」约定；v1 不持久化校验。
- 体积：技能卡应为精选小文档；超长由上下文预算自然约束，v1 不额外截断。

### 3.6 与主 agent 的隔离保证

- 目录不同（`~/.tact/subagent/` vs 各 skill 根），主 agent 的 skill 摘要 / `load_skill` / 斜杠命令都看不到它；
- 不进 `SkillRegistry` → 无 `/skill-reload` 联动问题，也无主 agent 误加载风险；
- 反向同理：主 agent 的 skills（含插件 skills/commands）不会进入子代理 system prompt。

### 3.7 主 agent 的发现通道：spawn 工具描述注入只读清单

隔离 ≠ 不可见。为了让主 agent 知道有哪些 `skill:` 名字可用，会话启动时（`interactive.rs` / `headless.rs`
构造 `toolset()` 之后）把技能卡目录扫成只读清单，**追加进 `spawn_subagent` 工具的 description**：

- 每行一条：`- <stem>: <description>`，description 折叠空白为单行并按 `SKILL_CARD_DESC_CAP`（60 字符）截断，
  控制每轮请求的重复 token 成本；渲染条数另按 `MAX_SKILL_CARD_CATALOG_LINES`（30）封顶，
  超出部分以 `… and N more` 收尾（单行截断只约束每行，不能约束总条数）；
- 无 `$HOME` / 目录不存在 / 0 张卡 → 不改 description（保持默认字符串）；
- **启动时快照**：会话内新增卡不生效（与工具集一致）；如需热加载属后续项；
- 这是与 fail-closed 的互补：模型**事前**看到合法名单，报错兜底仅在误用时出现。

## 4. 备选方案与取舍

| 方案 | 结论 |
|---|---|
| A. 复用主 agent `SkillRegistry`（`spawn_subagent.skill` 查共享注册表） | 放弃：会污染主 agent 可见列表；命名/语义与主 skill 纠缠；用户否决（要求隔离） |
| B. 子代理加 `load_skill` 运行时自加载 | 放弃：重，重蹈「子代理内部查表+上下文膨胀」覆辙 |
| C. 启动期注册表 + reload（类被删的 agent_def） | 放弃：多一份共享状态；本需求 spawn 按需读一个文件即可，天然热加载 |
| **D. 隔离目录 `~/.tact/subagent/` + spawn 按需读取（本设计）** | **采纳**：最少代码（~60 行）、无共享状态、无命名空间、与主 agent 物理隔离、热加载免费 |

## 5. 测试策略

- 单元（`tool::subagent` tests，**显式临时目录，不动 `HOME`**，避免并行竞态）：
  - 命中：正文以 `<skill>` 块出现在 system prompt 拼接结果中；通用模板首行保留；
  - 未知名：报错并列出可用卡；
  - frontmatter：key = 文件 stem；无栅栏整文件当正文；坏 YAML 栅栏后内容当正文、meta 回退默认；
  - `description` 缺省 = `"No description"`；
  - 隔离：`SkillRegistry`（主 agent 侧）看不到 `~/.tact/subagent` 内容；
  - 目录注入：`format_skill_card_line` 单行 + 60 字符截断补 `…`；`annotate_spawn_…` 追加可用清单 /
    空目录不改 description（3 个测试）。
- 编译：`cargo check --workspace --all-targets`；定向 `cargo test -p tact --lib tool::`。

## 6. 文档同步（双语）

- `book/12_chapter_subagent.md` / `_zh.md`：
  - §2 `SubagentInput` 结构体与字段表补 `skill` 行；
  - 新小节（复用原 §2.1 位置）「Subagent skill cards」：目录、frontmatter、正文即角色、与主 agent skill 的隔离说明；
  - §5 Static prompt 段补技能卡拼接示例。
- `book/26_chapter_issue*.md`：随实现推送前追加一条 `feat`/`optimization` 条目（含本 spec 链接）。
- 不涉及 `config.example.toml`、`agent_tui_kit`、插件文档。

## 7. 未来扩展（v1 不做，留档）

- 项目级 `<workdir>/.tact/subagent/` 作为本地覆盖根（需定义优先级：本地覆盖用户级，方向与 skill 注册表相反或一致待定）；
- 插件技能卡（若将来重新接入插件，仅加命名空间前缀，不动核心路径）；
- 技能卡 frontmatter 的 `allowed-tools` / `model` 执行（除非未来有强需求，否则维持纯文本角色）。
