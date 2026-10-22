# Plan: a bounded skill catalog

Spec: [2026-10-11-skill-catalog-budget-design.md](../specs/2026-10-11-skill-catalog-budget-design.md)

Done 2026-10-11; `./scripts/check-rust.sh` green. Uncommitted.

## 1. Bound the catalog (`crates/tact/src/skill/mod.rs`)

- `SkillDocument` gains `dir: PathBuf`, set from `path.parent()` in `load_skill_file` and
  `load_plugin_commands`.
- `Display for SkillDocument` emits `<skill name="…" dir="…">`, both attributes escaped
  (`escape_xml_attr`, the same rule `format_skill_agent_task` already applies to the name).
- `SkillRegistry` gains `disabled: HashSet<String>` + `set_disabled` / `is_disabled` / `disabled`.
  Filtering happens at **render** time, never at load: `/skill list` must still show a disabled row.
- Private `catalog_entries()` — name-sorted, disabled removed — backs all three renderers.
- `describe_available()` keeps its current meaning (uncapped, disabled-filtered); the TUI's
  `skills_description` reads it.
- New `describe_available_bounded(budget_chars, description_cap)` for the prompt:
  cap each description on a word boundary (`cap_description`), then drop the alphabetical tail and
  append the trailer (`- (+N omitted for the catalog budget: a, b, …)`, capped at
  `TRAILER_NAME_CAP = 40`), reserving the trailer's length in the keep-loop.
- New `catalog_budget_chars(model_context_window)`: `window / 100 * 8` chars, or `8_000` when the
  window is `0`. New `DEFAULT_SKILL_DESCRIPTION_CAP = 200`.
- `describe_available_with_body()` filters disabled, and is otherwise untouched.

## 2. The two `[agent]` keys

- `crates/tact/src/config/types.rs`: `AgentTomlConfig.skill_description_cap: Option<usize>` and
  `skill_disable: Vec<String>` (serde default); the same pair on `AgentSettings` as
  `skill_description_cap: usize` / `skill_disable: Vec<String>`.
- `crates/tact/src/config/resolve.rs`: resolve both in `resolve_non_llm`, carry them through
  `NonLlmSettings` and the two `AgentSettings` literals.
- `crates/tact/src/config/mod.rs`, `crates/tact/src/tool/read_image.rs`, `crates/tui/src/handlers/select.rs`,
  `crates/tact-ui/src/test_support.rs`, `crates/tact-ui/tests/recovery_compaction.rs`: the remaining
  `AgentSettings` literals (the compiler finds them).
- `crates/tact/src/skill/mod.rs`: `get_skill_registry` applies `skill_disable` to the registry.
- `config.example.toml`: document both keys next to `skill_body_auto_inject`.

## 3. Prompt assembly (`crates/tact/src/agent/mod.rs`)

- The `skills_available` block calls
  `describe_available_bounded(catalog_budget_chars(model_context_window), skill_description_cap)` in
  the non-auto-inject arm; the auto-inject arm is unchanged.
- Both prompt templates gain one sentence to the skill policy paragraph: bundled files resolve
  relative to the skill's `dir`.

## 4. The TUI surfaces

- `crates/agent_tui_kit/src/state/ui_types.rs`: `SkillEntry` gains `dir: String` and `disabled: bool`.
- Projections (`crates/tact-ui/src/interactive.rs`, `crates/tui/src/handlers/mod.rs`) fill both;
  `skills_snapshot_from(&SkillRegistry)` in `handlers/mod.rs` is the one projection, shared with the
  verbs below.
- `crates/tui/src/handlers/skills.rs::format_skill_agent_task` emits `dir` — the same wrapper shape as
  `SkillDocument::Display`.
- `crates/tui/src/handlers/mod.rs::skills_table_markdown` marks a disabled row.
- Every `SkillEntry { … }` literal (43, tests included) gains the two fields.

## 5. `/skill disable <name>` / `/skill enable <name>`

- `crates/tui/src/widgets/state/slash.rs`: both verbs declared in `SKILL_SUBCOMMANDS` with
  `takes_value: true`, so `/skill ` completes them and `every_declared_subcommand_has_a_handler` runs
  them.
- `crates/tui/src/widgets/state/slash_command.rs`: `subcommand_candidates` records the value-taking
  subcommand the walk landed on, and `skill_verb_candidates` offers the half that verb can act on as
  `<path> <name>` rows (`incomplete: false`, so Enter runs them). The space after `disable` keeps the
  popup open because `slash_candidates()` is non-empty — the only place in the tree where a value is
  completable.
- `crates/tui/src/handlers/skills.rs::set_skill_catalog_visibility`: resolve the name (unknown →
  flash, not the usage hint), treat a repeat as a no-op, flip the shared registry, rebuild
  `skills_data` from it, persist, and report in the `/hook-output` shape.
- `crates/agent_tui_kit/src/i18n.rs`: eight messages (en + zh) — hidden/shown report, persisted /
  session-only / write-failed, already-hidden / already-shown, unknown name.
- `crates/tact/src/config/persist.rs::update_agent_skill_disable_in_toml`: replace the whole array
  through `toml_edit` (comments preserved); an empty set is written as `[]`, not removed.
- `crates/tact/src/config/mod.rs::persist_skill_disable`: the public wrapper, `config_path` optional
  like `persist_theme`.

## 6. Tests

- `cargo test -p tact --lib skill::` — the catalog bound.
- `cargo test -p tact --lib config::` — the two keys, the TOML write.
- `cargo test -p tui --lib handlers::` — the verbs, the marked list, the unknown name.
- `cargo test -p tui --lib widgets::state::slash_command::` — the completion list.

## 7. Docs sync (one pass, at the end)

- `book/02_chapter_skill_zh.html`: §1 table, §3 wrapper, §5 API table, §7 the verbs, §9 map, §10 gaps.
- `book/21_chapter_config_zh.html`: the two keys and how `skill_disable` is usually written.
- `book/26_chapter_issue_zh.html`: 2026-10-11 entry.

## 8. Gate

- `./scripts/check-rust.sh` (fmt + clippy `-D warnings` + four packages). One cargo invocation at a
  time — the `target/` lock hangs parallel runs. `rustfmt` only the files this change touched: another
  session has an uncommitted `// PROBE:` edit in `crates/tact/src/permission/mod.rs`, which a
  workspace-wide `cargo fmt` could rewrite and which must not be staged.
