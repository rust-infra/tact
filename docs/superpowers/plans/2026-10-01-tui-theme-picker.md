# Plan: `/theme` picker + opt-in persist

Spec: [2026-10-01-tui-theme-picker-design.md](../specs/2026-10-01-tui-theme-picker-design.md)

Shipped as `a898dbde`. Retroactive record (2026-10-02); the steps below are the shape the commit
took, not a forecast.

1. **Kit** (`crates/agent_tui_kit/src/theme.rs`)
   - `ThemeName` gains `Eq`; `all()` becomes `pub`; new `as_str()` (canonical config spelling).
   - `theme_name_round_trips_through_as_str` pins `as_str` against `from_str`.
2. **Kit i18n** (`crates/agent_tui_kit/src/i18n.rs`)
   - `theme_select_prompt`, `theme_persist_prompt`, `theme_persisted_tmpl`,
     `theme_persist_failed_tmpl`, `theme_session_only_tmpl` in both locales.
   - `model_persist_yes` / `model_persist_no` renamed to `persist_yes` / `persist_no` — they are no
     longer model-specific.
   - `cmd_theme` description rewritten to name the picker and `Ctrl+T`.
3. **Config** (`crates/tact/src/config/{mod.rs,persist.rs}`)
   - `persist_theme(&str)` next to the other `persist_*` entry points; no config file is an error
     the caller reports as "session only".
   - `update_ui_theme_in_toml`; `set_scalar` replaces `Table::insert` in all five helpers so a
     replaced line keeps its trailing comment.
   - Tests: `replacing_a_model_keeps_its_trailing_comment`,
     `updates_ui_theme_keeping_the_rest_of_the_file`.
4. **TUI state** (`crates/tui/src/widgets/state/mod.rs`, `app/config.rs`)
   - `SelectKind::{ThemePick, PersistTheme}`.
   - `set_theme` / `apply_theme` split; `toggle_theme` becomes `set_theme(name.next())`.
   - `theme_label(msgs, name)` extracted from the twelve-arm `match`.
5. **TUI handlers** (`crates/tui/src/handlers/{mod.rs,select.rs}`)
   - `start_theme_picker` builds the rows from `ThemeName::all()`, marks the current one ` *`, and
     preselects it.
   - `C::Theme` calls the picker instead of `toggle_theme`.
   - `select.rs`: Enter applies via `apply_theme` then opens the persist step; Esc returns to
     `Normal`; `open_theme_persist_step` / `finish_theme_persist` own the question and the
     "session only" reporting.
6. **Tests**
   - `cargo test -p tui --lib handlers::` — picker opens marked and preselects, Enter applies,
     Esc keeps, `Ctrl+T` still cycles, persist step asks and defaults to No, reports session-only
     without a config file.
   - `cargo test -p tui --lib widgets::state::app::config` — `set_theme_switches_to_the_named_theme`,
     `toggle_theme_cycles_from_ink`.
   - `cargo test -p agent_tui_kit --lib theme` — the round-trip.
   - `cargo test -p tact --lib config` — the two persist tests.
7. **Docs sync**
   - `book/23_chapter_tui_zh.md` §6.10: the picker, `apply_theme` vs `set_theme`, the persist step.
   - `book/26_chapter_issue_zh.md`: 2026-10-01 entry, including the `set_scalar` defect.
