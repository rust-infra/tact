# 团队协调（Team Coordination）

> 语言：[中文](./14_chapter_team_zh.md) · [English](./14_chapter_team.md)

本章说明 Tact 的 **多 agent 团队原语**：具名 teammate 的持久 roster，以及支持点对点消息、广播与结构化协议请求（plan 审批、shutdown）的文件 backed inbox 系统。实现位于 `crates/tact/src/team.rs`，工具包装在 `crates/tact/src/tool/team.rs`。

重要前提：目前是 **协调数据层**，非编排引擎。「Spawn」teammate 仅创建 roster 记录 —— 不会启动第二个 agent 进程。见 [当前缺口](#8-当前缺口)。

---

## 1. 团队层提供什么

| 能力 | Tool | 底层调用 |
|------|------|----------|
| 注册 teammate | `spawn_teammate` | `TeammateManager::spawn_teammate` |
| 列出 roster | `list_teammates` | `list_teammates` |
| 点对点消息 | `send_message` | `send_message` |
| 群发 | `broadcast` | `broadcast` |
| 读 inbox | `read_inbox` | `read_inbox` |
| Plan 审批请求 | `plan_approval` | `protocol_request(kind = "plan_approval")` |
| Shutdown 握手 | `shutdown_request` / `shutdown_response` | `protocol_request(kind = "shutdown_request" / "shutdown_response")` |

八个工具均在主 agent `toolset()` 注册；`subagent_toolset()` 中均无。

---

## 2. 数据模型

### Roster

```rust
pub struct TeammateRecord {
    pub name: String,
    pub role: String,
    pub status: String,   // 目前始终 "idle"
}

pub struct TeamConfig {
    pub teammates: Vec<TeammateRecord>,
}
```

### Inbox 消息

```rust
pub struct InboxMessage {
    pub from: String,
    pub to: String,
    pub body: String,
    pub kind: String,     // "message" | "plan_approval" | "shutdown_request" | "shutdown_response"
    pub created_at: DateTime<Utc>,
}
```

`kind` 区分普通聊天与协议流量；存储路径两者相同。

---

## 3. 存储布局

`TeammateManager` 组合 [Store 与持久化](./01_chapter_store_zh.md) 中的两个 JSON store 原语：

```rust
config:  root.file("team/config.json")?,     // Store<TeamConfig> — roster
inboxes: root.collection("team/inbox")?,     // CollectionStore<InboxMessage> — 每个 owner 一个 JSONL 文件
```

磁盘上：

```text
.claude/
└── team/
    ├── config.json          # roster: [{name, role, status}, …]
    └── inbox/
        ├── alice.json       # 每条投递一行 InboxMessage JSON
        └── bob.json
```

消息 **追加**（JSONL）—— inbox 只增；无 read-cursor、ack 或 delete。

---

## 4. 消息流

```mermaid
sequenceDiagram
    participant Lead as Agent (lead)
    participant TM as TeammateManager
    participant FS as .claude/team/

    Lead->>TM: spawn_teammate("alice", "reviewer")
    TM->>FS: config.json ← roster + alice (status: idle)

    Lead->>TM: broadcast(from: "lead", body: "Standup in 5")
    loop each teammate in roster
        TM->>FS: inbox/{name}.json ← append InboxMessage
    end

    Lead->>TM: plan_approval(from: "lead", to: "alice", body: "Approve plan v2")
    TM->>FS: inbox/alice.json ← append (kind: plan_approval)

    Lead->>TM: read_inbox("alice")
    TM->>FS: read_all_from("alice")
    TM-->>Lead: pretty-printed JSON array
```

语义说明：

- `spawn_teammate` 拒绝重名（`teammate {name} already exists`）。
- `broadcast` 遍历 roster 并对每个 teammate 调用 `send_message` —— 发送者若在 roster 上也会收到自己的广播。
- `read_inbox` 返回 **完整** inbox 的 pretty-printed JSON，或 `"Inbox is empty."`。
- `protocol_request` 即带调用方选定 `kind` 的 `send_message` —— 无状态机验证 `shutdown_response` 是否跟随 `shutdown_request`。

---

## 5. 并发包装

`SharedTeammateManager` 与 task、worktree、background manager 同模式：

```rust
pub struct SharedTeammateManager {
    inner: Arc<Mutex<TeammateManager>>,
}
```

每个公开方法经 `with_manager` 委托，加锁并将 poisoning 转为 error。共享句柄在 `ToolContext.teammate_manager`，在 `tui.rs` 启动时构造一次：

```rust
let teammate_manager = SharedTeammateManager::new(TeammateManager::new(&store_root)?);
```

同一进程内所有工具共享同一 manager，进程内访问串行化；跨进程无（JSON store 无文件锁）。

---

## 6. 「Teammate」究竟是谁？

模型按设计自由：`from` 与 `to` 是 LLM 提供的 plain string。Nothing 验证：

- 发送者是否在 roster 上，
- 接收者是否存在（发给未知名会静默创建 `inbox/{name}.json`），
- teammate 是否 ever 读 inbox。

预期模式是协调 agent 将 roster 作共享状态、inbox 作 durable mailbox，供 eventual worker 抽象消费（[子 agent](./12_chapter_subagent_zh.md) 经 `task` 工具运行是最接近的现有类比，但未与 inbox 接线）。

---

## 7. 代码地图

| 文件 | 角色 |
|------|------|
| `crates/tact/src/team.rs` | `TeammateManager`、`SharedTeammateManager`、`TeamConfig`、`InboxMessage` |
| `crates/tact/src/tool/team.rs` | 八个 `#[tool]` 包装 |
| `crates/tact/src/tool/mod.rs` | `ToolContext.teammate_manager` |
| `crates/tact/src/tool/registry.rs` | `toolset()` 中的 team 工具 |
| `crates/tact-ui/src/headless.rs`、`interactive.rs` | 启动时从 `StoreRoot` 构造 manager |
| `crates/tact/src/store/mod.rs` | 底层使用的 `Store` / `CollectionStore` 原语 |

---

## 8. 当前缺口

| 缺口 | 详情 |
|------|------|
| 无实际 agent 进程 | `spawn_teammate` 仅记录名字；不启动 runtime、LLM 循环或 inbox 轮询 |
| Status 从不变化 | 每个 teammate 永远 `"idle"`；无 API 修改 `status` |
| 无发送者/接收者校验 | 发给未知名的消息静默创建 orphan inbox 文件 |
| Inbox 无界增长 | 仅追加 JSONL，无 read-cursor、ack 或修剪 |
| 协议 kind 仅为约定 | `plan_approval` / `shutdown_*` 无强制 request-response 配对 |
| 无 teammate 移除 | 无 `remove_teammate`；roster 只能增 |
| 无跨进程锁 | 并发 tact 进程可交错 roster read-modify-write |

---

## Related Docs

- [Store 与持久化](./01_chapter_store_zh.md) — `Store` / `CollectionStore` 原语与 `team/` 路径
- [工具系统](./07_chapter_tool_zh.md) — `ToolContext`  plumbing 与子 agent 工具集
- [子 Agent](./12_chapter_subagent_zh.md) — `task` 运行真实嵌套 agent；teammate 不运行
- [Worktree Lanes](./15_chapter_worktree_zh.md) — 真实多 agent 团队会配对的隔离原语
- [ARCHITECTURE.md](../ARCHITECTURE.md) — §7 子 agent、team、tasks、worktrees
