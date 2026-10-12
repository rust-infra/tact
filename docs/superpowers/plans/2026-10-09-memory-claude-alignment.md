# Plan: memory aligned with Claude Code's auto-memory model

Spec: [2026-10-09-memory-claude-alignment-design.md](../specs/2026-10-09-memory-claude-alignment-design.md)

Implemented on `main` 2026-10-09; this plan is **backfilled** 2026-10-12 from the
landed code, because main shipped the feature with a spec only.

Ported onto the post-migration layout (2026-10-12, `cae4920c`): paths are
`crates/tact_extensions/…` and `crates/tact_ui/…`; main's version bump was not
carried over. `./scripts/check-rust.sh` → `Rust checks passed.`; whole-workspace
tests 2987 → 3016 passed / 0 failed / 22 ignored (47 suites).

## 1. One memory directory per repository (`crates/tact_extensions/src/memory/mod.rs`)

- `memory_root(workdir)` is the entry point: `memory_root_with(workdir, home, override)`
  holds the decision, `memory_root_for(workdir, home)` derives the path — split so the
  derivation is unit-testable without touching `$HOME`.
- `repo_slug(workdir)` slugs the **common git dir's parent** (the checkout root), keeping a
  leading `-`, so `-Users-me-Projects-tact` matches Claude's spelling. A root that slugs to
  `-` alone yields `None`.
- `git_common_dir(workdir)` walks parents for `.git` and reads it directly (no
  `git rev-parse`): a directory is the common dir; a `gitdir: <path>` file (worktree /
  submodule) is followed through `common_dir_of`, which reads the `commondir` file — so a
  main checkout and its linked worktrees resolve to **one** common dir.
- `lexical_normalize` folds `.` / `..` textually so `/repo/crates/tact/../..` cannot produce
  a second directory for one repository. No repo or no `$HOME` → `<workdir>/.tact/memory`.
- `crates/tact_extensions/src/consts.rs`: `TactPath::home_projects_dir` /
  `home_projects_dir_for` produce `$HOME/.tact/projects`; `home_memory_dir` /
  `home_memory_dir_for` still name the retired `$HOME/.tact/memory` (migration source only).

## 2. The `[agent].auto_memory_directory` override

- `crates/tact_extensions/src/config/types.rs`: `AgentTomlConfig.auto_memory_directory:
  Option<String>` (line ~207) and `AgentSettings.auto_memory_directory: Option<String>`
  (line ~472).
- `crates/tact_extensions/src/config/resolve.rs` (`resolve_non_llm`): the TOML value is
  trimmed, an empty string filtered to `None`, and carried through `NonLlmSettings` into
  both `AgentSettings` literals — kept **verbatim**, because `~` / relative forms need the
  workdir that resolution does not have.
- `memory/mod.rs`: `expand_memory_dir(raw, workdir)` matches the `[agent].skill_dirs` rules
  (`~` / `~/…` → `$HOME`, relative → workdir, absolute as-is). The override is consulted
  first in `memory_root_with`, before the repository derivation and the `$HOME` fallback.
- `memory/mod.rs`: `configured_memory_dir()` reads `try_settings()`;
  `override_from_settings` extracts the field by reference so it is testable without the
  process-global config.

## 3. Index-only injection (`crates/tact_extensions/src/memory/mod.rs`)

- `MemoryManager::load_memory_index_prompt` replaces the old body-dumping prompt: it injects
  `MEMORY_INDEX_FILE` (`MEMORY.md`) and one sentence telling the model to fetch a topic with
  `load_memory`. Empty store → empty string, so a project with no memories pays nothing.
- `truncate_index` applies both caps, whichever trips first: `MAX_INDEX_LINES` (200) and
  `MAX_INDEX_BYTES` (25 KB); `render_index` also stops at 200 lines.
- `render_index` writes `- {name} ({file_stem}.md): {description} [{type}]`; the file stem is
  the `load_memory` handle and the index is the only place the model learns it.
- `load_all` re-renders the index whenever the on-disk bytes differ (`render_index` +
  `write_index`), so a hand-written, externally edited, or older-layout directory is fixed on
  load; identical bytes skip the write, so mtimes do not churn.
- `load_topic(name)` returns a topic's full file text by file stem (a matching display `name`
  is also accepted), with an error listing the known stems on a miss.
- `crates/tact_extensions/src/agent/mod.rs` (`load_memory_prompt`): forwards to
  `manager.load_memory_index_prompt()`; the call site at line ~2844 is unchanged.

## 4. The `load_memory` tool (`crates/tact_extensions/src/tool/memory.rs`)

- `LoadMemoryInput { name }` (the index's file stem); `load_memory(ctx, input)` locks the
  manager and calls `load_topic`.
- `LOAD_MEMORY_METADATA = ToolMetadata::read_json("load_memory", …)` — read-only,
  `Independent` resources, JSON input.
- `crates/tact_extensions/src/tool/registry.rs`: `toolset_with_memory(memory_enabled)`
  registers `save_memory` and `load_memory` together; `try_toolset` adds both only when
  `memory_enabled`. This is what makes index-only injection usable — `read_file` cannot reach
  a memory under `$HOME` (`tool::safe_path`).

## 5. `modified` timestamps

- `MemoryManager::save_memory` stamps `modified: {chrono iso-8601 UTC}` into the frontmatter
  (`SecondsFormat::Secs`, `true`), returns a `Saved memory …` line, and rebuilds the index.
- `MemoryFrontmatter` fields are all `Option` and parsed with `unwrap_or_default()`, so files
  written before the field existed still load.

## 6. Legacy migration (`crates/tact_ui/src/session_bootstrap.rs`)

- `bootstrap_session` resolves `tact_extensions::memory::memory_root(&work_dir)`, then, when
  `TactPath::home_memory_dir()` differs, calls
  `tact_extensions::memory::migrate_legacy_memory(&legacy, &memory_dir)`; `memory_manager`
  is built after.
- `migrate_legacy_memory` copies `*.md` (skipping `MEMORY.md`) into the repository directory
  **only when it holds no `.md` yet**, never deletes the source, and returns the copy count.
- The count is reported through the `Notices` channel (`Notices::notice("memory", …)`); a
  failure is a notice, never fatal.

## 7. Tests

- `cargo test -p tact_extensions --lib memory::` — derivation, override, injection, migration:
  `a_worktree_and_its_checkout_resolve_to_one_directory` (fixture with `.git/worktrees/<name>`
  + `commondir`), `the_real_worktrees_in_this_repository_share_the_main_directory`,
  `different_repositories_get_different_directories`, `a_dotted_workdir_resolves_like_its_normal_form`,
  `outside_a_repository_the_workdir_owns_the_directory`, `the_config_value_is_what_the_resolver_reads`,
  `a_configured_directory_overrides_the_derived_one`, `expand_memory_dir_follows_the_skill_dirs_rules`,
  `prompt_carries_the_index_and_not_the_bodies`, `an_empty_store_injects_nothing`,
  `a_hand_written_directory_gets_an_index_on_load`, `byte_cap_truncates_a_long_index`,
  `parses_frontmatter_with_yaml_header`, `saves_memory_and_rebuilds_index`,
  `load_topic_returns_the_full_file`, `migration_copies_memories_once_and_never_overwrites`.
- `cargo test -p tact_extensions --lib resolve::` — `resolve_auto_memory_directory_from_toml`
  (including blank = unset).
- `cargo test -p tact_extensions --lib tool::memory::` —
  `load_memory_returns_a_saved_body_and_lists_the_known_names`; the toolset gate is pinned by
  `memory_is_on_in_the_default_toolset` / `disabled_memory_removes_save_memory` in
  `tool/registry.rs`.
- `./scripts/check-rust.sh` (fmt + clippy `-D warnings` + the four packages). Whole-workspace
  2987 → 3016 passed / 0 failed / 22 ignored (47 suites) for the three #87 slices together.

## 8. Docs

- `book/03_chapter_memory_zh.md` (layout, index-only injection, `load_memory`,
  `auto_memory_directory`, migration), `ARCHITECTURE.md` (memory rows ~368 / ~424),
  `config.example.toml` § memory.
- **Drift, not updated:** the spec lists `README.md`, but `README.md:174` still says memory is
  `~/.tact/memory` and names only `save_memory` — 未在代码中找到 the README update.

## 9. Deliberate deviation: the real-worktree test skips when `.worktrees/` is empty

- `the_real_worktrees_in_this_repository_share_the_main_directory` no longer asserts this
  checkout has at least one real worktree: `.worktrees/` is gitignored and non-empty only
  while worktrees exist. It returns early when `.worktrees/` is absent and otherwise still
  asserts every worktree resolves to the main checkout's directory; the fixture
  `a_worktree_and_its_checkout_resolve_to_one_directory` covers the `commondir` logic.
