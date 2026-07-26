# Ink 主题与弹窗 Chrome 设计

> Date: 2026-07-27  
> Status: approved  
> Related: `crates/tui/src/theme.rs`, `crates/tui/src/render/popups/`, `book/23_chapter_tui.md`, `book/21_chapter_config.md`  
> Plan: 用户审阅本 spec 后由 writing-plans 生成

## Goal

新增内置主题对 `ink` / `ink-light`，像素级贴近 Cursor Release Notes 深色弹窗气质（近黑底、青绿次强调、浅蓝分区标题、细直角边框），并将**全部** TUI overlay 收口到共用弹窗 chrome。`ink` 成为配置未指定或 `auto` 失败时的新默认；现有 10 套主题保留可切换。

## Problem

1. 当前默认 `retro` 与用户期望的低干扰深色 RN 风格差距大。  
2. 弹窗外壳分散在各 `popups/*.rs` / Widget 中，标题色、footer、边框类型不统一；`HelpWidget` 等仍有硬编码色。  
3. 现有 `Theme` 字段无法同时表达「浅蓝分区标题」与「青绿版本号」两种强调色。

## Decision summary

| 选择 | 决定 |
|------|------|
| 范围 | 新主题对 + 全 overlay chrome；**不做**真正的 Release Notes/changelog 功能 |
| 实现路线 | 扩展 `Theme` 语义色字段 + `popups/mod.rs` 共用 chrome |
| 命名 | `ink` / `ink-light` |
| 默认 | 未配置 / `auto` 失败 → `ink`（显式 `retro` 不受影响） |
| 浅色 | 成对提供 `ink-light`，层次对称 |
| 还原度 | 像素级贴近；必要时扩展字段 |
| 旧主题 | 新字段用派生默认填充；仅消除硬编码色 / 统一 `block_border_type` 的极小差异 |

Out of scope：独立 design-tokens 层、changelog 弹窗功能、主题持久化到 config（Ctrl+T 仍仅会话内）、agent/store/协议改动。

---

## 1. 目标与架构

```text
config / --theme / "auto"
  → resolve_theme → ThemeName::{Ink, InkLight, …}
  → Theme::from（含 heading / version / muted）
  → App.theme
  → bar / log / input / popups 读 Theme
  → popups/mod.rs 共用 chrome（Clear + 边框 + 标题行 + 可选 footer）
```

不改变渲染管线形状；只加主题与外壳一致性。

---

## 2. 色板字段与默认值

### 2.1 `Theme` 新增字段

| 字段 | 用途 | 旧主题默认 |
|------|------|------------|
| `heading` | Markdown / 弹窗分区标题（浅蓝） | `accent` |
| `version` | 版本号、次强调（青绿） | 深色主题用 `success`；浅色主题用 `accent` |
| `muted` | footer 提示、次要文字 | 固化现有 `muted_fg()` 计算结果 |

`muted_fg()` 改为返回 `self.muted`，避免双来源。

### 2.2 `ink`（深色，新默认）目标 RGB

| 字段 | 约值 | 说明 |
|------|------|------|
| `bg` | `(13,13,13)` | 近黑 |
| `fg` | `(232,232,232)` | 浅灰白 |
| `accent` | `(125,211,252)` | 浅蓝主强调 |
| `heading` | `(125,211,252)` | 分区标题 |
| `version` | `(45,212,191)` | 青绿次强调 |
| `border` | `(58,58,58)` | 细克制边框 |
| `highlight` | `(40,48,64)` | 低对比选中底 |
| `muted` | `(128,128,128)` | 次要文字 |
| `status_bar_bg` | `(18,18,18)` | |
| `bottom_bar_bg` | `(20,20,20)` | |
| `bottom_bar_fg` | `(160,160,168)` | |
| `input_box_bg` / `input_box_fg` | `bg` / `fg` | 与背景一体 |
| `warning` / `error` / `success` | 黄 / 红 / 绿 | 语义色；截图未出现，保持可读 |

### 2.3 `ink-light`

近白底、深灰字；`heading` 深蓝、`version` 深青；边框浅灰；与深色同一套角色映射。

### 2.4 边框类型

- `Ink` / `InkLight` → `BorderType::Plain`  
- 其余：现状不变（`Brutal` = Plain，其它 = Rounded）

### 2.5 命名与解析

- `ThemeName::Ink` / `InkLight`  
- 字符串：`ink`、`ink-light`（兼接受 `ink_light` / `inklight`）  
- 纳入 `ThemeName::all()` 与 `next()`  
- 配置默认与 `auto` 失败回退：`retro` → `ink`

---

## 3. 弹窗 chrome 统一

### 3.1 共用 API

在 `crates/tui/src/render/popups/mod.rs` 新增：

```rust
/// 一条 footer 提示：键位 + 说明，例如 ("↑/↓", "scroll")、("Esc", "back")。
pub(crate) struct FooterHint { pub key: &'static str, pub label: &'static str }

/// RN 风格弹窗外壳：Clear + 边框 + 标题行（左标题、右 [x]）+ 可选 footer。
/// 返回内容区 Rect。色彩全部取自 Theme。
pub(crate) fn render_popup_chrome(
    frame: &mut Frame,
    popup_area: Rect,
    theme: &Theme,
    title: &str,
    footer: Option<&[FooterHint]>,
) -> Rect
```

样式规则：

- 边框：`theme.border` + `theme.block_border_type()`  
- 标题：`theme.fg` 加粗；右上角 `[x]` 用 `theme.muted`（装饰性；关闭仍走 Esc / 既有 handler，不新增鼠标点 `[x]` 行为）  
- footer：居中；键位 `theme.accent`、说明 `theme.muted`，条目之间 `|` 分隔  
- 背景：`theme.bg`（Clear 后填充）  
- 无 footer 时内容区可多占一行；有 footer 时内容区底部预留 1 行

现有 `render_list_popup_chrome` 内部改为委托新 chrome（保留列表几何辅助函数）。

### 3.2 迁移范围（全部 overlay）

| 弹窗 | 改动 |
|------|------|
| thinking / diff / code / system-prompt(stats) / subagent / task-DAG | 外壳换 `render_popup_chrome`；内容渲染不动 |
| command palette / file picker | 经 `render_list_popup_chrome` 收口 |
| select / slash | 外壳换新 chrome；选中高亮仍用 `theme.highlight` |
| help | 硬编码标题色改读 `theme.heading`；外壳换新 chrome |
| history | `PopupWidget` 跟随 `block_border_type()` |

旧主题观感：仅「硬编码 → Theme」与边框类型统一的极小差异。`popup_scene_tests` 等快照随新 chrome 更新。

### 3.3 Markdown

`render_md.rs` 标题色从 `accent` 改为 `theme.heading`。旧主题 `heading == accent`，无观感变化；`ink` 下分区标题为浅蓝。

---

## 4. 默认切换、配置、文档与测试

### 4.1 默认行为

- 未配置 `ui.theme` / 未传 `--theme` → 默认字符串 `"ink"`  
- `theme = "auto"`：检测成功仍为 `Dark`/`Light`；失败回退 `Ink`  
- 显式 `theme = "retro"` 不受影响  
- Ctrl+T / palette `theme`：循环含 `Ink`/`InkLight`，仍不写回配置

### 4.2 代码入口

- `crates/tact/src/config/resolve.rs` 默认值与断言  
- `config.example.toml`  
- `theme_detection.rs` 失败回退  
- `construct.rs` / `headless_loop.rs` / 测试脚手架：仅「无配置工厂默认」改 `ink`；刻意测 retro 的用例保留

### 4.3 文档（同变更）

- `book/21_chapter_config*.md`：默认与支持列表  
- `book/23_chapter_tui*.md`：主题表 + 弹窗 chrome  
- `docs/tui_rendering.md`：主题数量对齐  
- `book/26_chapter_issue*.md`：optimization 条目（默认主题与弹窗观感）

### 4.4 测试

- 单元：`ThemeName` 解析、`Theme::from` 字段、`block_border_type`（ink=Plain）、`muted_fg` 读字段  
- 渲染：popup/help 相关测试按新 chrome 更新；至少一组使用 `ink`  
- 配置：`resolve` 默认主题断言改为 `ink`  
- 不新增 E2E 浏览器测试

### 4.5 验收标准

1. 全新安装无配置启动 → `ink` 配色与 Plain 边框弹窗  
2. `theme = "retro"` → 与改前基本一致  
3. 所有 overlay 在 `ink` 下标题 / `[x]` / footer 样式一致  
4. Markdown 分区标题在 `ink` 下为 `heading` 色  

---

## 5. 建议落地顺序

1. `ThemeName` + 字段 + `ink`/`ink-light` 色板 + 解析/默认/检测回退  
2. `render_popup_chrome` + list chrome 委托  
3. 逐个 overlay / Widget 迁移；消除 help 硬编码  
4. `render_md` 改用 `heading`  
5. 配置示例、book、Ch26、渲染文档  
6. 测试与快照更新  

---

## 6. 风险与非目标

| 风险 | 缓解 |
|------|------|
| 默认主题变更影响老用户 | 文档与 Ch26 明确说明；显式 `retro` 仍可用 |
| `Theme` 字段变多导致构造点漏填 | 旧主题统一派生默认；编译期强制 match 全覆盖 |
| 弹窗迁移遗漏导致风格分裂 | 清单覆盖全部 overlay；popup 场景测试兜底 |

非目标重申：不做 Release Notes 内容功能；不做 tokens 层重构。
