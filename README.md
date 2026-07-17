<p align="center">
  <img src="./tact.png" alt="tact" width="200" />
</p>

<h1 align="center">tact</h1>

<p align="center">
  <strong>Terminal-first AI coding agent. Built in Rust. MIT licensed.</strong>
</p>

<p align="center">
  <a href="#quick-start"><strong>Quick Start</strong></a> ·
  <a href="#features"><strong>Features</strong></a> ·
  <a href="#architecture"><strong>Architecture</strong></a> ·
  <a href="#comparison"><strong>Comparison</strong></a> ·
  <a href="#configuration"><strong>Configuration</strong></a>
</p>

<p align="center">
  <img src="https://img.shields.io/badge/language-Rust-orange?style=flat-square&logo=rust" alt="Rust" />
  <img src="https://img.shields.io/badge/license-MIT-green?style=flat-square" alt="MIT License" />
  <img src="https://img.shields.io/badge/version-0.19.0-blue?style=flat-square" alt="Version" />
  <img src="https://img.shields.io/badge/platform-macOS%20%7C%20Linux%20%7C%20WSL-lightgrey?style=flat-square" alt="Platform" />
</p>

---

## What is tact?

tact is a **terminal-first AI coding agent** that lives inside your terminal. It reads your codebase, understands your intent, and executes — editing files, running commands, searching code, and coordinating with sub-agents. Think Claude Code or Cursor, but:

- 🦀 **Written in Rust** — a single small binary, no Electron, no Node.js
- 🏠 **Fully self-hosted** — your code never leaves your machine
- 🔓 **MIT licensed** — truly open source, not "source available"
- 🧩 **Extensible** — MCP plugins, custom skills, hooks, and tool macros

```
$ tact-ui headless "Add a --verbose flag to the CLI and update the README"
```

That's it. No YAML config wizard. No "sign up for waitlist." Just a prompt and a terminal.

---

## Quick Start

### 1. Install

**Linux / macOS**

```bash
curl -fsSL https://raw.githubusercontent.com/rust-infra/tact/main/scripts/install.sh | bash
```

Or from a clone:

```bash
./scripts/install.sh --from-source
```

**Windows (PowerShell)**

```powershell
irm https://raw.githubusercontent.com/rust-infra/tact/main/scripts/install.ps1 | iex
```

Or from a clone:

```powershell
.\scripts\install.ps1 -FromSource
```

The installer builds `tact-ui` from source by default (installs Rust via rustup if needed). When GitHub release assets are published, pass `--release` / `-Release` to download a pre-built binary instead:

```bash
curl -fsSL https://raw.githubusercontent.com/rust-infra/tact/main/scripts/install.sh | bash -s -- --release
```

Install options:

| Platform | Flag | Meaning |
|----------|------|---------|
| Unix | `--install-dir DIR` | Install location (default: `~/.local/bin`) |
| Unix | `--system` | Install to `/usr/local/bin` |
| Unix | `--release` | Prefer GitHub release, fall back to source |
| Windows | `-InstallDir PATH` | Install location (default: `%USERPROFILE%\.local\bin`) |
| Windows | `-Release` | Prefer GitHub release, fall back to source |

**Manual build from source**

Linux: install SQLite build dependencies first (required by `sqlx` / session store):

```bash
sudo apt-get update
sudo apt-get install -y libsqlite3-dev pkg-config clang libclang-dev
```

```bash
git clone https://github.com/rust-infra/tact.git
cd tact
cargo build --release
./target/release/tact-ui --help
```

Via Cargo (coming soon to crates.io):

```bash
cargo install --path crates/tact-ui   # or: cargo install -p tact-ui from the repo root
```

**Binary releases:** push a version tag to publish pre-built binaries for Linux (x86_64 / ARM64), macOS (x86_64 / ARM64), and Windows (x86_64):

```bash
git tag v0.19.0
git push origin v0.19.0
```

GitHub Actions (`.github/workflows/release.yml`) uploads `tact-ui-v<version>-<target-triple>.tar.gz` / `.zip` plus `SHA256SUMS`.

### 2. Configure

Create `tact.toml` in your project root (or `~/.tact/config.toml` for user-level defaults):

```toml
[llm]
provider = "anthropic"   # "anthropic" | "openai" | "deepseek" | "kimi"
model = "claude-sonnet-4-20250514"
api_key = "sk-ant-..."
base_url = "https://api.anthropic.com"  # required for anthropic

[permission]
mode = "default"   # "default" | "plan" | "auto"

[agent]
model_context_window = 200000
snapshot_max_items = 80
micro_compact_enabled = true
notifications_enabled = true

[ui]
theme = "retro"   # retro | brutal | nord | dark | auto ...

[tools]
brave_search_api_key = "bsk-..."   # optional, for web_search
```

CLI flags override the config file (e.g. `--model`, `--api-key`, `--theme`).

Optional agent settings (config file or CLI):

| Setting | CLI flag | Default | Description |
|---|---|---|---|
| `snapshot_max_items` | `--snapshot-max-items` | `80` | Max entries in the system-prompt Project structure snapshot |
| `model_context_window` | `--model-context-window` | `200000` | Model context window in tokens (auto-compact + TUI usage meter) |
| `micro_compact_enabled` | `--no-micro-compact` | `true` | Truncate old tool results in context |

### 3. Run

```bash
# Interactive TUI (default)
tact-ui

# Headless single-shot task
tact-ui headless "Fix all clippy warnings in src/ and run cargo test"

# With specific model
tact-ui headless --model "claude-sonnet-4-20250514" "Refactor the error handling in lib.rs"

# Plan-only mode (review before execution)
tact-ui -m plan headless "Add rate limiting to the API client"
```

---

## Features

### 🧠 Intelligent Agent Loop

Multi-turn conversation loop with built-in context management: auto-compaction when the context window fills up, recovery from interrupted sessions, and persistent memory across conversations.

### 🔧 40+ Built-in Tools

| Category | Tools |
|----------|-------|
| **File System** | `read_file`, `write_file`, `edit_file`, `apply_patch`, `batch_read` |
| **Shell** | `bash`, `background_run`, `check_background`, `sleep` |
| **Code Intelligence** | `search_code` (ripgrep), `lsp` (hover / goto-def / references / diagnostics) |
| **Web** | `web_search`, `web_fetch` |
| **Task Management** | `task`, `task_create`, `task_get`, `task_list`, `task_update` |
| **Team & Sub-agents** | `spawn_teammate`, `list_teammates`, `send_message`, `broadcast`, `read_inbox` |
| **Memory & Knowledge** | `save_memory`, `load_skill`, `compact` |
| **Git & Worktree** | `worktree_create`, `worktree_list`, `worktree_status`, `worktree_run`, `worktree_events` |
| **Scheduling** | `cron_create`, `cron_list`, `cron_delete` |
| **Interaction** | `ask_user`, `plan_approval`, `shutdown_request`, `shutdown_response` |

### 🔐 Three Permission Modes

```
default   →  Ask before every tool call (safe)
plan      →  Plan first, then ask once
auto      →  Auto-approve all actions (CI / trusted repos)
```

### 🪝 Hooks & Skills

- **Pre/Post hooks** — intercept tool calls before/after execution. Run linters, format code, log usage.
- **Skills** — `SKILL.md` playbooks under `~/.tact/skills/` and `.claude/skills/` (summaries in the system prompt; full body via `load_skill` or TUI `/skill-name`).
- **Cron** — schedule recurring prompts. The agent checks in on your project automatically.

### 👥 Sub-agents & Team

Spawn isolated sub-agents for parallel work. Coordinate via message-passing inboxes. Each sub-agent gets a sandboxed toolset (bash + file R/W). Use `plan_approval` / `shutdown_request` protocols for structured handoffs.

### 🌳 Git Worktree Isolation

Each task can run in its own `git worktree` lane. No branch switching, no stash dancing. Agents work in parallel without stepping on each other.

### 🔌 MCP Support

Native [Model Context Protocol](https://modelcontextprotocol.io/) client. Connect any MCP server and its tools become available to the agent at runtime.

### 📡 TUI & Headless

- **TUI mode** (`tact-ui`) — streaming output, syntax-highlighted diffs, interactive permission dialogs
- **Headless mode** (`tact-ui headless`) — CI/CD pipelines, scripts, or non-interactive workflows

### 🖼️ Image attachments (vision)

Attach workspace images with `@path/to.png` or `![alt](path)`. Raster files are optionally compressed via `[ui.vision_image]` before base64 attachment.

**Requires a vision-capable model/endpoint.** OpenAI-compatible providers send images as `image_url` content parts; text-only models or gateways that only accept `text` reject the request (HTTP 400, e.g. `unknown variant image_url, expected text`). Use a multimodal model (e.g. Claude vision, GPT-4o), or omit image attachments on text-only models.

### 💾 Persistent State

Transcripts, tool results, memories, cron jobs, and task state all persist to `~/.tact/` and `<project>/.tact/`. Pick up where you left off.

---

## Architecture

```
┌─────────────────────────────────────────────────┐
│                     tact                        │
│                                                 │
│  ┌─────────┐  ┌──────────┐  ┌───────────────┐  │
│  │  Agent  │  │   Tool   │  │  Permission   │  │
│  │  Loop   │──│  Router  │──│  Manager      │  │
│  └────┬────┘  └────┬─────┘  └───────┬───────┘  │
│       │            │                │           │
│  ┌────┴────┐ ┌─────┴──────┐ ┌──────┴───────┐   │
│  │ Context │ │ MCP Router │ │ Hook Engine  │   │
│  │ Compact │ │  (external) │ │ (pre/post)   │   │
│  └─────────┘ └────────────┘ └──────────────┘   │
│                                                 │
│  ┌─────────────────────────────────────────┐    │
│  │           LLM Client                    │    │
│  │   Anthropic · OpenAI · Compatible       │    │
│  └─────────────────────────────────────────┘    │
│                                                 │
│  ┌─────────┐  ┌──────────┐  ┌───────────────┐  │
│  │ Sub-    │  │ Worktree │  │  Memory /     │  │
│  │ Agents  │  │ Lanes    │  │  Skills       │  │
│  └─────────┘  └──────────┘  └───────────────┘  │
└─────────────────────────────────────────────────┘
```

The agent loop:
1. Builds the system prompt from role, guidelines, constraints, memory, and dynamic context
2. Sends the conversation to the LLM with tool definitions
3. Processes streaming responses: text → display, tool calls → execute
4. Checks permissions for each tool call
5. Runs pre/post hooks on tool execution
6. Writes results back to the conversation history
7. Auto-compacts when context approaches the window limit

See [`ARCHITECTURE.md`](./ARCHITECTURE.md) for a deeper dive.

---

## Comparison

| | **tact** | Claude Code | Cursor | Aider | Open Interpreter |
|---|---|---|---|---|---|
| **Language** | Rust | TypeScript | TypeScript | Python | Python |
| **Interface** | Terminal / TUI | Terminal | Editor (VSCode fork) | Terminal | Terminal |
| **License** | MIT | Proprietary | Proprietary | Apache 2.0 | AGPL |
| **Self-hosted** | ✅ | ✅ | ✅ | ✅ | ✅ |
| **Multi-model** | Anthropic + OpenAI | Anthropic only | Multi | Multi | Multi |
| **Permission system** | 3 modes + hooks | ✅ | ✅ | ✅ | ✅ |
| **Sub-agents** | ✅ (team + inbox) | ✅ | ❌ | ❌ | ❌ |
| **Worktree isolation** | ✅ | ❌ | ❌ | ❌ | ❌ |
| **MCP support** | ✅ (native) | ✅ | ✅ (via extension) | ❌ | ❌ |
| **Cron / scheduled** | ✅ | ❌ | ❌ | ❌ | ❌ |
| **Binary size** | ~15MB | Hundreds MB | Hundreds MB | ~50MB+ | ~200MB+ |
| **Skills system** | ✅ (file-based) | ✅ | ✅ (rules) | ❌ | ❌ |

---

## Built-in Tools

| Tool | Description |
|------|-------------|
| `read_file` | Read file contents with optional offset/limit |
| `write_file` | Write or overwrite a file |
| `edit_file` | Replace exact text in a file (first match, or all with `replace_all`) |
| `apply_patch` | Apply unified diff patches |
| `batch_read` | Read multiple files in parallel |
| `bash` | Run a shell command |
| `background_run` | Run a command in the background |
| `check_background` | Check background task status |
| `search_code` | Search codebase with regex (ripgrep) |
| `lsp` | Query language server (hover, goto-def, references, diagnostics) |
| `web_search` | Search the web |
| `web_fetch` | Fetch and parse a web page |
| `sleep` | Wait for N milliseconds |
| `task` | Spawn a sub-agent with fresh context |
| `task_create` | Create a persistent task |
| `task_get` | Get task details by ID |
| `task_list` | List all tasks with status |
| `task_update` | Update task status, owner, dependencies |
| `spawn_teammate` | Create a named teammate |
| `list_teammates` | List all teammates |
| `send_message` | Send a message to a teammate |
| `broadcast` | Broadcast to all teammates |
| `read_inbox` | Read teammate inbox |
| `plan_approval` | Send a plan approval message |
| `shutdown_request` | Request shutdown |
| `shutdown_response` | Respond to shutdown request |
| `save_memory` | Save persistent memory across sessions |
| `load_skill` | Load a named skill |
| `compact` | Summarize conversation to save context |
| `worktree_create` | Create a git worktree lane |
| `worktree_list` | List tracked worktrees |
| `worktree_status` | Show git status in a worktree |
| `worktree_run` | Run a command inside a worktree |
| `worktree_events` | List worktree lifecycle events |
| `cron_create` | Create a scheduled prompt |
| `cron_list` | List scheduled prompts |
| `cron_delete` | Delete a scheduled prompt |
| `ask_user` | Ask the user (TUI popup; `multi_select` for checkboxes) |

---

## Configuration

tact merges config from two sources (priority: high → low):

```
CLI args  >  tact.toml
```

Use `--config /path/to/config.toml` to point at a specific file instead of auto-discovery.

### Config file locations (auto-discovered)

```
<project>/.tact/config.toml      # project-level
<project>/tact.toml               # project-level (alt)
~/.tact/config.toml               # user-level
```

### Full config reference

```toml
[llm]
provider = "anthropic"           # "anthropic" | "openai" | "deepseek" | "kimi"
model = "claude-sonnet-4-20250514"
api_key = "sk-ant-..."
base_url = "https://..."         # proxy or compatible endpoint
max_tokens = 8000
thinking_budget = 32000

[permission]
mode = "default"                 # "default" | "plan" | "auto"

[agent]
model_context_window = 200000     # tokens; auto-compact + TUI meter
snapshot_max_items = 80
micro_compact_enabled = true
notifications_enabled = true

[ui]
theme = "retro"                  # or "auto"
# vision_image.* only reduces tokens for attached images; does not enable vision
# vision_image.compress = true
# vision_image.max_edge = 1280
# vision_image.jpeg_quality = 80

[tools]
brave_search_api_key = "bsk-..."
```

### CLI flags (override config)

| Flag | Description |
|------|-------------|
| `--config` | Path to config file |
| `--provider` | LLM provider |
| `--model` | Model name |
| `--api-key` | API key |
| `--base-url` | API base URL |
| `--max-tokens` | Max tokens per LLM call |
| `--thinking-budget` | Extended thinking budget |
| `--permission-mode` / `-m` | Permission mode |
| `--theme` | TUI theme |
| `--snapshot-max-items` | Project structure snapshot size |
| `--no-micro-compact` | Disable micro-compaction |
| `--brave-search-api-key` | Brave Search API key |
| `--tokio-console` | Enable tokio-console debugging |

---

## Project Structure

```
crates/
├── protocol/    # Shared wire types (AgentUpdate, UserCommand, …)
├── tact/        # Agent runtime library: loop, tools, hooks, permissions, MCP, LSP
├── tact-ui/     # CLI binary (TUI + headless); wires tact + tui
├── tact_llm/    # LLM provider adapters
├── tui/         # Terminal UI (ratatui)
└── tool_refactor_macros/   # #[tool] proc macro
```

---

## Roadmap

- [ ] Publish to crates.io
- [ ] Pre-built binary releases (GitHub Actions)
- [ ] Web dashboard for task/tool monitoring
- [ ] More MCP transports (SSE, WebSocket)
- [ ] Llama / Ollama support for fully local operation
- [ ] VS Code extension (bridge to TUI)
- [ ] Multi-user team server
- [ ] Plugin marketplace

---

## Contributing

tact is early stage and welcomes contributions! Some good places to start:

- 🐛 **Bug reports** — open an issue
- 💡 **Feature requests** — open a discussion
- 🔧 **PRs** — pick up a `good-first-issue`

Before opening a PR, run `./scripts/check-rust.sh` (or install hooks with `./scripts/install-git-hooks.sh` to run it on push).

See [`ARCHITECTURE.md`](./ARCHITECTURE.md) for an overview of the codebase.

---

## License

MIT — do whatever you want, just keep the copyright notice.

---

<p align="center">
  <sub>Built with 🦀 by <a href="https://github.com/Rg0x80">Rg0x80</a></sub>
</p>
