# Remote MCP (Streamable HTTP + OAuth 2.0) 设计

> 日期：2026-09-11 · 状态：**已批准 / 已实现**（用户确认：本地回环自动回调 + 启动不阻塞 + `/mcp auth` 补做 + 关键点加日志）
> 关联：`crates/tact/src/mcp/`、`crates/tact/Cargo.toml`、`crates/tact/src/agent/mod.rs`、`crates/protocol/src/agent.rs`、`crates/tact-ui/src/driver.rs`、`crates/tui/src/handlers/`、`book/08_chapter_mcp*.md`、`book/21_chapter_config*.md`、`book/26_chapter_issue*.md`

## 1. 背景与动机

Tact 的 MCP 客户端只支持 stdio（`McpClient::connect` → `TokioChildProcess`）。配置解析层其实已经认识远程条目（`McpProjectConfig` 的 `type`/`url`），但 `resolve_servers` 把它们记进 `skipped_remote` 后**丢弃**，运行时不连接（`crates/tact/src/mcp/mod.rs`）。后果：

- 远程 MCP 服务（Streamable HTTP / SSE）完全不可用；
- 2026-09-10 接入的 OpenAI `openai-curated` catalog 中，带远程 MCP 的插件**可安装但不能用**；
- 用户写 `{ "url": ... }` 无任何反馈，只能从 `notice_lines` 看到「skipped」。

同时，远程 MCP 端点普遍要求鉴权（MCP 规范 2025-06-18 / SEP-985 约定 OAuth 2.0 授权码 + PKCE + 动态客户端注册），因此只做 transport 不解决实际问题。

## 2. 现状与可复用点

- `McpService` trait（`list_all_tools` / `call_tool` / `cancel`）已把「传输」与「客户端/路由/工具表」解耦：`McpClient` 的两个构造入口 `try_new`（真实）与 `with_service`（测试替身）都只依赖它。远程接入**只需替换 `connect` 产出的 transport**，上层 `MCPToolRouter`、工具命名、权限分类全部不动。
- rmcp 0.17.0 原生提供所需能力（当前 features 未启用）：
  - `rmcp::transport::streamable_http_client::{StreamableHttpClientTransport, StreamableHttpClientTransportConfig}`（真 Streamable HTTP，含会话管理、SSE 自动重连、`auth_header`、`custom_headers`、`allow_stateless`）。
  - `rmcp::transport::auth`（feature `auth`）：`AuthorizationManager`（`.well-known` 元数据发现、RFC 8414、动态客户端注册、scope 升级、token 自动刷新）、`AuthorizationSession`（`get_authorization_url` / `handle_callback`）、`CredentialStore` / `StateStore`（可插拔持久化）。
- `rmcp` 的 `bearer_auth(auth_header)` 语义已确认：`auth_header` 是**不含 `Bearer ` 前缀的裸 token**。
- 仓库已有 `reqwest`（0.12）与 `http` 1.x（lock 内），rmcp 的 `auth`/`reqwest` feature 会引入自己的 `reqwest` 0.13（lock 中已存在，无新增版本族）。

## 3. 范围

### 在内（v1）

**A. 远程 transport**

- `mcp.json` 条目新增远程形态（同一份配置、两种传输）：
  ```json
  {
    "mcpServers": {
      "remote": {
        "url": "https://mcp.example.com/mcp",
        "headers": { "X-Api-Key": "..." },
        "auth": { "type": "oauth", "clientId": "...", "scopes": ["tools.read"], "callbackPort": 0 }
      }
    }
  }
  ```
  - `url` + `type: "http"|"sse"` → Streamable HTTP（`type` 仅作提示；本版不做旧版 2024-11-05 纯 SSE 端点）。
  - `headers` → transport `custom_headers`（静态鉴权就在这里，如 `Authorization`）。
  - `auth.type = "oauth"` → OAuth 2.0 授权码 + PKCE 流程，token 作为 `auth_header` 注入。
- 内部把连接列表从 `McpServerConfig` 扩展为枚举 `McpTransportConfig::{Stdio, Remote}`；`resolve_servers` 产出远程项而非丢弃。
- 插件的 `.mcp.json` / manifest `mcpServers` 远程条目同样接入（去掉现有 `debug!` 跳过）。

**B. OAuth 2.0**

- **token 持久化**：`~/.tact/mcp/oauth/<server>.json`（`FileCredentialStore` 实现 `CredentialStore`）。含 client_id、token_response（access/refresh/expires_in）、granted_scopes、`token_received_at`。
- **连接时**：`AuthorizationManager::new(url)` + 注入 `FileCredentialStore` → `initialize_from_store()` → 有凭据则 `get_access_token()`（过期自动 refresh）；无凭据则记为「待授权」，**不阻塞启动**。
- **授权时**（`/mcp auth <server>`）：绑定 `127.0.0.1:<callbackPort|0>` 得到真实回调 URI → `discover_metadata` → `AuthorizationSession::new`（动态客户端注册）→ 打印授权 URL 并提示用户在浏览器打开 → 回环 server 收 `?code=&state=` → `handle_callback(code, state)` 换 token 并落盘 → 重载 MCP router 使该服务器立即生效。
- **刷新**：运行期 token 过期由 rmcp `get_access_token()` 自动 refresh；refresh 失败归为「需重新授权」。

**C. 可观测性（用户明确要求）**

- 关键点 `tracing` 日志（debug/info/warn）：
  - 解析：远程条目识别（server、url、transport、auth 类型，**不记 token/header 值**）；
  - 连接：transport 分派、URL、耗时、结果（成功/失败原因）；
  - OAuth：元数据发现 URL、动态注册 client_id、授权 URL、回调收到、token 交换成功（含 expires_in）、refresh 触发与结果、凭据落盘路径；
  - 失败：所有 `AuthError` 分类带 `error = %err`。
- 面向用户的摘要仍走 `McpLoadReport.notice_lines()`：新增「待授权」一行（含 `/mcp auth <name>` 提示）。

**D. 命令面**

- `/mcp auth <server>`（TUI）：执行 OAuth 授权流程 + 重载 MCP router。（已实现；`/mcp login <server>` 为等价别名。）
- **CLI 等价入口**（headless 无 TUI，必须有 CLI），按副作用拆分，任一命令不做两件事：
  - `tact-ui mcp list`：连接全部 server 并打印传输方式与状态；`tact-ui mcp get <name>`：只连接该 server，打印传输方式、来源、状态与工具名（`mcp__<server>__<tool>`）。
  - `tact-ui mcp add <name> --url <URL> [--oauth] [--header N:V]…` / `--command <CMD> [--arg A]… [--env N=V]…`；`--user` 写用户级文件，`--force` 覆盖同名条目。
  - `tact-ui mcp remove <name> [--user]`：只删声明；名字在别的文件时会提示 `--user`，插件提供的则说明无法在此删除。
  - `tact-ui mcp login <server>` / `tact-ui mcp logout <server>`：`login` 跑 OAuth 并落盘 token；`logout` 删除 `~/.tact/mcp/oauth/<server>.json`，不要求 server 仍被声明，无凭据时幂等成功。`auth` 为 `login` 的别名。
  - 原计划中的 `/mcp status` 由 `mcp list` 覆盖，TUI 内状态仍来自启动时的 `pending_auth`/`failures` 提示。
  - 安全：server 名同时是工具名段与凭据文件名，故含空白、控制字符或路径分隔符的名称一律拒绝；`oauth_credential_path` 也直接拒绝不安全名称，防止恶意 `mcp.json` 键或 `mcp logout <name>` 触达目录外文件。

### 不在内（v1 限制，文档写明）

- 旧版 2024-11-05 纯 HTTP+SSE 端点（非 Streamable）：跳过并报告。
- OAuth 以外的交互式授权（如设备码流程）、企业 SSO / mTLS、客户端证书。
- 远程 "apps" connector / 无 MCP 能力的 catalog 条目（现状不变）。
- 系统浏览器自动打开：v1 打印 URL 由用户手动打开（避免无桌面环境行为差异）。

## 4. 架构与数据流

```
mcp.json ──parse──> McpProjectConfig{command|url, headers, auth}
                        │
              resolve_servers (later-wins by name)
                        │
              McpTransportConfig::{Stdio, Remote}
                        │
        ┌───────────────┴────────────────┐
   Stdio                          Remote
 McpClient::connect            McpClient::connect_remote
 TokioChildProcess             auth? ──oauth──> FileCredentialStore
                               (无凭据→pending_auth，不阻塞)
                                        │
                              StreamableHttpClientTransport
                                        │
                                   McpService trait
                                        │
                                  McpClient / MCPToolRouter
```

启动路径不变：`load_mcp_router_with_report()` → `notice_lines()` → 交互式走 `AgentUpdate::Info`，headless 走 stderr。

## 5. 行为变更（用户可见）

1. `{ "url": ... }` / `type: "http"` 的服务器会被真实连接；`skipped_remote` 仅剩「不支持的 transport」。
2. OAuth 服务器首次需授权：启动时提示 `MCP server <name> needs authorization — run /mcp auth <name>`；执行后 token 落盘，后续启动免授权。
3. `/mcp auth <server>` 授权成功即热重载，无需重启。
4. 连接/授权失败不阻断 agent 启动（保持既有容错语义）。

## 6. 测试重点

- 配置解析：`url` 独立、`type: http|sse`、`headers`、`auth.type=oauth`（含 `clientId`/`scopes`/`callbackPort`）、`command` 优先 stdio、二者皆无的报错路径。
- `resolve_servers`：远程进入连接列表；`skipped_remote` 收窄；同名 later-wins 对远程同样生效。
- transport 分派：用 `McpService` mock 断言 stdio/remote 选择与工具表构建（不 spawn 真实子进程、不发真实网络）。
- OAuth：`FileCredentialStore` 读写往返、缺失→`AuthorizationRequired`、过期→refresh 路径（可用 mock/直接构造 `StoredCredentials`）、回调 `state` 校验失败拒绝。
- 回环回调：本地 `TcpListener` 起停、`?code=&state=` 解析、超时/取消。
- 报告：`notice_lines` 含待授权条目。

## 7. 依赖变更

- workspace `rmcp` features 增加 `transport-streamable-http-client-reqwest` 与 `auth`。
- `crates/tact` 增加 `http = "1"`（`custom_headers` 需要 `HeaderName`/`HeaderValue`）。

## 8. 文档同步

- `book/08_chapter_mcp.md` + `_zh.md`：transport 表、限制段、新增「Remote MCP & OAuth」小节。
- `book/21_chapter_config.md` + `_zh.md`：`mcp.json` 远程/`auth` 字段。
- `book/26_chapter_issue.md` + `_zh.md`：newest-first 条目。
