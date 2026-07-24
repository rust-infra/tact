# Task tool / Log / Sticky / DAG UI redesign

> Date: 2026-07-25  
> Status: implementing  
> Related: `docs/superpowers/specs/2026-07-24-task-progress-panel-design.md`

## Summary

Make persistent-task tooling readable in the TUI:

1. **Tool rows** — human titles (no raw JSON)
2. **Log** — short change cards (not full checklist spam)
3. **Sticky** — default-expanded dependency **tree** with `#id`
4. **`/tasks-dag`** — Mermaid→Unicode popup via `meraid` (nodes: status + `#id` only)

`task_list` / `task_get` do **not** emit `TasksChanged` / Log cards.

---

## 1. Tool title format

Final title (TUI `ToolDisplayKind::Task` uses `arg_summary` only — no `Task_update` prefix):

```text
N. # Task.24 · 完成任务 * 任务名: 后端接口 * 负责人:张2 * 被阻塞于: [12] -> [12, 24] * 阻塞: [20]
```

| Piece | Rule |
|-------|------|
| Prefix | `#` |
| Id | `Task.{id}`; create-before-id / list → `Task` |
| Primary action after `·` | create / 执行 / 完成 / 重置 / 删除 / 查看 / 列出 / **空更新** |
| Detail segments | ` * `-separated; include snapshot fields when present |
| 任务名 / 负责人 | from record when non-empty |
| 被阻塞于 / 阻塞 | if changed: `[old] -> [new]`; if unchanged: `[current]` only |
| Owner-only / dep-only | primary action = 设置负责人 / 被阻塞于 / 阻塞 as appropriate; multi: join with ` · ` in primary slot… **Revised:** primary is single status/list/get/create/空更新; extra mutations appear only in detail segments (负责人 / 被阻塞于 / 阻塞). If no status but owner set → primary `设置负责人`. If only deps → primary `被阻塞于` and/or detail both. |

**Generation:** `tool_dispatch` builds `arg_full`/`arg_summary` using TaskManager lookup. After successful mutate, rebuild summary from post-state and prefer that for `StepFinished` display when possible.

**EN locale:** English action verbs via Messages / parallel strings in agent (agent may use fixed ZH for v1 if Messages not available — prefer shared formatter with lang from config later; **v1: Chinese verbs** matching user examples; i18n follow-up OK).

---

## 2. Log short card

On each `TasksChanged` (create/update only):

```text
# Task.24 · 完成任务
被阻塞于: [12] -> [12, 24]
负责人:张2
```

- Line 1: `# Task.{id} · {primary}` (or `# Task · 创建任务` when id known post-create)
- Following lines: **fields that changed** vs previous TUI snapshot only
- Leading `📋` plain-text path retained so newlines stay
- No full board checklist in Log

---

## 3. Sticky

- Visible rules unchanged (session gate + open items)
- **Default `expanded = true`** when becoming visible
- Body: **tree** by `blocks` edges; roots = tasks with empty `blocked_by` (or not listed as anyone's block target)
- Multi-parent (**A1**): node may appear under each parent
- Row: `[x] #23 项目初始化 (老四)`
- Collapse → one-liner title; click toggles
- No `… +N` cap

---

## 4. `/tasks-dag`

- Palette/slash builtin opens scrollable overlay
- Mermaid `flowchart TD` from `blocks`; `meraid` Mono render
- Node label: `[x] #23` only (no subject)
- `y` copies Mermaid source; Esc closes
- Empty snapshot → hint message

Protocol: `TaskSnapshot` includes `blocks` + `blocked_by`.

---

## 5. Out of scope

- Sticky Mermaid mode toggle (DAG is slash-only)
- Full Mermaid interactive pan/zoom
- Changing TaskManager on-disk schema
