# Plan: `/skill` and slash subcommand completion

Spec: [2026-10-01-tui-skill-command-and-completion-design.md](../specs/2026-10-01-tui-skill-command-and-completion-design.md)

Shipped as `89400ca3`. Retroactive record (2026-10-02).

1. **Command model** (`crates/tui/src/widgets/state/slash.rs`)
   - `Subcommand { name, hint, children, takes_value }`; `SlashCommand::subcommands()` declares the
     tree, omitting anything the handler does not implement.
   - Remove `Skills` / `SkillReload`; `Skill` gains the `list` / `reload` children.
   - `needs_args()` gains `Skill`.
2. **Candidate selection** (`crates/tui/src/widgets/state/slash_command.rs`)
   - `Candidate { path, detail, is_skill, incomplete }` replaces the old match list.
   - `App::slash_candidates()` dispatches on how far the input has got;
     `command_candidates` / `subcommand_candidates` / `skill_candidates` are the three sources.
   - Skills leave `command_candidates` entirely.
3. **Input handling** (`crates/tui/src/handlers/insert.rs`, `handlers/palette.rs`)
   - Tab completes the whole path; Enter completes incomplete candidates and runs finished ones;
     a trailing space only closes the popup when nothing follows.
   - Palette Enter on a command with subcommands opens the completion popup instead of running it;
     prior input is preserved through the undo stack.
4. **Skill invocation** (`crates/tui/src/handlers/skills.rs`)
   - `invoke_skill(app, name, args, slash_line)` shared by `/skill <name>` and `/{name}`; only the
     echoed log line differs.
   - `skill_args_from_the_skill_command` strips the whole `/skill <name>` prefix.
   - A skill whose name collides with a declared subcommand is filtered out of the list.
5. **Highlighting** (`crates/tui/src/render/slash_style.rs`,
   `crates/agent_tui_kit/src/render/slash_style.rs`, `crates/agent_tui_kit/src/state/ui_types.rs`)
   - `split_skill_slash` handles the gathered `/skill demo` form; `SKILL_COMMAND` asserted against
     `SlashCommand::Skill`.
6. **Tests**
   - `cargo test -p tui --lib widgets::state::slash_command` (the nine candidate tests).
   - `cargo test -p tui --lib handlers::insert` (the five key tests).
   - `cargo test -p tui --lib handlers::mod` — including `every_declared_subcommand_has_a_handler`,
     which is the guard that keeps the declaration honest, and the three skill-command tests.
   - `cargo test -p tui --lib handlers::palette` (the three palette/undo tests).
   - `cargo test -p tui --lib handlers::skills` (the three invocation tests).
   - `cargo test -p tui --lib render` and `-p agent_tui_kit --lib render` — the full-frame popup and
     highlight tests.
7. **Docs sync**
   - `book/23_chapter_tui_zh.md`: `/skill` and the completion rules.
   - `book/26_chapter_issue_zh.md`: 2026-10-01 entries.
