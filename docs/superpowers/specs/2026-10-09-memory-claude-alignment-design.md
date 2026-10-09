# Memory aligned with Claude Code's auto-memory model

Status: **implemented in the working tree** as of 2026-10-09, branch `main`.
Written alongside the code.

## Problem

Tact's persistent memory copied Claude Code's *format* (YAML frontmatter,
`type`, a `MEMORY.md` index) but not its *structure*. The differences are not
cosmetic — they were the two complaints that motivated this change:

| | Claude Code | Tact (before) |
|---|---|---|
| Storage | `~/.claude/projects/<repo>/memory/` — one directory per **git repository**, shared across that repo's worktrees and subdirectories | `~/.tact/memory/` — one directory for **every project on the machine** |
| Injected at startup | only `MEMORY.md`, truncated at 200 lines / 25 KB | **every memory's full body**, concatenated, with no byte bound |
| Topic files | read on demand by the model's file tools | never written (`MEMORY.md` was the only index) |
| `MEMORY.md` | *is* the loaded artifact | regenerated on every save but **filtered out at load**, so it never reached the prompt |
| Staleness | `modified` ISO-8601 frontmatter stamped on write | no timestamp |
| Toggle | `autoMemoryEnabled` per user/project + `CLAUDE_CODE_DISABLE_AUTO_MEMORY` | `[agent].memory_enabled`, machine-wide only |
| Relocation | `autoMemoryDirectory` setting | none (added by this change) |

Two concrete failures followed from the "one global directory" choice:

1. **Project facts leaked everywhere.** A note about one repository's release
   branch is injected in every unrelated repository — the reader has no way to
   tell which project a memory came from, and the model must reason past facts
   that do not apply.
2. **Prompt growth was unbounded.** `load_memory_prompt` walked every entry and
   emitted its full content. The 200-line cap existed only on the *index file*
   (`rebuild_index`), which was never injected — so the cap guarded the wrong
   artifact and the prompt was uncapped.

## Design

### 1. Repository-scoped storage directory

New resolution chain, in `crates/tact/src/memory/mod.rs`:

```
memory_root(workdir)
  ├─ git repository → ~/.tact/projects/<slug>/memory/
  ├─ no repo       → <workdir>/.tact/memory/        (project root)
  └─ no $HOME      → <workdir>/.tact/memory/        (legacy fallback)
```

`<slug>` is derived from the repository's **common git dir**, not from the
worktree path: `git rev-parse --git-common-dir` returns `.git` from the main
checkout and the same absolute `<main>/.git` from a linked worktree. Slugging
that path therefore gives one directory per *repository*, so a worktree and its
main checkout share memories — matching Claude Code, including its
"project configs & auto memory now shared across git worktrees" behavior.

Resolution is **pure**: it reads `$HOME`, walks parents for a `.git`, and reads
that file/directory. It never spawns a process. `.git` is either a directory
(main checkout) or a file containing `gitdir: <path>` (linked worktree,
submodule); both are handled.

Slug: the common git dir's parent, made absolute, with every character outside
`[A-Za-z0-9._-]` replaced by `-`. `/Users/me/Projects/tact/.git` →
`-Users-me-Projects-tact`. (Claude's own slug is `-Users-rg-Projects-<name>`;
the exact spelling is ours to choose because nothing else consumes it.)

### 2. Index-only injection, with topic bodies on demand

`load_memory_prompt` is replaced by `load_memory_index_prompt`:

- The loaded artifact is `MEMORY.md`, capped at `MAX_INDEX_LINES` (200) or
  `MAX_INDEX_BYTES` (25 KB), whichever comes first — the same two limits
  Claude Code uses.
- Each index line carries the topic file name, so the model knows what it can
  fetch: `- name (file.md): description [type]`.
- The prompt block states that topics are one `load_memory` call away.

`MEMORY.md` must therefore always be *correct* on disk (it is now the thing that
is read), not merely regenerated on save: `load_all` re-renders it whenever the
content differs, so a hand-edited directory, an externally added file, or a
directory left by an older layout all produce a fresh index. Re-rendering is
skipped when the bytes already match, so a clean start does not churn the file's
mtime.

### 3. `load_memory` tool

New native tool next to `load_skill`, registered under the same
`memory_enabled` gate as `save_memory`:

- Input: `name` — the *file stem* the index lists.
- Output: the full frontmatter + body, or a listed error of the known names.
- Read-only, `Independent` resources; `read_json` metadata preset.

This is the gap that made Claude Code's design non-transplantable: Tact's file
tools refuse anything outside the workspace (`tool::safe_path`), so a memory
living in `$HOME/.tact/` is unreachable by `read_file`. Without this tool,
index-only injection would hide memory content from the model entirely.

### 4. `[agent].auto_memory_directory`

The counterpart of Claude Code's `autoMemoryDirectory`, added so the derived
layout is overridable rather than hardcoded:

- New `[agent].auto_memory_directory: Option<String>`, resolved verbatim into
  `AgentSettings.auto_memory_directory` (trimmed, blank → `None`) so path
  expansion happens where the workdir is known.
- Expansion (`expand_memory_dir`) follows the three rules `[agent].skill_dirs`
  already established — `~`/`~/…` against `$HOME`, relative against the
  workdir, absolute as-is — so two path-valued settings cannot disagree about
  what `~/x` means.
- The override is consulted **before** the repository derivation and before the
  `$HOME`-unset fallback, so it applies unconditionally.
- Read from the installed process config (`try_settings()`), the same way
  `get_skill_registry` reads `skill_dirs`. That keeps `memory_root(workdir)`
  callable from bootstrap without threading a parameter through every caller;
  `memory_root_with(workdir, home, override)` holds the whole decision so the
  logic stays unit-testable without the global config.

### 5. `modified` timestamps

`save_memory` stamps `modified: <ISO-8601 UTC>` into the frontmatter it writes,
so a memory's age is visible both in the file and to the model on load.
Frontmatter is parsed with `#[serde(default)]`-tolerant fields, so existing
files without the field keep loading.

## Migration

Existing installs have one global `~/.tact/memory/` (and, on older ones, a
project-local `<workdir>/.tact/memory/`). Scoping would strand both.

`migrate_legacy_memory(legacy_dir, memory_dir)`:

- Runs once per session at bootstrap, before the manager loads.
- Copies `*.md` files (skipping `MEMORY.md`) from the legacy global dir into
  the repository directory **only when the destination is empty**; nothing is
  deleted, so a user with two checkouts keeps the source intact.
- Reports the copy count through the existing `Notices` channel, so the move is
  visible rather than silent.

## Files

| Area | File |
|---|---|
| Resolution, index cap, migration, timestamps | `crates/tact/src/memory/mod.rs` |
| `auto_memory_directory` config key + resolution | `crates/tact/src/config/types.rs`, `crates/tact/src/config/resolve.rs` |
| `~/.tact/projects` path helper | `crates/tact/src/consts.rs` |
| `load_memory` tool | `crates/tact/src/tool/memory.rs` |
| Tool registration (both gates) | `crates/tact/src/tool/registry.rs` |
| Bootstrap wiring + notices | `crates/tact-ui/src/session_bootstrap.rs` |
| Docs | `book/03_chapter_memory_zh.md`, `README.md`, `ARCHITECTURE.md`, `config.example.toml` |
