# `/theme` — a picker, and an opt-in write to config

Status: **shipped** (`a898dbde`, 2026-10-01).
Retroactive record, written 2026-10-02 from the shipped code, that commit's message, and the
Ch 26 entry. Nothing here is a new decision — it is the design the code already implements, put
where the other designs live.

## Problem

- `/theme` did exactly one thing: `C::Theme => app.toggle_theme()`, i.e. `ThemeName::next()` around
  a ring of twelve. Going from `ink` (the config default) to `kawaii` meant six presses, each one
  needing a glance at the bottom bar to see where you had landed.
- `/model` and `/permission` have been pickers for a while — list, current entry marked, Enter
  confirms. The theme was the odd one out.
- The twelve labels lived in a twelve-arm `match` inside `toggle_theme`. A picker needs the same
  twelve labels, and some of them are not literal translations (`japanese` is `Wa` in English,
  `和風` in Chinese) — two copies of that table is a bug waiting for the first rename.
- Nothing wrote `[ui] theme`. A theme chosen at runtime reverted on the next launch, and no file
  could change it.

## Design

### Command surface

| Input | Effect |
|---|---|
| `/theme` | `SelectKind::ThemePick` over `ThemeName::all()`; the current row carries ` *` **and** is the initial selection (not parked on row one) |
| Enter | Apply the highlighted theme, then ask about persisting it |
| Esc | Keep the current theme, back to `InputMode::Normal` |
| `Ctrl+T` | `toggle_theme` — the cheap "next one" path, unchanged |

`ThemeName::all()` changed from private to `pub` for this: the picker needs the cycle order, and a
caller should not have to reconstruct it by calling `next()` until it wraps.

### One label table

`theme_label(msgs, name)` (`crates/tui/src/widgets/state/app/config.rs`) is the single source for
the picker rows, the `Ctrl+T` change message, and the argument to `theme_changed_tmpl`. The
twelve-arm `match` that used to live inside `toggle_theme` is gone.

### Two apply paths — one action, one message

`set_theme` applies **and** announces; `apply_theme` applies silently. `Ctrl+T` goes through
`set_theme`; the picker goes through `apply_theme` and lets its persist step do the talking. If the
picker used `set_theme`, one action would print `🎨 Theme: Nord` and then `saved` / `session only` —
the same thing twice.

### The persist step

- `SelectKind::PersistTheme { name }` mirrors the question `/model` already asks: options Yes / No,
  default **No** — writing a file is the deliberate answer, and the theme is applied either way.
- Yes → `tact::config::persist_theme(ThemeName::as_str())` → `update_ui_theme_in_toml` → `[ui] theme`.
- No, Esc, or **no config file at all** → "this session only". With no config file the question is
  not asked in the first place: `ui_config_available()` gates it, because asking a question whose
  "yes" cannot be honoured is worse than an honest line.

### Canonical spelling, not the display label

`ThemeName::as_str()` writes `ink-light`, not `InkLight` — the spelling `ThemeName::from_str` reads
back. `theme_name_round_trips_through_as_str` fails if the two drift, because a name `from_str`
cannot read back would silently reset the theme on the next launch.

### The defect this surfaced

`persist.rs`'s helpers used `Table::insert`, which swaps the whole item and takes the *replaced*
item's decor with it: `model = "x"  # pinned for the ctx window` lost its comment on every `/model`
save, while the module doc claimed comments were preserved. The new `set_scalar()` clones the
existing item's decoration onto the new value, and all five persist helpers (model / model+budget /
subagent / effort / theme) go through it. `replacing_a_model_keeps_its_trailing_comment` and
`updates_ui_theme_keeping_the_rest_of_the_file` pin the behaviour.

## Non-goals

- **No new CLI flag.** `--theme` already exists and keeps its meaning.
- **No live reload of anything but the theme.** LLM / provider changes still need a restart.
- **No theme editor and no user-defined themes.** The picker offers the twelve built-ins.

## Verification

- `crates/tui/src/handlers/mod.rs`: `theme_command_opens_a_picker_marked_at_the_current_theme`
  (twelve rows, exactly one ` *`, opened on that row), `confirming_the_theme_picker_applies_the_chosen_theme`,
  `cancelling_the_theme_picker_keeps_the_theme`, `ctrl_t_still_cycles_themes`.
- `crates/tui/src/handlers/select.rs`: `theme_persist_step_asks_and_defaults_to_no`,
  `theme_persist_step_reports_session_only_without_a_config_file`,
  `declining_to_persist_reports_a_session_only_theme`.
- `crates/tui/src/widgets/state/app/config.rs`: `set_theme_switches_to_the_named_theme`,
  `toggle_theme_cycles_from_ink`.
- `crates/agent_tui_kit/src/theme.rs`: `theme_name_round_trips_through_as_str`.
- `crates/tact/src/config/persist.rs`: `replacing_a_model_keeps_its_trailing_comment`,
  `updates_ui_theme_keeping_the_rest_of_the_file`.
- Docs: [Ch 23](../../../book/23_chapter_tui_zh.md) §6.10, [Ch 26](../../../book/26_chapter_issue_zh.md)
  2026-10-01 entry.
