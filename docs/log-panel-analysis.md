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
- 不可见的消息（如隐藏的 thinking block 内部行）被跳过，不参与逻辑行计数

---

## Phase 1 — 换行缓存（逻辑→视觉）

每个逻辑消息调用 `wrap_line(&line, wrap_width)` 按当前面板宽度折行：

```
逻辑行 0: "hello world this is a very long message"
  → 视觉行: [0] "hello world "  [1] "this is a "  [2] "very long "  [3] "message"

visual_start_cache = [0, 4, 7, ...]   ← 前缀和，标记每个逻辑行从哪条视觉行开始
visual_cache = [视觉行0, 视觉行1, 视觉行2, 视觉行3, ...]
```

- `visual_cache_ver` + `visual_cache_width` **双重版本检测**——宽或高任一变化才触发重建
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
        raw_text,         // 原始文本（搜索用）
        search_term,      // 搜索词
        is_match,         // 是否命中搜索
        is_selected,      // 是否鼠标选中
        word_sel,         // 词级选择范围
        prefix,           // thinking 折叠提示
        fg_color,         // 前景色
    );
    renderer.push(visual_start, cell);
}
```

### 3.2 TextCell 渲染策略

`TextCell::build_lines()` 按优先级选择：

| 优先级 | 状态 | 渲染方式 |
|--------|------|----------|
| 1 | 搜索匹配 | `build_highlighted_line()` — 匹配词黄底黑字 |
| 2 | 整行选中 | `build_line_selected_lines()` — 全行 REVERSED |
| 3 | 词级选中 | `build_word_selected_lines()` — 仅选中词 REVERSED |
| 4 | 默认 | 直接返回 `cached_lines` 克隆 |

`render_partial(area, buf, skip_lines)` 支持从第 N 行开始绘制，跳过视口外的行。

### 3.3 LogColumnRenderer

```rust
impl Widget for LogColumnRenderer<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        for (vis_start, cell) in &self.cells {
            // 跳过视口外的 cell
            // 计算 visible_start/visible_end
            // 调 cell.render_partial(cell_area, buf, skip_lines)
        }
    }
}
```

按视觉行偏移排列 cell，做二次视口裁剪后逐个绘制。

---

## 覆盖层

| 覆盖层 | 文件 | 功能 |
|--------|------|------|
| Thinking cards | `cells/thinking.rs` | 思考块折叠指示器（"↑ 3/15 blocks hidden ↑"） |
| Diff cards | `cells/diff.rs` | 差异块卡片覆盖 |
| Code cards | `cells/code.rs` | 代码块卡片覆盖 |

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

### LogScroll (`state/log_scroll.rs`)

| 字段 | 类型 | 说明 |
|------|------|------|
| `offset` | `u16` | 逻辑行滚动偏移 |
| `state` | `ScrollbarState` | ratatui 滚动条状态 |
| `height` | `u16` | 面板可用高度 |
| `visual_cache` | `Vec<Line>` | 所有视觉行（已折行，不含搜索/选中样式） |
| `visual_start_cache` | `Vec<usize>` | 逻辑行 → 视觉行起始位置（前缀和） |
| `visual_cache_width` | `u16` | 缓存时的面板宽度 |
| `visual_cache_ver` | `usize` | 缓存版本号 = `messages.len()` |
| `visible_indices_ver` | `usize` | 可见索引版本号 |
| `phys_to_logical_cache` | `Vec<Option<usize>>` | 物理→逻辑反向映射 |

### TextCell (`cells/text.rs`)

| 字段 | 说明 |
|------|------|
| `cached_lines` | 预折行的视觉行（纯文本，无样式） |
| `raw_text` | 原始文本（用于搜索高亮重建） |
| `search_term` | 当前搜索词 |
| `is_search_match` | 是否命中搜索 |
| `is_selected` | 是否被鼠标选中 |
| `word_selection` | 词级选择 `(start_byte, end_byte)` |
| `prefix` | 首行前缀（thinking 折叠提示） |
| `fg_color` | 前景色 |

### Renderable trait (`renderable.rs`)

```rust
trait Renderable {
    fn height(&self, width: u16) -> u16;
    fn render(&self, area: Rect, buf: &mut Buffer);
    fn render_partial(&self, area: Rect, buf: &mut Buffer, skip_lines: usize);
}
```

---

## 设计要点总结

1. **版本号驱动的增量缓存** — `visual_cache_ver`、`visible_indices_ver` 用消息数量做版本号，只在变化时重建；`visual_cache_width` 额外检测宽度变化
2. **三维度索引映射** — 物理消息 → 逻辑消息 → 视觉行，用前缀和 + 二分查找高效转换
3. **渲染策略分离** — 缓存存原始文本，搜索/选中时现场重建样式行，避免缓存膨胀
4. **视口裁剪贯穿始终** — Phase 2 粗粒度过滤，`LogColumnRenderer` 二次过滤，cell 内部 `skip_lines` 精确裁剪
5. **Widget 组合模式** — `LogColumnRenderer` 实现 ratatui `Widget` trait，可直接 `frame.render_widget()`
