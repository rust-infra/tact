# Extra Skill Directories Config — Design

Date: 2026-07-24  
Status: Approved  
Related: `book/02_chapter_skill.md`, `crates/tact/src/skill/mod.rs`, `crates/tact/src/consts.rs`

## Goals

1. Allow TOML config to list **additional** skill roots beyond the built-in search path.
2. Relocate the workdir built-in root from `<workdir>/skills/` to
   `<workdir>/.tact/skills/` (replace, not dual-scan).

## Non-goals

- CLI flag for skill dirs (can add later).
- Replacing / disabling built-in roots via config.
- Namespacing skills from extra dirs (same bare-name rules as standalone skills).
- Migrating existing `<workdir>/skills/` trees on disk.

## Approach

### Built-in roots (later wins)

1. `<workdir>/.tact/skills/`
2. `~/.tact/skills/`
3. `~/.agents/skills/`
4. `<workdir>/.claude/skills/`
5. **`[agent].skill_dirs` in config order**
6. Installed plugin cache (`plugin:skill` names unchanged)

### Config

```toml
[agent]
skill_dirs = ["~/shared-skills", "./vendor/skills"]
```

- Optional; default empty / omitted.
- Each entry is a root that contains `*/SKILL.md` (same layout as other roots).
- Relative paths resolve against **workdir**.
- `~` / `~/…` expand via `$HOME`.
- Missing directories are skipped (same as today’s soft-skip).

### Wiring

- `AgentTomlConfig.skill_dirs: Option<Vec<String>>`
- `AgentSettings.skill_dirs: Vec<String>` (raw, unresolved)
- `get_skill_registry(workdir)` appends resolved extras from
  `config::try_settings()` when installed
- `/skill-reload` keeps calling `get_skill_registry(&app.work_dir)` — picks up
  the same settings automatically

### API rename

- Replace `legacy_skills_dir()` with `tact_skills_dir()` → `tact_dir()/skills`

## Testing

- Unit: `.tact/skills` loads; bare `skills/` no longer loads.
- Unit: configured extra dir loads and overrides same-named earlier root.
- Unit: `~` expansion and relative-to-workdir resolution.
- Resolve smoke: TOML `skill_dirs` lands in `AgentSettings`.

## Docs sync

- `tact.example.toml`, bilingual Ch 2, Ch 26 entry, ARCHITECTURE / tact.md
  mentions of legacy `skills/`.
