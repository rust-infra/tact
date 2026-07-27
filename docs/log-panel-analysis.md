# Log 面板渲染流程分析

> 分析对象: `crates/tui/src/render/log.rs` — `render_log_panel()`

## 整体架构

整个 `render_log_panel` 分为 **4 个阶段 + 3 个覆盖层**：

```
┌──────────────────────────────────────────────────────┐
│  Phase 0: 可见索引                                    │
│  物理消息 → 逻辑消息（过滤不可见/流式缓冲）              │
│  visible_indices, phys_to_logical_cache               │
├──────────────────────────────────────────────────────┤
│  Phase 1: 换行缓存                                    │
│  逻辑消息 → 视觉行（wrap_line 按面板宽度折行）          │
│  visual_cache, visual_start_cache                     │
├──────────────────────────────────────────────────────┤
│  Phase 2: 视口裁剪                                    │
│  滚动偏移 + 可见高度 → 哪些视觉行可见 → 映射回逻辑索引   │
│  visual_scroll, end_visual, logical_start/end         │
├──────────────────────────────────────────────────────┤
│  Phase 3: 构建 cell → 渲染                             │
│  TextCell + LogColumnRenderer → ratatui Widget        │
│  + Block 边框 + Scrollbar                              │
├──────────────────────────────────────────────────────┤
│  Overlays: thinking / diff / code 卡片覆盖层           │
└──────────────────────────────────────────────────────┘
```

---

## Phase 0 — 可见索引（物理→逻辑）

```
messages (物理)            visible_indices (逻辑)
┌─────┬──────┐            ┌───┬───┬───┐
│  0  │ 可见  │ ────→ 0    │ 0 │ 1 │ 3 │   （消息 2 被过滤/隐藏）
│  1  │ 可见  │ ────→ 1    └───┴───┴───┘
│  2  │ 隐藏  │
│  3  │ 可见  │ ────→ 2
└─────┴──────┘
+ stream.buffer  → 额外一个逻辑行（流式输出中的文本）
```

- 用 `visible_indices_ver`（版本号 = `messages.len()`）做**脏检测**，只在消息数量变化时重建
- 同时维护反向映射 `phys_to_logical_cache[phys] → Option<logical>`
- 不可见的消息（thinking block 内部行）被跳过，不参与逻辑行计数

### 哪些消息会被隐藏？

**唯一隐藏场景：thinking block 内部的思考内容行。** 标题行始终可见，思考内容默认只显示最后 3 行。其余全部折叠隐藏，直到用户打开 thinking popup 查看完整内容。

核心里逻辑在 `widgets/state/app/visibility.rs`：

```rust
pub(crate) fn is_message_visible(&self, idx: usize) -> bool {
    for block in &self.thinking.blocks {
        if idx > block.title_idx && idx <= block.end_idx {
            let total = block.end_idx - block.title_idx;
            let visible_start = block.scroll_offset.min(total.saturating_sub(1));
            let visible_end = (block.scroll_offset + 3).min(total); // 窗口固定 3 行
            let relative = idx - (block.title_idx + 1);
            return relative >= visible_start && relative < visible_end;
        }
    }
    true  // 不在任何 thinking block 内 → 始终可见
}
```

#### 具体例子

假设 Agent 返回一个 thinking block，标题 1 行 + 内容 8 行：

```
messages[] (物理索引)
┌─────┬──────────────────────────────────────┐
│  4  │  "🧠 Thinking (8 lines)…"           │  ← title_idx = 4  (标题)
│  5  │  "│ Let me analyze the codebase…"    │  ← 思考内容行 1
│  6  │  "│ First, I need to understand…"    │  ← 思考内容行 2
│  7  │  "│ The architecture uses…"          │  ← 思考内容行 3
│  8  │  "│ Key components include…"         │  ← 思考内容行 4
│  9  │  "│ I should check the database…"    │  ← 思考内容行 5
│ 10  │  "│ Looking at the query plan…"      │  ← 思考内容行 6  ← scroll_offset = 5
│ 11  │  "│ The bottleneck is in JOIN…"      │  ← 思考内容行 7  ← 可见（3 行窗口）
│ 12  │  "│ Solution: use indexed view…"     │  ← 思考内容行 8  ← end_idx = 12
│ 13  │  ""                                 │  ← 隔离空行 (end_idx + 1)
│ 14  │  "Based on my analysis, I recommend…"│  ← 正常回复，始终可见
└─────┴──────────────────────────────────────┘
```

`ThinkingBlock` 结构：
- `title_idx = 4` — 标题行（`idx > title_idx` 才可能被隐藏，标题本身始终可见）
- `end_idx = 12` — 最后一行思考内容
- `total = 8` — 思考内容行数
- `scroll_offset = 5` — 窗口从第 5 行开始（默认 `total - 3`，显示最后 3 行）

**逐行判断：**

| idx | 含义 | `is_message_visible(idx)` | 原因 |
|-----|------|---------------------------|------|
| 4 | 标题行 | ✅ | `idx > 4` 为 false，不进入隐藏逻辑 |
| 5 | 内容行1 | ❌ | relative=0，`0 < scroll_offset(5)` → 窗口外 |
| 6 | 内容行2 | ❌ | relative=1，`1 < 5` |
| 7 | 内容行3 | ❌ | relative=2，`2 < 5` |
| 8 | 内容行4 | ❌ | relative=3，`3 < 5` |
| 9 | 内容行5 | ❌ | relative=4，`4 < 5` |
| 10 | 内容行6 | ✅ | relative=5，`5 ∈ [5, 8)` |
| 11 | 内容行7 | ✅ | relative=6，`6 ∈ [5, 8)` |
| 12 | 内容行8 | ✅ | relative=7，`7 ∈ [5, 8)` |
| 13 | 隔离空行 | ✅ | `idx > 12` 为 false，不在任何 block 范围内 |
| 14 | 正常回复 | ✅ | 不在任何 thinking block 内 |

**映射结果：**

```
messages (物理)           visible_indices (逻辑)     phys_to_logical_cache
┌─────┬─────────┐         ┌────┐                      [4]=Some(0)
│  4  │ 标题 ✅  │ ──→ 0   │ 4  │                      [5]=None
│  5  │ 隐藏 ❌  │         │ 10 │                      [6]=None
│  6  │ 隐藏 ❌  │         │ 11 │                      [7]=None
│  7  │ 隐藏 ❌  │         │ 12 │                      [8]=None
│  8  │ 隐藏 ❌  │         │ 13 │                      [9]=None
│  9  │ 隐藏 ❌  │         │ 14 │                      [10]=Some(1)
│ 10  │ 可见 ✅  │ ──→ 1   └────┘                      [11]=Some(2)
│ 11  │ 可见 ✅  │ ──→ 2                               [12]=Some(3)
│ 12  │ 可见 ✅  │ ──→ 3                               [13]=Some(4)
│ 13  │ 空行 ✅  │ ──→ 4                               [14]=Some(5)
│ 14  │ 正常 ✅  │ ──→ 5
└─────┴─────────┘
```

> `scroll_offset` 可通过交互调整，改变可见窗口（如设为 0 则显示前 3 行）。窗口大小始终固定为 3 行。

---

## Phase 1 — 换行缓存（逻辑→视觉）

每个逻辑消息调用 `wrap_line(&line, wrap_width)` 按当前面板宽度折行：

```
逻辑行 0: "hello world this is a very long message"
  → 视觉行: [0] "hello world "  [1] "this is a "  [2] "very long "  [3] "message"

visual_start_cache = [0, 4, 7, ...]   ← 前缀和，标记每个逻辑行从哪条视觉行开始
visual_cache = [视觉行0, 视觉行1, 视觉行2, 视觉行3, ...]
```

- `visual_cache_ver`（消息数量）+ `visual_cache_width`（面板宽度）**双重版本检测**——消息数或宽度变化才触发重建
- `wrap_line` 内部按 Unicode 显示宽度（`unicode-width` crate）切分，正确处理 CJK 字符
- stream buffer 用 `app.theme.accent` 颜色单独渲染

---

## Phase 2 — 视口裁剪

```
total_visual = 1200 行
visible_height = 20 行
offset = 15（逻辑滚动偏移）

visual_start_cache[15] = 180  ← 逻辑行 15 从视觉行 180 开始
visual_scroll = 180           ← 视口从视觉行 180 开始
end_visual = 200              ← 视口到视觉行 200

binary_search 反向映射：
  logical_start = binary_search(180) = 15
  logical_end   = binary_search(200) = 18
```

步骤：

1. **clamp 滚动偏移** — `offset` 不能超过 `max_scroll`（由 `visual_start_cache` 二分查找确定）
2. **视觉行→逻辑行** — 用 `binary_search` 把视口起止位置映射回逻辑索引范围
3. **底部对齐** — 若 `end_visual >= total_visual` 且内容超出视口，clamp 到 `max_visual_scroll`
4. 两次 `binary_search` 开销 O(log n)，视口内遍历 O(visible)

---

## Phase 3 — 构建 Cell → 渲染

### 3.1 构建 cell

```rust
for logical_i in logical_start..logical_end {
    let cell = TextCell::new(
        cached_lines,     // 换行缓存
        raw_text,         // 原始文本（选中范围用）
        selection_range,  // 字节级选中范围，None 表示未选中
        prefix,           // thinking 折叠提示
        indent_cols,      // 左侧 gutter 缩进列
        fg_color,         // 前景色
    );
    renderer.push(visual_start, cell);
}
```

### 3.2 TextCell 渲染策略

`TextCell::build_lines()` 按优先级选择：

| 优先级 | 状态 | 渲染方式 |
|--------|------|----------|
| 1 | 整行选中 | 复用 `cached_lines`，整行加 `REVERSED` |
| 2 | 部分选中 | `build_selected_lines()` — 仅选中字节范围 `REVERSED` |
| 3 | 默认 | 直接返回 `cached_lines` 克隆 |

`render_partial(area, buf, skip_lines)` 支持从第 N 行开始绘制，跳过视口外的行。

### 3.3 LogColumnRenderer

```rust
struct LogColumnRenderer<'a> {
    cells: Vec<(usize, Box<dyn Renderable + 'a>)>,  // (视觉起始行, 可渲染单元)
    viewport_top: usize,
    viewport_height: usize,
}

impl Widget for LogColumnRenderer<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        for (vis_start, cell) in &self.cells {
            // 跳过视口外的 cell
            // 计算 visible_start / visible_end
            // 调 cell.render_partial(cell_area, buf, skip_lines)
        }
    }
}
```

按视觉行偏移排列 cell，做二次视口裁剪后逐个绘制。`cells` 使用 `Box<dyn Renderable>` trait object，支持混合不同类型的可渲染单元。

### 3.4 渲染流程

1. 构建带边框的 `Block`，在其中渲染 `Clear` 清空区域
2. `LogColumnRenderer` 作为 Widget 绘制 TextCell
3. 三个覆盖层（thinking/diff/code cards）叠加绘制
4. 滚动条在右侧绘制（thumb 跟随视觉行位置，非逻辑偏移）
5. 渲染结束后将 `visual_start_cache` 克隆到 `visual_start`，供鼠标命中测试和外部滚动处理器使用

---

## 覆盖层

| 覆盖层 | 文件 | 功能 |
|--------|------|------|
| Thinking cards | `cells/thinking.rs` | 思考块折叠卡片（含预览行、耗时统计） |
| Diff cards | `cells/diff.rs` | 差异块卡片覆盖（含行号、文件路径） |
| Code cards | `cells/code.rs` | 代码块卡片覆盖（含语言标签、语法高亮） |

三个覆盖层都接收 `visual_scroll` / `visible_height` 做独立的视口裁剪。

---

## 滚动条

```rust
ScrollbarState::new(total_visual)
    .viewport_content_length(visible_height)
    .position(sb_position)
```

thumb 位置公式：

```
sb_position = visual_scroll × (total_visual - 1) / (total_visual - visible_height)
```

---

## 关键数据结构

### LogScroll (`widgets/state/log_scroll.rs`)

| 字段 | 类型 | 说明 |
|------|------|------|
| `offset` | `u16` | 逻辑行滚动偏移 |
| `state` | `ScrollbarState` | ratatui 滚动条状态 |
| `height` | `u16` | 面板可用高度 |
| `visible_indices` | `Vec<usize>` | 可见索引缓存：逻辑行 → 物理消息索引（由 render_log_panel 每帧重建） |
| `visual_start` | `Vec<usize>` | 视觉行起始索引（每次渲染后从 `visual_start_cache` 克隆，供鼠标命中测试/滚动处理使用） |
| `visual_cache` | `Vec<Line>` | 所有视觉行（已折行，不含选中样式） |
| `visual_start_cache` | `Vec<usize>` | 逻辑行 → 视觉行起始位置（前缀和） |
| `visual_cache_width` | `u16` | 缓存时的面板宽度 |
| `visual_cache_ver` | `usize` | 缓存版本号 = `messages.len()` |
| `visible_indices_ver` | `usize` | 可见索引版本号 |
| `phys_to_logical_cache` | `Vec<Option<usize>>` | 物理→逻辑反向映射 |

### TextCell (`cells/text.rs`)

| 字段 | 说明 |
|------|------|
| `cached_lines` | 预折行的视觉行（纯文本，无样式） |
| `raw_text` | 原始文本（用于选中范围重建） |
| `selection_range` | 选中字节范围 `(start, end)`，`None` 表示未选中 |
| `prefix` | 首行前缀（thinking 折叠提示） |
| `indent_cols` | 左侧 gutter 缩进列数 |
| `fg_color` | 前景色 |

### Renderable trait (`renderable.rs`)

```rust
trait Renderable {
    fn render(&self, area: Rect, buf: &mut Buffer);
    fn render_partial(&self, area: Rect, buf: &mut Buffer, skip_lines: usize) {
        self.render(area, buf); // 默认实现：忽略 skip_lines
    }
    fn height(&self, width: u16) -> u16;
}
```

---

## 设计要点总结

1. **版本号驱动的增量缓存** — `visual_cache_ver`、`visible_indices_ver` 用消息数量做版本号，只在变化时重建；`visual_cache_width` 额外检测宽度变化
2. **三维度索引映射** — 物理消息 → 逻辑消息 → 视觉行，用前缀和 + 二分查找高效转换
3. **渲染策略分离** — 缓存存原始文本，选中时现场重建样式行，避免缓存膨胀
4. **视口裁剪贯穿始终** — Phase 2 粗粒度过滤，`LogColumnRenderer` 二次过滤，cell 内部 `skip_lines` 精确裁剪
5. **Widget 组合模式** — `LogColumnRenderer` 实现 ratatui `Widget` trait，可直接 `frame.render_widget()`
