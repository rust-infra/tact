# Permission Model
> Language: [English](./10_chapter_permission.md) · [中文](./10_chapter_permission_zh.md)

This chapter explains how Tact decides whether each tool call may run: intent classification by risk, three permission modes, an in-session allowlist, and interactive approval through the TUI.

Every native and MCP tool passes through the same gate in Phase 1 of `Agent::execute_tool_call` — after `PreToolUse` hooks and before parallel execution. See [Agent Lifecycle Hooks](./09_chapter_hook.md) for hook ordering.

---

## 1. What the Permission Model Does

`PermissionManager` (`crates/tact/src/permission/mod.rs`) answers one question per tool call:

> Given this tool name and input, should we **allow**, **deny**, or **ask the user**?

It does **not** execute tools. It classifies intent, applies the active mode and allowlist, and returns a `PermissionDecision`. The agent in `crates/tact/src/agent/tool_dispatch.rs` turns that into either scheduling the tool or synthesizing a blocked `ToolResult`.

| Layer | Responsibility |
|-------|------------------|
| `PermissionPolicy::resolve()` | Classify native tool input → `CapabilityRisk` |
| `PermissionManager::check()` | Map risk + mode + settings + allowlist → `PermissionBehavior` |
| `tool_dispatch.rs` | Handle `Ask` via TUI `RequestSelect` or headless `ask_user(risk)` |
| `bash` tool + `shell.rs` | Hard-block a subset of dangerous shell commands at execution time |

Shell commands get **two** defenses: high-risk patterns trigger permission prompts; a smaller set is rejected outright inside the `bash` tool even after approval.

---

## 2. Intent Classification

### Core types

```rust
pub enum CapabilitySource { Native, Mcp }

pub enum CapabilityRisk { Read, Write, High }

pub struct CapabilityIntent {
    pub source: CapabilitySource,
    pub server: Option<String>,  // MCP server segment, if any
    pub tool: String,              // short tool name after parsing
    pub risk: CapabilityRisk,
}
```

`normalize_capability(tool_name, tool_input)` is the single entry point. It parses the tool name, then calls `classify_risk()`.

### Native vs MCP tool names

| Pattern | Example | Parsed result |
|---------|---------|---------------|
| Native | `read_file` | `source = Native`, `tool = "read_file"` |
| MCP | `mcp__demo__db__query` | `source = Mcp`, `server = Some("demo__db")`, `tool = "query"` |

MCP names use the prefix `mcp__`, then `server__tool` with the **rightmost** `__` split (so server IDs may contain underscores).

### Risk rules

Classification is heuristic — based on tool name prefixes and, for `bash`, the command string:

| Risk | Rule |
|------|------|
| **Read** | Tools with `PermissionPolicy::Read` (e.g. `read_file`); shell commands that are provably read-only (see [§7](#7-shell-high-risk-detection)) |
| **Write** | Tools with `PermissionPolicy::Write`; shell tools (`bash` / `background_run` / `worktree_run`) for commands that cannot be proven read-only |
| **High** | Tools with `PermissionPolicy::High` (e.g. `spawn_subagent`); shell commands starting with `sudo ` or `su ` |

A separate hard-block list in `shell.rs` still rejects a subset of dangerous commands at execution time (see [§7](#7-shell-high-risk-detection)), even after permission approval.

MCP tools use their own metadata / defaults; unknown tools are treated as **High** at the dispatch gate.

---

## 3. PermissionBehavior: Allow, Deny, Ask

```rust
pub enum PermissionBehavior {
    Allow,
    Deny,
    Ask,
}

pub struct PermissionDecision {
    pub behavior: PermissionBehavior,
    pub reason: String,
}
```

| Behavior | Meaning in `tool_dispatch.rs` |
|----------|-------------------------------|
| **Allow** | Tool enters Phase 2 (parallel execution) |
| **Deny** | `PreparedState::Resolved` with `"Permission denied: …"`; model receives a failed tool result |
| **Ask** | Interactive prompt (TUI), or headless `ask_user` defaults (allow Write/Read, deny High); see [§6 TUI RequestSelect flow](#6-tui-requestselect-flow) |

---

## 4. Permission Modes

```rust
pub enum PermissionMode {
    Default,
    Plan,
    Auto,
}
```

Display labels (from `PermissionMode`'s `Display` impl):

| Mode | Label | Behavior |
|------|-------|----------|
| `Default` | `default - ask for writes` | Read allowed; Write asks unless settings/allowlist match; High asks unless a settings **allow** rule matches |
| `Plan` | `plan - read only` | Read allowed (including provably read-only shell commands — `ls`, `grep`, `git status`, …); Write and High **denied** without prompting |
| `Auto` | `auto - allow non-high operations` | All risks auto-approved (including High) |

### Decision order in `PermissionManager::check()`

The checks run in this fixed order:

```text
1. Read risk?                         → Allow (all modes)
2. Plan mode + non-Read?              → Deny
3. Auto mode?                         → Allow (all risks)
4. Settings deny rule?                → Deny
5. Settings allow rule?               → Allow (including High)
6. Settings ask rule (non-High)?      → Ask
7. High risk (no Deny/Allow rule)?    → Ask (skips in-session allowlist)
8. In-session always_allowed match?   → Allow
9. Default                            → Ask
```

```mermaid
flowchart TD
    TC["ToolUse { name, input }"] --> Risk["PermissionPolicy::resolve()"]
    Risk -- Read --> Allow["Allow"]
    Risk -- Write / High --> Plan{"Plan mode?"}

    Plan -- Yes --> Deny["Deny"]
    Plan -- No --> Auto{"Auto mode?"}

    Auto -- Yes --> Allow
    Auto -- No --> Settings{"Settings rule?"}

    Settings -- Deny --> Deny
    Settings -- Allow --> Allow
    Settings -- Ask / none --> High{"High risk?"}

    High -- Yes --> Ask["Ask user"]
    High -- No --> AllowList{"always_allowed_tools?"}

    AllowList -- Yes --> Allow
    AllowList -- No --> Ask
```

**High-risk vs allowlists:** the in-session bare-name allowlist (`allow_tool`) does **not** bypass High — those calls still `Ask`. A matching project settings **allow** rule (including rules persisted by "Always allow this tool" via `allow_tool_with_input`) **does** allow High for that input pattern.

---

## 5. Allowlist and Consecutive Denials

### In-session allowlist

`PermissionManager` holds `always_allowed_tools: Vec<String>`. On construction (`try_new`), it is seeded with `"read_file"`.

When the user picks **"Always allow this tool"** in the TUI, `allow_tool(tool_name)` appends the exact tool name (e.g. `edit_file`, `bash`). Future **Write**-risk calls to that name skip the Default-mode prompt.

The allowlist is **in-memory only** — it is not persisted to SQLite or TOML between sessions.

### Consecutive denials

Each user **Deny** increments `consecutive_denials`. Allow-once and always-allow reset it to zero.

After `max_consecutive_denials` (default **3**) denials, `should_suggest_plan_mode()` returns true. In non-interactive mode, `ask_user()` prints a hint to stderr:

```text
[3 consecutive denials -- consider switching to plan mode]
```

There is no automatic mode switch today — the message is advisory only.

---

## 6. TUI RequestSelect Flow

When `check()` returns `Ask` and the agent has a UI channel (`runtime.ui_tx`), `tool_dispatch.rs` sends:

```rust
AgentUpdate::RequestSelect {
    prompt,      // e.g. "Allow bash: {\"command\":\"npm test\"}"
    options,     // ["Allow once", "Deny", "Always allow this tool"]
    respond,     // oneshot channel back to the agent
}
```

The TUI (`crates/tui/src/widgets/state/app/agent.rs`) switches to `InputMode::Select` and renders the select popup (`log_confirm = false` so the choice does not clutter the log).

| User choice | Index | Agent action |
|-------------|-------|--------------|
| Allow once | 0 | Run tool; set `permission_label = "Allow once"` on `StepFinished` |
| Deny | 1 (default) | `PreparedState::Resolved`; `StepFailed` with deny message |
| Always allow this tool | 2 | `allow_tool(name)`; run tool; `permission_label = "Always allow this tool"` |

The `permission_label` is attached to `StepResult` and shown on the tool meta row in the TUI. See [Tool Rendering](../docs/tool_rendering.md).

### Headless / no UI channel

If `ui_tx` is absent, the agent calls `permission_manager.ask_user(tool, risk)`:

| Risk | Non-interactive default | stderr |
|------|-------------------------|--------|
| **High** | Deny | `[permission] non-interactive: denying high-risk <tool>` |
| **Write** / **Read** | Allow once | `[permission] non-interactive: allowing <tool>` |

Use `--auto` (Auto mode) when unattended runs must also approve High-risk tools without a TUI. Settings allow/deny rules still apply before `ask_user` is reached.

---

## 7. Shell High-Risk Detection

Shared logic lives in `crates/tact/src/shell.rs`:

```rust
pub fn is_high_risk_shell_command(command: &str) -> bool;
pub fn validate_shell_command(command: &str) -> Result<()>;
```

`is_high_risk_shell_command` lowercases the command and checks for blocked substrings:

| Pattern | Effect |
|---------|--------|
| `sudo`, `shutdown`, `reboot` | High risk |
| `> /dev/`, `>> /dev/` | High risk |
| `rm -rf /`, `rm -fr /`, `rm -rf /*`, … | High risk |
| `rm -rf ~`, `rm -fr $home`, … | High risk |

### Read-only shell command classification

Since 2026-08-13, `PermissionPolicy::ShellCommand` classifies a shell command string as **Read** when — and only when — it is provably read-only. The logic lives in `crates/tact/src/tool/readonly_shell.rs` and runs in two stages:

1. **Plain-command split** — the string must be whitespace-separated words (bare words or single/double-quoted segments) with no shell metacharacters: `; & | > < $ backtick \`, globs, braces, parentheses, `!`. Redirections, pipes, command substitution and escapes are rejected outright, so the classification cannot disagree with what `sh -c` actually runs. A leading `~` and embedded single-quoted segments are accepted (both are literal).
2. **Safelist match** — the first word must be a program whose options alone cannot write:
   - Always safe: `cat cd cut echo expr false grep head id ls nl paste pwd rev seq stat tail tr true uname uniq wc which whoami`
   - `base64` — except `-o` / `--output`; `find` — except `-exec -execdir -ok -okdir -delete -fls -fprint -fprint0 -fprintf`; `rg` — except `--pre --hostname-bin --search-zip -z`
   - `git` — only `status / log / diff / show / branch`, with unsafe global options (`-C -c --git-dir --paginate` and friends) and output/exec options (`--output --ext-diff --textconv --exec`) rejected; `git branch` additionally rejects any argument that could create, rename, or delete a branch
   - `sed` — only `sed -n {N|M,N}p` print-line-range forms

The safelist and option rules mirror OpenAI Codex's `is_known_safe_command` (`codex-rs/shell-command/src/command_safety/is_safe_command.rs`). The classifier is deliberately conservative: a false negative only costs an approval prompt, while a false positive would run a mutation silently under plan mode — so anything ambiguous stays **Write**. Net effect: in plan mode `ls`, `grep -rn x .`, `git status` run without prompting; `cargo test`, pipes, redirections, and unknown programs are still denied.

### Two layers

```mermaid
sequenceDiagram
    participant Agent
    participant Perm as PermissionManager
    participant TUI
    participant Bash as bash tool

    Agent->>Perm: check("bash", {command})
    alt High risk (e.g. sudo)
        Perm-->>Agent: Ask
        Agent->>TUI: RequestSelect
        TUI-->>Agent: Allow once
    else Write risk (e.g. npm test)
        Perm-->>Agent: Ask or Allow (mode/allowlist)
    end

    Agent->>Bash: call(command)
    alt validate_shell_command fails
        Bash-->>Agent: Error: Dangerous command blocked
    else OK
        Bash-->>Agent: stdout/stderr
    end
```

1. **Permission layer** — `classify_risk` uses `is_high_risk_shell_command` to mark High risk → always `Ask` (except Read-only bash).
2. **Execution layer** — `bash` and `background_run` call `validate_shell_command` before spawning. A blocked command fails even if the user approved it.

Benign destructive paths are allowed at execution but may still prompt: e.g. `rm -rf ./build` passes `validate_shell_command` but is classified as **Write**, so Default mode asks first.

Read-only bash detection rejects commands with shell metacharacters — `ls; rm -rf /` is **Write**, not Read.

---

## 8. Integration in the Tool Pipeline

Permissions run in **Phase 1** of `execute_tool_call` (`crates/tact/src/agent/tool_dispatch.rs`), strictly after hooks:

```text
For each ToolUse (sequential):
  stats · cancel check
  StepAdded / StepStarted
  PreToolUse hooks          ← can mutate input or Block
  PermissionManager::check  ← this chapter
  Ask → RequestSelect (if needed)
  PreparedState::Run | Resolved

Phase 2: parallel waves (no permission re-check)
Phase 3: build ToolResult blocks in model order
```

```mermaid
sequenceDiagram
    autonumber
    participant LLM
    participant Agent
    participant Hook as PreToolUse
    participant Perm as PermissionManager
    participant TUI
    participant Tool as ToolRouter / MCP

    LLM->>Agent: ToolUse blocks
    Agent->>Hook: invoke_hooks!(PreToolUse)
    alt HookControl::Block
        Hook-->>Agent: blocked message
    else Continue
        Agent->>Perm: check(name, input)
        alt Allow
            Perm-->>Agent: Allow
            Agent->>Tool: execute (Phase 2)
        else Deny
            Perm-->>Agent: Deny
            Agent-->>LLM: ToolResult (permission denied)
        else Ask
            Perm-->>Agent: Ask
            Agent->>TUI: RequestSelect
            TUI-->>Agent: user choice
            opt approved
                Agent->>Tool: execute
            end
        end
    end
```

`PermissionManager` lives on `AgentRuntime` (`crates/tact/src/agent/mod.rs`), not on `ToolContext`. Sub-agents created by the `spawn_subagent` tool get their own manager (always `PermissionMode::Default`) but inherit the main agent's `ui_tx` so permission popups still work.

---

## 9. Configuration

### TOML

```toml
[permission]
mode = "default"   # "default" | "plan" | "auto"
```

Defined in `PermissionTomlConfig` (`crates/tact/src/config/types.rs`). Default when omitted: `"default"`.

### CLI

`--permission-mode` / `-m` overrides TOML via `config/resolve.rs` → `ResolvedConfig.permission_mode`.

### Startup behavior today

| Entry point | Mode used |
|-------------|-----------|
| `tact-ui headless` | `permission_mode_from_config()` — reads TOML / CLI; unknown values fall through to **Auto** |
| `tact-ui` (interactive TUI) | Same as headless — `permission_mode_from_config()` |

---

## 10. Code Map

| File | Role |
|------|------|
| `crates/tact/src/permission/mod.rs` | `CapabilityRisk`, `PermissionManager`, `normalize_capability`, classification heuristics |
| `crates/tact/src/shell.rs` | Shared high-risk shell patterns; `validate_shell_command` for execution-time block |
| `crates/tact/src/agent/tool_dispatch.rs` | Pre-flight permission check; `RequestSelect` handling; `permission_label` on `StepFinished` |
| `crates/tact/src/agent/mod.rs` | `AgentRuntime.permission_manager` |
| `crates/tact/src/tool/bash.rs` | Calls `validate_shell_command` before spawning shell |
| `crates/tact/src/background.rs` | Same validation for background shell commands |
| `crates/tact/src/tool/subagent.rs` | Sub-agent uses `Default` mode; inherits `ui_tx` |
| `crates/tact-ui/src/permission.rs` | `permission_mode_from_config()` |
| `crates/tact-ui/src/headless.rs`, `interactive.rs` | Construct `PermissionManager` at session start |
| `crates/tact/src/config/types.rs` | `[permission] mode` TOML schema |
| `crates/tui/src/widgets/state/app/agent.rs` | Handles `AgentUpdate::RequestSelect` |
| `crates/protocol/src/lib.rs` | `AgentUpdate::RequestSelect`, `StepResult.permission_label` |

---

## 11. Current Gaps

| Gap | Detail |
|-----|--------|
| Allowlist not persisted | "Always allow this tool" lasts only for the current process |
| No runtime mode switch API | User must restart with a different mode; stderr only suggests Plan after repeated denials |
| Headless High still needs Auto or settings allow | Non-interactive `ask_user` allows Write/Read Ask, but denies High unless a settings allow rule already returned Allow |
| `PlanStep.need_approval` deprecated | Field marked `#[deprecated(since = "0.19.0")]`; use `PlanStep::new()` — permission is driven by `PermissionManager` |
| Permission vs hook overlap | Both can block tools; hooks run first and skip permission on `Block` |

---

## Related Docs

- [Tasks and Tool Scheduling](./11_chapter_task.md) — three-phase pipeline permissions sit inside
- [Subagents](./12_chapter_subagent.md) — `spawn_subagent` High risk, separate `PermissionManager`, inherited `ui_tx`
- [Agent Lifecycle Hooks](./09_chapter_hook.md) — PreToolUse runs immediately before permission check
- [ARCHITECTURE.md](../ARCHITECTURE.md#3-permission-system) — architecture diagram and mode table
- [docs/state_machines.md](../docs/state_machines.md) — permission decision state machine
- [docs/tool_rendering.md](../docs/tool_rendering.md) — how `permission_label` appears in the TUI
- [docs/parallel_tool_execution.md](../docs/parallel_tool_execution.md) — why pre-flight stays sequential
