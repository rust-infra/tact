# Design: `mcp_tool` hook handlers

Date: 2026-09-30
Status: approved for implementation

Closes one of the three items in [Ch 9 §13](../../../book/09_chapter_hook_zh.md): *"`mcp_tool` hook
handlers | Codex hooks can invoke an MCP tool; Tact runs commands only."*

## 1. Problem

A hook entry declares a `type`, and Tact ignores it: the presence of a `command` string decides
everything. So an entry written the other way — `{"type": "mcp_tool", "server": …, "tool": …}` — is
silently inert. `server`, `tool` and `arguments` are not even fields on `HookCommand`, so serde drops
them without a word, and the entry is admitted, identity-hashed, reviewed by the user, registered…
and then does nothing at every event, because `run_command_hook` returns `continue_default()` when
there is no `command`.

That is the worst version of a gap: the review flow built for hooks asks a human to approve a
definition that, once approved, cannot run.

The feature is worth having on its own terms, too. Codex's point in allowing it is that a policy can
live in an MCP server — the same one that already holds the integration's tools — instead of in a
shell script that has to re-implement the hook payload, the decision contract, and `additionalContext`
parsing in whatever language it is written in.

## 2. Design

### 2.1 The enabler

Hook closures are registered as `Fn(&crate::Agent, …)`, and `MCPToolRouter::call(&self, …)` takes a
shared reference — so an `mcp_tool` hook can reach `agent.mcp_router` without new plumbing. This is why
the handler is cheap in Tact and not in a harness that only exposes the router behind `&mut`.

### 2.2 One entry point, two kinds

`HookCommand::kind()` resolves the entry, and `run_hook` dispatches:

- `type: "mcp_tool"` (or `"mcpTool"`, since Codex's config vocabulary is camelCase elsewhere) plus a
  `server` and a `tool` → `run_mcp_tool_hook`.
- Everything else → `run_command_hook`, unchanged. Deliberately permissive about `type`: today an
  unknown or absent value means "command with a `command` string", and making it an error would break
  a working configuration to fix a typo.
- `mcp_tool` **missing** `server` or `tool` → an `Invalid` reason, reported the way a failed hook is
  reported. A declaration that cannot run must not be indistinguishable from one that ran and had
  nothing to say — which is exactly the failure being fixed.

### 2.3 The call, and what comes back

`run_mcp_tool_hook` builds the namespaced name, calls the router with the entry's `arguments` (`{}`
when absent), and wraps the call in the hook's own `timeout` — a hook's budget is the hook's, not the
server's `tool_timeout_sec`.

The result is normalized through **the same** `parse_output` a command hook's stdout goes through, so
a tool that returns `{"decision": "block", "reason": …}` blocks, one that returns
`{"hookSpecificOutput": {"additionalContext": …}}` injects context, and `additionalContextLimit`
bounds it. That is the whole point of routing it here rather than inventing a second contract: one
hook output contract, two ways to produce it.

The post-processing both kinds share — the per-hook context budget and reporting `systemMessage` —
moves into `finish_hook_output`, so the two paths cannot drift.

### 2.4 Naming

`McpToolName` could parse `mcp__<server>__<tool>` but nothing could build it; `build_tool_specs` held
the only `format!`. A second literal in the hook path would be a routing key duplicated, so a free
`mcp_tool_name(server, tool)` becomes the one place that spells it, and `build_tool_specs` uses it.

### 2.5 What a failure looks like

An unreachable server, an unknown tool, or a tool error is reported as `[plugin hook <event> failed]
<reason>` and **continues**, matching every other hook failure: a hook must not be able to halt the
loop by being broken. A tool that returns a decision is honoured exactly as a command hook's would be.

## 3. Non-goals

- **Inline `[hooks]` tables in `config.toml`.** Ch 9 records this as a *deliberate* gap: Tact has one
  file entry point, because that is what `bm hook install`-style tooling writes, and a second spelling
  needs its own precedence rules. Adding it reopens that decision rather than filling a hole.
- **Managed / enterprise hooks and `bypass_trust`.** Also recorded as deliberate, and it is a
  trust-model change: `bypass_trust` is a switch that disables the review gate this subsystem exists
  to enforce. `tact-ui hooks trust --all` is the scriptable equivalent. Not to be reversed as a side
  effect of adding a handler type.
- MCP **prompts** as a hook source, and letting a hook *return* the next tool call.

## 4. Tests

- An `mcp_tool` entry reaches the server's tool and its `arguments` arrive intact.
- A tool returning a JSON decision blocks (or injects context) exactly as a command hook's stdout does.
- `additionalContextLimit` bounds an `mcp_tool` hook's injected context, like a command hook's.
- A `mcp_tool` entry missing `server` or `tool` is reported as invalid rather than silently inert.
- A failing tool call is reported and continues.
- An entry with an unknown or absent `type` still runs as a command (no regression).
- `mcp_tool_name` round-trips through `McpToolName`.

## 5. Docs sync

- `book/09_chapter_hook_zh.md`: the "Command hooks" section gains the second kind, the code
  map gains `run_hook` / `run_mcp_tool_hook`, and the §13 deliberate-gaps table loses the `mcp_tool`
  row (keeping the other two, with their reasons).
- `book/26_chapter_issue_zh.md`: newest-first entry.
