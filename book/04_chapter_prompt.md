# System Prompt in Tact

This chapter explains how Tact builds the **system prompt**—the initial instruction given to the LLM before any user message—and how its structure is designed to stay cache-friendly while still reflecting fresh memory and project context every turn.

---

## 1. Why a Structured System Prompt Matters

The system prompt is not a single string of rules. It is an assembly of several information sources:

- **Role**: who the agent is (e.g., "You are a coding agent operating in /path/to/project").
- **Skills**: what capabilities are available right now.
- **Guidelines / Constraints**: soft best practices and hard limits.
- **CLAUDE.md**: project-specific instructions loaded from the workspace.
- **Memory**: persistent facts learned from previous conversations.
- **Dynamic context**: freshly computed project snapshot (file tree, recent changes, etc.).

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

<claude_md>

# Guidelines you need to follow
- ...

# Constraints that must be adhered to
- ...

# Memory guidance
...

# Additional context
...

=== DYNAMIC_BOUNDARY ===

## Memory
...

## Dynamic context
...
```

| Section | Source | Stability |
|---------|--------|-----------|
| `role` | hard-coded agent identity | static |
| `skills_available` | skill registry | mostly static |
| `claude_md` | `CLAUDE.md` in workspace | static per project |
| `guidelines` / `constraints` | agent defaults | static |
| `memory_guidance` | constant prompt text | static |
| `additional` | optional extra markdown | varies |
| `memory` | `MemoryManager` | dynamic |
| `dynamic_context` | directory snapshot / recent files | dynamic |

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
    .skills_available("- bash\n- read_file")
    .add_guideline("Think before you act")
    .add_constraint("Never expose secrets")
    .memory("User prefers Rust over Python")
    .dynamic_context("Current file tree ...")
    .build()?;

let rendered = prompt.to_prompt().render()?;
```

### 3.2 Wiring into the agent

The actual content is assembled in `Agent::build_system_prompt` (`crates/tact/src/lib.rs`):

```rust
let prompt = SystemPrompt::builder()
    .role(format!("You are a coding agent operating in {}.", workdir.display()))
    .guidelines([...])
    .constraints([...])
    .skills_available(self.tool_context.skill_registry.describe_available())
    .memory(self.load_memory_prompt()?)
    .claude_md(load_claude_md_prompt(workdir))
    .dynamic_context(load_dynamic_context(workdir, &mut self.runtime.cached_dir_snapshot))
    .memory_guidance(MEMORY_GUIDANCE.trim())
    .build()?;
```

`build_system_prompt()` is called **inside the agent loop**, right before each LLM request. That means `memory` and `dynamic_context` are refreshed every turn.

---

## 4. Static vs. Dynamic Modes

`AgentSystemPrompt` has two variants:

| Variant | Behavior | Use case |
|---------|----------|----------|
| `Static` | Returns the same string every time | Tests, demos, or when you want full manual control |
| `Dynamic` | Re-renders the template each turn | Normal operation; keeps context and memory fresh |

In normal Tact usage (`tact-ui` / headless), the agent starts in `Dynamic` mode.

---

## 5. The Dynamic Boundary and KV-Cache

LLM providers can **cache** the prefix of a long prompt across calls. The stable sections at the top of the system prompt (role, guidelines, constraints, CLAUDE.md) are perfect for caching.

The line:

```text
=== DYNAMIC_BOUNDARY ===
```

is a visual marker inside the system prompt. Everything above it should stay identical across turns so the provider can reuse the cached prefix. Everything below it (`memory`, `dynamic_context`) is allowed to change.

Because `build_system_prompt()` runs every turn, only the suffix from the boundary onward needs to be re-evaluated. If the provider supports prompt caching, you get fresh context without paying full cost for the entire instruction block.

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

### 6.2 Injecting extra markdown

You can append arbitrary markdown via `additional`:

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

Given:

- role = "You are a coding agent operating in /home/xxxx/Projects/tact."
- guidelines = ["Think before you act"]
- constraints = ["Never expose secrets"]
- memory = "User prefers concise responses."
- dynamic_context = "Recent files: src/prompt/mod.rs"

The rendered prompt looks like:

```markdown
# Your role

You are a coding agent operating in /home/xxxx/Projects/tact.

# Guidelines you need to follow

- Think before you act

# Constraints that must be adhered to

- Never expose secrets

=== DYNAMIC_BOUNDARY ===

## Memory

User prefers concise responses.

## Dynamic context

Recent files: src/prompt/mod.rs
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

- Template: [`crates/tact/src/prompt/system_prompt_template.md`](../crates/tact/src/prompt/system_prompt_template.md)
- Builder / render logic: [`crates/tact/src/prompt/mod.rs`](../crates/tact/src/prompt/mod.rs)
- Agent wiring: [`crates/tact/src/lib.rs`](../crates/tact/src/lib.rs) (`Agent::build_system_prompt`)
- Memory manager: [`crates/tact/src/memory/mod.rs`](../crates/tact/src/memory/mod.rs) — see [Persistent Memory](./03_chapter_memory.md)
- Skill registry: [`crates/tact/src/skill/mod.rs`](../crates/tact/src/skill/mod.rs) — see [Skill Registry](./02_chapter_skill.md)
- Dynamic context loader: [`crates/tact/src/lib.rs`](../crates/tact/src/lib.rs) (`fn load_dynamic_context`)
