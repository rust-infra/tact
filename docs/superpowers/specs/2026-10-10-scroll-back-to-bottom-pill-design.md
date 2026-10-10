# 「回到最新」悬浮药丸 — 设计

**日期：** 2026-10-10
**状态：** 设计（未实现）
**参考：** Codex CLI 的 `New activity · ↓ Back to bottom · esc`（截图见 `docs/design/scroll-back-to-bottom.html`）

## 问题

往上翻日志看内容时，新的输出会到下面来，而用户看不到；Tact 今天也没有任何东西提示「下面还有新东西」，或者「你已经不在最新处」。

Codex 的做法是一行**悬浮药丸**：滚离底部时出现在输出区底部（输入框正上方），居中，内容为
`New activity · ↓ Back to bottom · esc`；没有新内容时退化为 `↓ Back to bottom · esc`。点它、或按 `esc`，回到最新。

## 前置条件（这是本设计真正的难点，不是药丸本身）

药丸只有在「新行**不会**把你拽回底部」时才有意义。Tact 今天**两套行为并存**：

- 工具卡 Resize 路径**尊重**用户的滚动位置 —— `let was_pinned = self.is_log_pinned_to_bottom(); … if was_pinned { scroll_log_to_bottom() }`（`agent.rs:283-289`）；
- 而消息追加路径**无条件**把你拽回底部 —— `messages.rs:326/549`、`extensions.rs:188/244`、`visibility.rs:627/706`、`agent.rs:367/383/893` 全都是 `if input_mode is Insert|Normal { scroll_log_to_bottom() }`。

所以今天往上翻、来一条流式输出，你**有时**会被弹回底部，**有时**不会。先把这个统一掉，药丸才有意义。

**决策：把「跟尾」策略收敛到两个追加原语里**（`App::append_msg`、`App::extend_msgs`，`popups.rs:404/444`）：

```rust
let was_pinned = self.is_log_pinned_to_bottom();
// … 推入这一行 …
if was_pinned { self.scroll_log_to_bottom() } else { self.log_scroll.unseen = true }
```

然后删掉上面那 8 处无条件调用。一句话规则：**原本贴着底 → 继续贴着底；否则保持你的位置，并点亮药丸。**

**8 处的归类（逐处看过）：**

| 调用点 | 谁触发 | 处置 |
|---|---|---|
| `agent.rs:893`（每次 `AgentUpdate` 之后） | agent 输出 | 删 —— **这是「往上翻会被弹走」的主凶** |
| `agent.rs:367 / 383`（TaskComplete / TaskCancelled） | agent | 删 |
| `messages.rs:549`（`add_task_stats_block`，回合结束冻结行） | agent | 删 |
| `visibility.rs:627`（thinking 增长） | agent | 删 |
| `visibility.rs:706`（工具卡增长） | agent | 删 |
| `extensions.rs:187 / 243`（系统 / 表格追加） | agent | 删 |
| `messages.rs:323 scroll_after_message()` | **混的** | 删 —— 它服务 `add_system_message`，而那个 API 同时被 agent 的 `Info` 与用户动作调用，**在调用点上分不出来** |

**一处必须显式保留：提交消息时的重新跟尾。** 用户按下回车是明确的动作，自己的回显应该出现在眼前 —— 但这条规则属于**提交处理器**，不属于追加原语。所以：`append_msg` 里放「跟尾策略」，提交路径上额外一次显式 `scroll_log_to_bottom()`。把 8 处全删干净而不加这一处，就会变成「我发了消息但看不到自己发的」。

**坑（比上面这些都重要）：`visual_top` 只在第一帧之前是 `usize::MAX`。** `render_log_panel` 每帧把钳制后的值**写回**（`render/log.rs:323`），所以第一帧之后「跟尾」就不再由 sentinel 表达，而是靠 `is_log_pinned_to_bottom()` 的缓存路径判断；而那条路径要求 `visible_indices_ver == items.len()` —— **追加一行的瞬间缓存必然过期**，于是它返回 `false`。

两个后果：
- `was_pinned` **必须在推入这一行之前取**（先判断再追加），否则每一行都被判成「用户滚走了」，日志从此再也不跟尾；
- 更稳的做法是**把跟尾状态显式化**：给 `LogScroll` 加 `follow: bool`（用户滚动 → false；`scroll_log_to_bottom()` → true），不再从 `visual_top` 和缓存版本号反推。代价是多一个字段、要在 `scroll_log_to_bottom` 与滚动处理器里各置一次；换来的是免疫缓存过期和「原地重写行但行数不变」那类边界（见项目记忆里 visual cache token 的坑）。

**建议按显式 `follow` 做** —— 反推版本在单测里能过，但会在真实使用中偶发「日志不跟尾」，而且很难复现。

## 状态

| 字段 | 位置 | 含义 |
|---|---|---|
| `visual_top` | `LogScroll`（已有） | `usize::MAX` = 跟尾；具体值 = 用户自己定的位置 |
| `unseen: bool` | `LogScroll`（新） | 离开底部期间**有新行到达**（药丸上的 `New activity ·` 徽章） |

- 显示条件：`!is_log_pinned_to_bottom()`。
- 徽章条件：`unseen`。
- `unseen` 置位：追加行且未贴底（上面那段）。
- `unseen` 清零：任何回到最新的路径（药丸点击 / `esc` / `G` / 滚到底）。

## 几何与绘制

- **一行**，落在**日志区域的最后一行** —— 也就是 Log 面板的下边框行；粘性条（Tasks/Subagent/Background）可见时，是那条的底边框行（`STICKY_BORDER_ROWS` 已经占掉这一行，药丸画在它上面）。
- **靠右下停靠，不居中。** 这是本设计里最容易做错的一处：**实时统计行是居中的**，药丸若也居中，两条居中的行上下叠在一起就糊成一坨 —— 用户一眼看到的就是「和实时统计混在一起了」。停靠右端把两者的**轴**分开：左边是内容（域摘要/日志），右下是 chrome。
  - Codex 的药丸是居中的，但它上面那行 `• Working (21s · esc to interrupt)` 是**左对齐**的 —— 它从来没有两条同轴的行，所以居中在那边不糊，在 Tact 这边会糊。
  - 附带好处：粘性条可见时，域摘要本来就左对齐，两者天然不抢轴（终端里两个 span 不会互相覆盖，摘要按药丸的左边界裁掉即可）。
- **宽度不够就不画。** 药丸宽约 30–44 列（含 `New activity · ` 徽章），日志区放不下时整只隐藏 —— 与吉祥物同一条规则：放不下就不画，不挤内容。
- 它是 **overlay**，所以遵守 AGENTS.md 的覆盖层不变量：先 `Clear` 自己的矩形（Clear 矩形 ⊇ 绘制矩形），再整块刷 band 背景，最后画字形 —— 尾巴不能留旧样式。
- **宽度 = 文本宽 + 4**：左圆角端 + 1 列内边距 + 文本 + 1 列内边距 + 右圆角端。靠右停靠，右端距面板右边界留 1 列。
- **圆角端用 Nerd Font 的 Powerline 半圆字形**（终端里没有 `border-radius`，一个格子就是矩形，这是标准替身）：
  - 左端 `ple-left_half_circle_thick` `U+E0B6`，右端 `ple-right_half_circle_thick` `U+E0B4`。
  - **两个端格是唯一不属于 band 的格子**：`fg = theme.status_bar_bg`、`bg = theme.bg` —— 形状用 band 色画在面板底色上，圆角才看得见；若端格也刷成 band 底色，端头就退化成直角。
  - 已对三个本地 Nerd Font（`JetBrainsMonoNerdFontMono` / `HackNerdFontMono` / `0xProtoNerdFontMono`）核过：两者 advance 与 `0` 完全相同（正好 1 列），墨迹满格高，且各自**略微溢出自己的格子** —— 所以端头与 band 之间没有缝。
  - 风险与吉祥物同：PUA 字形，没有 Nerd Font 就是豆腐。Tact 已事实要求 Nerd Font（`󰜼` 按钮 + 吉祥物），所以暂不加开关。
- 配色：band 用 `theme.status_bar_bg`（各主题里它就是「比正文底亮一档的 chrome 色」，Japanese 下 `#232837` on `#191E2D`，正是 Codex 那条微亮的带子）；`New activity` 用 `theme.muted_fg()`，动作与按键用 **`theme.warning`**（amber —— 与 B 档同一取舍：这一行左边已是 accent 红，第三个红字会读成标题的一部分）。Codex 是整条一个蓝色，这里分两色是因为「状态」与「动作」不是一回事。
- 不画的情况：日志区域行数不够（药丸放不下）、或当前是 `Palette` / `FilePicker` / `Select` 等覆盖模式（那些模式自己占屏）。

## 文案

`{badge} · {action} · {key}`，三段都在 i18n 里（`Messages`，英中两份）：

| key | EN | ZH |
|---|---|---|
| `scroll_pill_new_activity` | `New activity` | `新活动` |
| `scroll_pill_back_to_bottom` | `↓ Back to bottom` | `↓ 回到最新` |
| `scroll_pill_key` | `esc` | `esc` |

徽章不存在时整段连同分隔符一起省略（`↓ Back to bottom · esc`），不留悬空 `·`。

## 交互

- **点击**药丸 → 回到底部。命中矩形由渲染函数返回（照 `render_sticky_host` 返回 `tab_areas`、`render_log_panel_pure` 返回 cancel-button 矩形的同一套做法），app 侧存进 `app.mouse`。
- **`esc`** → 回到底部。这是本设计里唯一需要动既有语义的地方：

  Tact 的 `esc` 已经是一条**分层「退出当前视图状态」的梯子**：slash 弹窗打开 → 关弹窗；语音中 → 取消语音；否则 Insert 模式 → 回 Normal；Normal 模式 → 清选区。**「滚离底部」也是一种视图状态**，所以新的一级插在「退出 Insert」**之上**：

  ```
  slash 弹窗 → 语音 → 滚离底部（回到底部）→ 退出 Insert → 清选区
  ```

  即：滚离底部时按 `esc` 先回底部（消费掉），再按一次才退 Insert。这**改变**了「滚离底部时 Insert 按 esc」的行为，但药丸把 `esc` 写在脸上，所以是可发现的；而且 `esc` 是唯一可用的键 —— `End` 在 Insert 模式已经绑给输入光标了（`insert.rs:565`）。

- 滚轮/PageDown 自己滚到底 → 与药丸无关地清掉徽章（`unseen` 是「不在底部」的派生状态）。

## 被否掉的方案

| 方案 | 为什么否掉 |
|---|---|
| **居中**（照 Codex 抄） | **和实时统计行同轴**。统计行居中、药丸也居中 → 两行叠着读成一坨。Codex 居中不糊只因为它上面那行是左对齐的；Tact 没有那个前提。（这是第一版设计稿犯的错。） |
| 画在实时统计行**同一行**的右端 | 统计行在 120 列面板里宽约 89 列，右端只剩十几列，装不下 28–30 列的药丸；要让它装得下就得把统计行挤左，数字会动。 |
| 常驻预留一行 | 空闲会话白花一行。药丸是 overlay，0 行成本。 |
| 画在实时统计行上 | 那一行是这一轮的读数，药丸会盖住数字。 |
| 用 `End` 当按键 | Insert 模式下 `End` 已是「输入光标到行尾」。 |
| 复用粘性条的 tab 行 | 粘性条是任务/子代理/后台三个域自己的界面，且只在可见时存在；药丸要在任何状态下都在。 |
| 让新行永远把你拽回底部（现状的一部分） | 那样药丸永远不出现，而且往上读的时候被弹走是更糟的体验。 |

## 测试

- 纯函数：两种状态的文案；宽度与居中；徽章缺失时不留悬空分隔符。
- 行为（`App` 层）：贴底时追加 → 仍贴底、无徽章；滚离后追加 → 位置不变、徽章点亮；`esc` → 贴底 + 徽章清除；**贴底时 Insert 按 `esc` → 仍退出 Insert**（既有用例必须保持绿）。
- 缓冲区：药丸 band 覆盖整个矩形（每个格子都带 band 背景，不留旧样式）；文字列与返回的命中矩形一致；点击命中矩形 → `visual_top == usize::MAX`。
- **与实时统计行共存（钉住这次的教训）**：任务在飞 + 滚离底部 → 统计行仍在最后一行内容行上且**居中**，药丸在**下边框行**上且**靠右**；断言两者的矩形**既不同行、也不同轴**（药丸的右端贴右边界，且与统计行的水平区间不相交）。
- 与粘性条共存：域摘要文本在药丸左边界前被裁掉，两者不覆盖。

## 三档做法（改动量从小到大）

三档的**状态与策略完全一样**（`follow` / `unseen` + 追加原语里的策略），差的只是「怎么把这件事说出来」。
所以选小的一档不是妥协 —— 上面那层随时可以换。

| | 做法 | 多出来的改动 | 有提示吗 | 能点吗 |
|---|---|---|---|---|
| **A** | 只修跟尾策略，不画任何东西 | —— （6 个文件，纯删改） | ✗ | ✗ |
| **B** ⭐ | A + 提示挂在**输入框上边框右端** | `RenderCtx` 加 2 个 bool、`render_input_box` 加一个**右对齐 title**、i18n 2 条、`insert.rs` 1 个 match arm | ✓ | ✗（只有 `esc`） |
| **C** | A + Codex 式悬浮药丸 | B 的全部，再加新模块 ~120 行 + 命中矩形 + 鼠标路由 + 宽度守卫 + 2 条缓冲区测试 + 与粘性条的裁切关系 | ✓ | ✓ |

**B 之所以这么便宜**：`render_input_box` 已经在往同一个 `Block` 上挂**多个 title** —— 左边是 `input_box_title`，中间是语音标签（`input.rs:229-235`）。再加一个 `.alignment(Alignment::Right)` 的 title 是**同一个模式**，落点就是输入框上边框右端（比 Codex 的药丸低一行、靠右，顺带也不跟居中的统计行抢轴）。零新渲染单元、零 overlay、零命中矩形、零缓冲区不变量。

**推荐 B。** 这个功能的价值九成在「往上翻不再被弹走」——那是 A 就有的；提示只是把它说出来。B 用约 15 行把这件事说出来，C 的另外 150 行买的是「鼠标可点」和那个 band 的样子。

### B 的渲染规格

输入框上边框那一行现在是两个 title（左 `input_box_title`、中语音标签），B 加**第三个、右对齐**：

```
❯ Input (Shift/Alt+Enter=newline)        ⏺ Voice          新活动 · ↓ 回到最新 · esc
╭───────────────────────────────────────────────────────────────────────────────╮
```

| 段 | 样式 | 说明 |
|---|---|---|
| `新活动` / `New activity` | `theme.muted_fg()` | 状态，不是动作 |
| ` · ` | `theme.muted_fg()` | 徽章不存在时**连分隔符一起省略** |
| `↓ 回到最新 · esc` | **`theme.warning`** | 动作 + 按键 |

**为什么动作不用 `theme.accent`**：这一行左边的 `❯ Input (…)` 和中间的 `⏺ Voice` 已经是 accent 了，第三个 accent 会读成标题的一部分。`theme.warning`（amber）在本项目里已经是「这一层在动」的颜色（吉祥物用的也是它），而且它是**动态出现**的，本来就该跟静态标题区分开。

- 徽章条件是 `unseen`（离开底部期间有新行）；没有徽章时只显示 `↓ 回到最新 · esc`。
- 宽度 < 70 列不画 —— 再窄就会和左侧 title 挤在一起（ratatui 的多个 title 不会互相让位）。
- 这一行**没有 band 背景**：它就是边框上的文字（和现有 title 一样），比 C 的药丸轻。
- 位置比 C 低一行、且右对齐 —— 与居中的实时统计行**既不同行也不同轴**。
- **可点。** 渲染时顺手算出那串字形的矩形并返回（`InputBoxHitAreas { cancel, scroll_back }`，与现有 `[Cancel]` 按钮完全同一个契约：kit 纯渲染 → 返回矩形 → host 记录 → 鼠标处理器命中即动作）。命中矩形**精确覆盖字形**：右对齐的 block title 结束于右边框内侧一列，所以矩形是 `[right-1-width, right-1)`；不画时返回 `Rect::default()`，因此旧的矩形永远不会吃掉一次点击。鼠标处理器里它排在语音按钮之后、`[Cancel]` 之前 —— 三者都在输入框上，互不重叠。
  所以 B 与 C 的差别只剩「有没有 band 背景」这一条。

窄宽度的处理：挂输入框时右对齐 title 会被 block 裁掉，与左侧 title 挤在一起就省略提示（`< 70` 列不画）。

## 实现顺序

1. `LogScroll::{follow, unseen}` + 把跟尾策略收进 `append_msg` / `extend_msgs`，删掉 8 处无条件 `scroll_log_to_bottom()`，**并在提交处理器里补一次显式跟尾**。**这一步单独就能让滚动行为变正确**，先做、先测，不带药丸也能上线。
2. i18n 三条文案（英中）。
3. `agent_tui_kit::render::scroll_pill`（新模块，纯渲染 + 返回命中矩形），在 `task_panel.rs` 的粘性条之后调用。
4. `esc` 新的一级 + 鼠标命中路由。
5. 文档：`book/23_chapter_tui_zh.md`（§6.6 底栏 / §6.11 Log 消息模型）、`book/26_chapter_issue_zh.md` 新条目。
