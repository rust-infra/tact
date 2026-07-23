# System Prompt in Tact
> Language: [English](./04_chapter_prompt.md) · [中文](./04_chapter_prompt_zh.md)

This chapter explains how Tact builds the **system prompt**—the initial instruction given to the LLM before any user message—and how its structure is designed to stay cache-friendly while still reflecting fresh memory and project context every turn.

---

## 1. Why a Structured System Prompt Matters

The system prompt is not a single string of rules. It is an assembly of several information sources:

| Section | Description |
|---------|-------------|
| **Role** | Who the agent is (e.g., "You are a coding agent operating in /path/to/project"). |
| **Skills** | What capabilities are available right now. |
| **Guidelines** | Soft best practices for completing the task well (e.g., understand the goal before acting). |
| **Constraints** | Hard operational limits the agent must follow (e.g., use tools instead of guessing, when to stop). |
| **CLAUDE.md** / **AGENTS.md** | Optional project instruction files from the workspace (default: `AGENTS.md` only; see `[agent].instruction_sources`). |
| **Memory** | Persistent facts learned from previous conversations. |
| **Dynamic context** | Freshly computed project snapshot (file tree, recent changes, etc.). |

If all of this is thrown at the model as one noisy paragraph, the LLM has a harder time following the important rules. A predictable, sectioned layout makes behavior more consistent across models (Claude, OpenAI, Kimi, DeepSeek, …).

---

## 2. Template Layout

The template lives in:

```text
crates/tact/src/prompt/system_prompt_template.md
```

It is rendered by [Tera](https://keats.github.io/tera/) from the data assembled in `crates/tact/src/prompt/mod.rs`.

The final output follows this order:

```markdown
# Your role
...

# Available skills
...

# Guidelines you need to follow
- ...

# Constraints that must be adhered to
- ...

# Memory guidance
...

# Additional context

<claude_md>    {# optional #}
<additional>   {# AGENTS.md #}

=== DYNAMIC_BOUNDARY ===

## Memory
...

## Dynamic context
...
```


| Section                      | Source                                                    | Stability          |
| ---------------------------- | --------------------------------------------------------- | ------------------ |
| `role`                       | hard-coded agent identity                                 | static             |
| `skills_available`           | skill registry                                            | mostly static      |
| `claude_md`                  | `CLAUDE.md` (optional; under `# Additional context`)      | static per session |
| `guidelines` / `constraints` | agent defaults                                            | static             |
| `memory_guidance`            | constant prompt text                                      | static             |
| `additional`                 | `AGENTS.md` (default; under `# Additional context`)       | static per session |
| `memory`                     | `MemoryManager`                                           | dynamic            |
| `dynamic_context`            | directory snapshot / recent files                         | dynamic            |


Sections above `=== DYNAMIC_BOUNDARY ===` change rarely. Sections below it may change every turn.

---

## 3. Code Map

### 3.1 Template and rendering

- `crates/tact/src/prompt/system_prompt_template.md` — the Tera template.
- `crates/tact/src/prompt/mod.rs` — `SystemPrompt` builder, `Prompt` wrapper, and `render()`.

`SystemPrompt` is a builder:

```rust
let prompt = SystemPrompt::builder()
    .role("You are a coding agent operating in /home/xxxx/Projects/tact.")
    .skills_available("- rust-skills: Comprehensive Rust coding guidelines")
    .add_guideline("Think before you act")
    .add_constraint("Never expose secrets")
    .memory("User prefers Rust over Python")
    .dynamic_context("Current file tree ...")
    .build()?;

let rendered = prompt.to_prompt().render()?;
```

### 3.2 Wiring into the agent

The actual content is assembled in `Agent::build_system_prompt` (`crates/tact/src/agent/mod.rs`):

```rust
let prompt = SystemPrompt::builder()
    .role(format!("You are a coding agent operating in {}.", workdir.display()))
    .guidelines([...])
    .constraints([...])
    .skills_available(self.tool_context.skill_registry.describe_available())
    .memory(self.load_memory_prompt()?)
    .claude_md(cached_md_section(&mut cached_claude_md, || assemble_claude_md_prompt(workdir, &instruction_sources)))
    .additional(cached_md_section(&mut cached_agents_md, || assemble_agents_md_prompt(workdir, &instruction_sources)))
    .dynamic_context(load_dynamic_context(workdir, &mut self.runtime.cached_dir_snapshot))
    .memory_guidance(MEMORY_GUIDANCE.trim())
    .build()?;
```

`build_system_prompt()` is called **once per task**, at the top of `agent_loop` before the turn loop starts. The same rendered string is reused for every LLM request within that task, keeping the prompt byte-stable across turns for prefix KV-caching. `memory` and `dynamic_context` are re-evaluated at the start of the next task; enabled instruction files (`AGENTS.md` / `CLAUDE.md`) and the directory snapshot are assembled **once per session** and cached.

### 3.3 Instruction file sources (`instruction_sources`)

By default only **`AGENTS.md`** is injected. `CLAUDE.md` is opt-in via `tact.toml`. Both render inside the same `# Additional context` section (CLAUDE block first when enabled, then AGENTS).

```toml
[agent]
# Default — AGENTS.md only
instruction_sources = ["agents_md"]

# Claude Code style — both file families
instruction_sources = ["agents_md", "claude_md"]

# Fine-grained CLAUDE paths
instruction_sources = ["agents_md", "claude_md_user", "claude_md_project"]
```


| Key                 | Files                                             |
| ------------------- | ------------------------------------------------- |
| `agents_md`         | `<workdir>/AGENTS.md`, optional `<cwd>/AGENTS.md` |
| `claude_md`         | All three CLAUDE paths below                      |
| `claude_md_user`    | `~/.claude/CLAUDE.md`                             |
| `claude_md_project` | `<workdir>/CLAUDE.md`                             |
| `claude_md_subdir`  | `<cwd>/CLAUDE.md` when cwd ≠ workdir              |


---

## 4. Static vs. Dynamic Modes

`AgentSystemPrompt` has two variants:


| Variant   | Behavior                                          | Use case                                           |
| --------- | ------------------------------------------------- | -------------------------------------------------- |
| `Static`  | Returns the same string every time                | Tests, demos, or when you want full manual control |
| `Dynamic` | Re-renders the template at the start of each task | Normal operation; keeps context and memory fresh   |


In normal Tact usage (`tact-ui` / headless), the agent starts in `Dynamic` mode.

---

## 5. The Dynamic Boundary and KV-Cache

LLM providers can **cache** the prefix of a long prompt across calls. The stable sections at the top of the system prompt (role, guidelines, constraints, CLAUDE.md) are perfect for caching.

The line:

```text
=== DYNAMIC_BOUNDARY ===
```

is a visual marker inside the system prompt. Everything above it should stay identical across tasks so the provider can reuse the cached prefix. Everything below it (`memory`, `dynamic_context`) is allowed to change.

Because `build_system_prompt()` runs at each task start, only the suffix from the boundary onward needs to be re-evaluated. If the provider supports prompt caching, you get fresh context without paying full cost for the entire instruction block.

---

## 6. Customizing the System Prompt

### 6.1 Adding guidelines or constraints

Use the builder methods in `Agent::build_system_prompt`:

```rust
.constraints([
    "Always run cargo check after editing Rust files",
    "Prefer edit_file over write_file for small changes",
])
```

### 6.2 Injecting extra markdown (`AGENTS.md`)

`Agent::build_system_prompt` loads project `AGENTS.md` (and a cwd-relative copy when cwd ≠ workdir) into the template `additional` slot on the **first** render of a session, then reuses the cached string for later turns.

You can also append arbitrary markdown via the builder:

```rust
.additional("# Project conventions\n\nUse anyhow::Result everywhere.")
```

### 6.3 Replacing the entire template

Pass a custom template string when constructing `SystemPrompt`:

```rust
SystemPrompt::from(include_str!("my_template.md"))
    .role("...")
    .build()?
```

Keep the `=== DYNAMIC_BOUNDARY ===` convention if you want to benefit from prefix caching.

---

## 7. Example Output

A real session snapshot (default `instruction_sources = ["agents_md"]`, project `AGENTS.md` present, five skills discovered, several `[feedback]` memories):

- **role** — coding agent in `/Users/rg/Projects/tact`
- **skills_available** — five skill summaries + slash / `load_skill` note
- **guidelines** / **constraints** — tact built-in defaults
- **memory_guidance** — when to call `save_memory`
- **additional** — project `AGENTS.md` (rendered under `# Additional context`)
- **memory** — persistent `.tact/memory/*.md` content
- **dynamic_context** — date, workdir, model, platform, directory snapshot

The rendered prompt looks like:

```markdown
# Your role

You are a coding agent operating in /Users/rg/Projects/tact.

# Available skills

- code-reviewer: Code review specialist
- demo-test: Test skill loading
- echo-args: Test skill for slash arguments — echoes $ARGUMENTS back and replies briefly
- english-tutor: English tutoring assistant
- shell-master: Shell command specialist

When a user message already contains a `<skill name="…">…</skill>` block, the user slash-invoked that skill — follow those instructions directly and do not call `load_skill` for the same skill. If the block includes an `ARGUMENTS:` line (Claude Code convention when the skill has no `$ARGUMENTS` placeholder), that line is the user's slash-command arguments for this invocation; apply the skill to fulfill them.

# Guidelines you need to follow

- Try to understand how to complete the task well before completing it.

# Constraints that must be adhered to

- Think step by step
- Think before you act; respond with your thoughts before calling tools
- Do not make up any assumptions, use tools to get the information you need
- Use the provided tools to interact with the system and accomplish the task
- If you are stuck, or otherwise cannot complete the task, respond with your thoughts and stop
- If the task is completed, or otherwise cannot continue, like requiring user feedback, stop.
- When editing files, always re-read the file first if its content may have changed since you last read it
- For multi-line changes, prefer apply_patch; for exact string replacements, use edit_file (replace_all=true to change every occurrence in the file)
- If a tool result was truncated and you need the details, re-run the relevant tool (e.g., read_file)
- For small edits to existing files, prefer edit_file over write_file; use write_file only for new files or complete rewrites

# Memory guidance

When to save memories:
- User states a preference ("I like tabs", "always use pytest") -> type: user
- User corrects you ("don't do X", "that was wrong because...") -> type: feedback
- You learn a project fact that is not easy to infer from current code alone
  (for example: a rule exists because of compliance, or a legacy module must
  stay untouched for business reasons) -> type: project
- You learn where an external resource lives (ticket board, dashboard, docs URL)
  -> type: reference

When NOT to save:
- Anything easily derivable from code (function signatures, file structure, directory layout)
- Temporary task state (current branch, open PR numbers, current TODOs)
- Secrets or credentials (API keys, passwords)

# Additional context

## AGENTS.md instructions

### From project root (AGENTS.md)

# Tact agent notes

- Run tests with `cargo test -p tact` / `cargo test -p tui` for focused crates.
- Prefer `edit_file` for small changes; use `apply_patch` for multi-line hunks.
- Book chapters live under `book/`; keep `04_chapter_prompt.md` aligned with `system_prompt_template.md`.
- User-facing docs and examples are English unless the task asks otherwise.

=== DYNAMIC_BOUNDARY ===

## Memory

# Memories (persistent across sessions)

## [feedback]
### batch_edit_min_files: batch_edit should only be used when editing 3+ distinct files
batch_edit should only be used when the edits span 3 or more distinct files. For edits touching fewer than 3 different files, prefer individual edit_file calls instead. This avoids the overhead of batch validation when simple single-file edits suffice.

This rule is also documented in `docs/agent_guidelines.md`.

### edit_file_lazy_diff_feedback: edit_file diff preview should be opt-in and lazy-loaded
User feedback: `edit_file` should support a `show_diff` (or similar) parameter so the user can opt in to a diff preview card. The card should lazy-load: show a clickable placeholder first; on click, run `git diff` and open/expand the result, instead of running diff automatically on every edit.

### edit_file_lazy_diff_feedback_v2: edit_file diff preview uses new_text, lazy-loads git diff on click
User feedback: `edit_file` should support `show_diff` (or similar) for optional diff preview. Suggested UX: preview the card with `new_text` (already in args, no extra cost); on click, open/expand and run `git diff` for the full diff, avoiding automatic diff on every edit.

### log_selection_should_be_character_level: Log text selection should be character-level, not line-only
Log panel selection is currently line-based: `log_selection` stores `(start_line, end_line)`, highlights whole lines, and copies full lines. That prevents selecting part of a line (e.g. mid-line to mid-line on another row) and feels awkward when drag-selecting while scrolling. User wants character/column-level selection.

### tool_card_double_click_detail_area_only: Tool card double-click popup should be limited to the detail card area
Tool Card double-click to open a popup should only apply inside the detail card (bordered preview: title, preview body, bottom hint). Clicks on the header status/title rows (the two header lines) should not open the popup.

## Dynamic context

Current date: 2026-07-15
Working directory: /Users/rg/Projects/tact
Model: deepseek-v4-pro
Platform: macos

## Project structure

  book/
  crates/
  docs/
  scripts/
  skills/
book
  output/
  prompts/
  scripts/
  templates/
book/output
  chm/
  mcp/
book/output/chm
  html/
book/scripts
  chm/
  lib/
crates
  protocol/
  tact-ui/
  tact/
  tact_llm/
  tool_refactor_macros/
  tui/
crates/protocol
  src/
crates/tact
  src/
crates/tact-ui
  src/
  tests/
crates/tact-ui/tests
  harness/
crates/tact/src
  agent/
  config/
  cron/
  hook/
  lsp/
  mcp/
  memory/
  notifications/
  permission/
  prompt/
  skill/
  store/
  task/
  tool/
  worktree/
crates/tact_llm
  src/
crates/tool_refactor_macros
  src/
crates/tui
  src/
crates/tui/src
  handlers/
  render/
  widgets/
docs
  superpowers/
docs/superpowers
  plans/
  specs/
skills
  demo-test/
```

This is then attached to the LLM request via `CreateMessageParams::with_system(&system)`, followed by the conversation history.

---

## 8. Testing

The prompt module has unit tests covering:

- all sections render when populated,
- empty sections are omitted,
- static sections appear before `=== DYNAMIC_BOUNDARY ===`,
- no XML tags are emitted for dynamic context.

Run them with:

```bash
cargo test -p tact prompt
```

---

## Related Files

- Template: `[crates/tact/src/prompt/system_prompt_template.md](../crates/tact/src/prompt/system_prompt_template.md)`
- Builder / render logic: `[crates/tact/src/prompt/mod.rs](../crates/tact/src/prompt/mod.rs)`
- Agent wiring: `[crates/tact/src/agent/mod.rs](../crates/tact/src/agent/mod.rs)` (`Agent::build_system_prompt`)
- Memory manager: `[crates/tact/src/memory/mod.rs](../crates/tact/src/memory/mod.rs)` — see [Persistent Memory](./03_chapter_memory.md)
- Skill registry: `[crates/tact/src/skill/mod.rs](../crates/tact/src/skill/mod.rs)` — see [Skill Registry](./02_chapter_skill.md)
- Dynamic context loader: `[crates/tact/src/agent/mod.rs](../crates/tact/src/agent/mod.rs)` (`fn load_dynamic_context`)

