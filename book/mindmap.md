# Tact Book Mind Map

**Right-hand tree layout** (root on the left → topic column → descriptions on the right). Works better than a radial mind map for all 26 chapters.

## Interactive version (recommended)

Open **[mindmap.html](./mindmap.html)** in a browser — also embedded in [index.md](./index.md). Dark theme, color-coded topics, chapter links on the right.

![Tact Book mind map (right-hand tree)](./mindmap.png)

---

## Layout options

| Layout | File | Notes |
|--------|------|-------|
| **Right-hand tree** | `mindmap.html` / `mindmap.png` | Root → topics → details (**default**) |
| Reading paths | Mermaid below | Three common entry paths |
| Runtime pipeline | Mermaid below | Single chain 01→18 |

---

## Mermaid approximation (right-hand tree)

Mermaid cannot draw `{` braces; this `flowchart LR` approximates the same structure:

```mermaid
flowchart LR
    ROOT["Tact Book<br/>26 chapters"]

    ROOT --> B1 & B2 & B3 & B4 & B5 & B6 & B7 & B8

    B1["① Runtime order<br/>Ch 1–11"] --> D1["Store → Skill → Memory → Prompt<br/>→ Compact → Recovery → Tool<br/>→ MCP → Hook → Permission → Scheduling"]

    B2["② Tool families<br/>Ch 12–15"] --> D2["Subagents · Background · Team · Worktree"]

    B3["③ Off-path<br/>Ch 16–17"] --> D3["Cron · Notify"]

    B4["④ Capstone<br/>Ch 18"] --> D4["agent_loop · streaming · TaskComplete"]

    B5["⑤ Deep topics<br/>Ch 19–20"] --> D5["Persistent Tasks · Hallucination"]

    B6["⑥ Bootstrap & UI<br/>Ch 21–25"] --> D6["Config → LLM → TUI → Protocol"]

    B7["⑦ Quality<br/>Ch 24"] --> D7["Mock LLM · driver tests · TestBackend"]

    B8["⑧ Issue log<br/>Ch 26"] --> D8["Shipped optimizations · bug fixes"]
```

---

## Reading paths

```mermaid
flowchart LR
    subgraph A["Path A · New provider"]
        A21["21 Config"] --> A22["22 LLM"] --> A23["23 TUI"] --> A25["25 Protocol"]
    end

    subgraph B["Path B · Understand loop"]
        B01["01–06"] --> B07["07–11"] --> B18["18 Loop"]
    end

    subgraph C["Path C · Testing"]
        C18["18 Loop"] --> C24["24 Testing"]
    end
```

| Path | Chapters | When to use |
|------|----------|-------------|
| A | 21 → 22 → 23 → 25 | New provider or binary |
| B | 1 → 11 → 18 | One full agent_loop turn |
| C | 18 → 24 | Integration tests |

---

## Chapter index

| # | Chapters | Group |
|---|----------|-------|
| 1–11 | [Store](./01_chapter_store.md) … [Scheduling](./11_chapter_task.md) | ① Runtime order |
| 12–15 | [Subagent](./12_chapter_subagent.md) … [Worktree](./15_chapter_worktree.md) | ② Tool families |
| 16–17 | [Cron](./16_chapter_cron.md) · [Notify](./17_chapter_notify.md) | ③ Off-path |
| 18 | [Agent Loop](./18_chapter_agent_loop.md) | ④ Capstone |
| 19–20 | [Tasks](./19_chapter_persistent_tasks.md) · [Hallucination](./20_chapter_hallucination.md) | ⑤ Deep topics |
| 21–23, 25 | [Config](./21_chapter_config.md) … [Protocol](./25_chapter_protocol.md) | ⑥ Bootstrap & UI |
| 24 | [Testing](./24_chapter_testing.md) | ⑦ Quality |
| 26 | [Issue Log](./26_chapter_issue.md) | ⑧ Engineering changelog |
