# 子代理技能卡（Subagent Skill Cards）实现计划

> 日期：2026-09-06 · 设计：`docs/superpowers/specs/2026-09-06-subagent-skill-cards-design.md`
> 分支：`feat/plugin-compat`（`8c74f4e` 之后）
> 目标：`spawn_subagent` 支持 `skill: <name>` —— 从 `~/.tact/subagent/<name>.md` 读取正文，
> 拼进子代理 Static system prompt，作为与主 agent skill 系统物理隔离的角色/方法注入。

## 执行顺序总览

1. T1：实现技能卡读取 helper + `SubagentInput.skill` + spawn 装配（单文件改动为主）。
2. T2：单元测试。
3. T3：`cargo check --workspace --all-targets` + 定向测试。
4. T4：双语文档同步（Ch 12）+ Ch 26 条目。
5. T5：最终核查（grep 残留 / diff review）。

---

## T1 核心实现（`crates/tact/src/tool/subagent.rs`）

- `SubagentInput` 增加字段（schema 描述见设计 §3.3）：

```rust
#[schemars(description = "Name of a subagent skill card to attach as the role.")]
#[serde(default)]
pub skill: Option<String>,
```

- 新增私有 helper（模块内，不新增模块/不新增 ToolContext 字段）：
  - `fn subagent_skill_dir() -> Option<PathBuf>`：`TactPath::home_tact_dir()?.join("subagent")`；
    无 `HOME` 返回 `None`（视为无技能卡，跳过 skill 分支）。
  - `fn parse_skill_card_frontmatter(text: &str) -> (SkillCardFrontmatter, String)`：极简栅栏切分；
    无栅栏 → 整文件当正文；有栅栏 → 取闭合栅栏之后正文；YAML 解析失败 fail-open（meta 默认）。
  - `fn read_skill_card(dir: &Path, name: &str) -> Option<SkillCard>`：目标 `<dir>/<name>.md`；
    读失败/不存在 → `None`；返回 `SkillCard { description, body }`；key = 文件名 stem（frontmatter `name` 忽略）。
    名字必须为纯文件 stem（`is_plain_stem`：无分隔符 / `.`/`..` / NUL），否则 `None`（目录逃逸即拒绝）。
  - `fn list_skill_cards(dir: &Path) -> Vec<String>`：顶层 `*.md` 的 `- stem: description` 行，排序；
    仅用于未知名报错；目录不存在返回空；跟随 symlink（`fs::metadata(entry.path())`），与读取行为一致。
  - `fn apply_skill_card(system_prompt: &mut String, dir: &Path, name: &str) -> Result<()>`：
    命中 → `system_prompt.push_str("<skill name=…>…body…</skill>")`；未知名 → `Err`（含可用列表）。
- spawn handler 装配点（在「构建通用 system prompt」与「SubagentStart hooks」之间）：
  - `if let Some(name) = input.skill.as_deref()`：
    - `let dir = subagent_skill_dir()`；无 `HOME` → `bail!`；
    - `apply_skill_card(&mut system_prompt, &dir, name)?`；
  - 维持现有顺序：通用模板 → 技能卡 → hooks。

- 工具长描述（`SPAWN_SUBAGENT_METADATA.description`）补一句
  `Set skill: <name> to attach an isolated subagent skill card from ~/.tact/subagent/<name>.md; its body is appended to the child system prompt as the role.`

- 不改：`registry.rs`（工具集仍五件）、插件侧、`SkillRegistry`、`ToolContext` 构造、config。

### T1b spawn 工具描述注入可用卡清单（主 agent 发现通道）

- `crates/tact/src/tool/mod.rs`：`ToolRouter` 增加 `description_overrides: HashMap<String, String>` + `set_tool_description(name, text)`；
  `tool_specs()` 返回副本时按 override 替换对应工具的 description（不动 `cached_specs` 缓存内容）。
- `crates/tact/src/tool/subagent.rs`：
  - `const SKILL_CARD_DESC_CAP: usize = 60`；
  - `const MAX_SKILL_CARD_CATALOG_LINES: usize = 30`（总条数封顶，超出以 `… and N more` 收尾）；
  - `format_skill_card_line(stem, desc)`：折叠空白为单行 + 按字符数截断到 60 补 `…`；`list_skill_cards` 复用；
  - `annotate_spawn_subagent_skill_catalog(&mut ToolRouter)`（pub，经 `tool/mod.rs` 再导出为 `tact::tool::…`）：
    无 `$HOME` 或 0 张卡 → no-op；否则把 `Available subagent skill cards:\n- …` 追加进 `spawn_subagent` 的 description。
- `crates/tact-ui/src/{interactive,headless}.rs`：`toolset()` 后调用 annotate（启动时快照，会话内不加新卡不生效）。
- 测试：`catalog_lines_are_flattened_and_capped`、`annotate_spawn_description_appends_available_cards`、
  `annotate_spawn_description_skips_when_no_cards`、`annotate_spawn_description_caps_catalog_size`、
  `skill_card_name_rejects_directory_escape`、`catalog_lists_symlinked_cards_like_read_resolves_them`（unix）。

## T2 单元测试（`tool::subagent::tests`）

纯 helper 测试（显式传入临时目录，**不动 `HOME` 环境变量**，避免并行测试竞态）：

1. `apply_skill_card_appends_body_after_base_prompt`：命中 `reviewer.md` → 通用模板开头保留、
   含 `<skill name="reviewer">` 与正文。
2. `apply_skill_card_unknown_name_lists_available`：目录放 `a.md`/`b.md`，请求 `missing` → 报错含可用卡列表。
3. `skill_card_key_is_stem_and_bad_frontmatter_fails_open`：有栅栏但 YAML 非法 → 正文 = 闭合栅栏之后；
   无栅栏 → 整文件当正文；key 始终 = stem。
4. `skill_card_description_defaults_when_absent`：无 frontmatter 时 `description = "No description"`。
5. `skill_cards_isolated_from_main_skill_registry`：`SkillRegistry` 只扫 skill 根，看不到 `~/.tact/subagent` 内容（隔离断言）。

> 注：hooks 在技能卡之后的顺序由 handler 代码结构保证（`apply_skill_card` 位于 hooks 循环之前），
> 不单独用拉起 LLM 的测试覆盖；skill 分支最终写进 `AgentSystemPrompt::Static`。

## T3 验证

- `cargo check --workspace --all-targets`（单进程）。
- `cargo test -p tact --lib tool::` 全绿。
- 手工冒烟（可选）：`~/.tact/subagent/reviewer.md` 建卡后跑一次真实 spawn（非必须，可省）。

## T4 文档同步（双语）

- `book/12_chapter_subagent.md` / `_zh.md`：
  - §2 结构体代码块 + 字段表：补 `skill` 行；
  - 原 §2.1 位置新增小节「Subagent skill cards」：目录 `~/.tact/subagent/`、frontmatter、
    正文即角色、spawn 拼装示例、与主 agent skill 隔离说明（两端结构对齐）；
  - §5 静态 prompt 段：补技能卡拼接示例代码块。
- `book/26_chapter_issue*.md`：各加一条 newest-first 条目（date 2026-09-06 · type `feat`），
  指针指向本 spec/plan、`subagent.rs`、Ch 12；注明动机为「8c74f4e 删除 agent-def 后补回隔离的角色注入」。

## T5 收尾核查

- re-grep：无 `skill` 相关残留、无主 agent skill 污染路径；
- `git diff` 复核：改动应集中在 `subagent.rs` + 两份 book（双语）+ Ch 26（双语）+ 本 spec/plan。
