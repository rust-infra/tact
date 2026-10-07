# `/lang` — the UI language becomes a persisted `[ui]` preference

Status: **implemented in the working tree**, branch `feat/codex-hook-and-mcp-parity`, not yet
committed as of 2026-10-02. Written alongside the code.

## Problem

- `/theme` gained a "save to config?" step one commit earlier (`a898dbde`). `/lang`, sitting right
  next to it, still did `app.toggle_language()` and nothing else.
- `[ui]` had no language key, and the TUI read the language from nowhere — `App::new` hardcoded
  `Language::English`. A language chosen at runtime reverted on every launch, and no file could
  change it.
- So the choice was unmakeable in the only place that mattered: someone who reads Chinese had to
  press `Ctrl+L` again every single session.

## Design

### Inherited contract

The persist shape is `/theme`'s, not a new one: apply **silently** → ask "save to config?" with
**No** as the default → write through a `persist_*` / `update_ui_*_in_toml` pair → report
"session only" when there is nothing to write to. See
[2026-10-01-tui-theme-picker-design.md](2026-10-01-tui-theme-picker-design.md). Everything below is
what differs.

### Toggle, not picker

With two languages a picker costs a keystroke and buys nothing, so `/lang` still flips and `Ctrl+L`
still flips the same way. What `/lang` adds is the second step — and it is the only difference from
`Ctrl+L`.

### `[ui] language`, and deliberately no CLI flag

- `UiTomlConfig.language: Option<String>` → `UiSettings.language: String`, resolved with `"en"` as
  the default.
- **No `--language`.** `--theme` earns its keep because a theme is chosen per terminal; a language
  belongs to the person, and `/lang` writes it once. The resolution chain is therefore only
  "TOML → default", one level shorter than the theme's.
- Read back at startup by `App::set_configured_language`, called *after* `App::new` so the
  components' `Messages` snapshots get refreshed through `apply_language` like any other switch.

### The locale tag, not the display label

`Language::label()` returns `中文` for Chinese — the string the user just picked, and the string the
UI draws. It is not a value `Language::parse()` can read back, so writing it to the file would be a
silent no-op on the next launch. New `Language::as_str()` (`en` / `zh`) is what goes in.

`Language::parse()` returns `Option<Self>` instead of carrying a fallback: the caller owns the
fallback, so "what the config said" and "what we guessed" stay distinguishable. It accepts
`en` / `english` and `zh` / `cn` / `chinese`, case- and whitespace-insensitively. An unrecognized
value logs `tracing::warn!` and falls back to English — silently reading `language = "jp"` as
English would look like the config had said so.

### One action, one message

`toggle_language` used to both flip and announce. `/lang` reusing it would print `🌐 Language: 中文`
and then `saved` / `session only` — the same thing twice. So the flip is split:

| Method | Effect | Caller |
|---|---|---|
| `apply_language(language)` | silent; sets `self.language` **and** pushes the new `Messages` to `thinking` / `stream` / `tools` | `/lang`, `set_configured_language` |
| `toggle_language()` | `apply_language(self.language.next())` + write `[ui] language` + one message | `Ctrl+L` |

The split is not stylistic. `self.language` is what the *render* path reads, while the components
snapshot their `Messages` at construction — flipping only `self.language` would redraw rows that
already exist inside the old locale's chrome.

> **Updated 2026-10-04.** `Ctrl+L` originally announced the flip without persisting it, so the same
> complaint applied to the language: the choice was gone on the next launch. It now writes
> `[ui] language` itself, mirroring `Ctrl+T` (see
> [2026-10-01-tui-theme-picker-design.md](2026-10-01-tui-theme-picker-design.md) and the Ch 26 entry
> for 2026-10-04). The announce-only template was removed with it.

### Shared config probe

`theme_config_available()` becomes `ui_config_available()`. Both commands write the same `[ui]`
table, so both must give the same answer to "does that table have a file to live in".

> **Updated 2026-10-04.** The probe no longer reads the process-global settings on demand: it is
> `App::ui_config_available()`, backed by `App::ui_config_path`, which `run_tui` captures once at
> startup (via `TuiConfig::ui_config_path`). `Ctrl+T` and `Ctrl+L` are now callers too, and a global
> read inside a key handler is what made the toggle tests write into whichever config file a
> parallel test happened to have installed.

### `PersistLang` is its own variant

Not one "which `[ui]` key" variant: the two write different spellings (a theme name vs a locale tag)
and report in their own words.

## Non-goals

- **No language picker.** Two languages; a toggle is the whole choice.
- **No CLI flag** (see above).
- **No third locale.** `parse` accepts aliases for the two that exist, not new languages.
- **No translation of model output.** Only UI chrome.

## Verification

- `crates/tui/src/handlers/select.rs`: `language_persist_step_asks_and_defaults_to_no`,
  `language_persist_step_reports_session_only_without_a_config_file`,
  `declining_to_persist_reports_a_session_only_language`, and
  `confirming_save_writes_the_locale_tag_and_leaves_the_other_tables` — the last one drives the whole
  flow by keypress against a real temp config and asserts the file contains `language = "zh"` (not
  `中文`) and that `[llm.providers.kimi]` survived the write.
- `crates/tui/src/widgets/state/app/config.rs`: `apply_language_switches_without_announcing`,
  `language_names_round_trip_through_the_config_spelling`.
- `crates/tact/src/config/persist.rs`: `updates_ui_language_keeping_the_theme_line` (the sibling
  `theme` line and its trailing comment survive),
  `ui_language_is_created_when_the_table_is_missing`.
- Docs: [Ch 23](../../../book/23_chapter_tui_zh.md) §6.10, [Ch 21](../../../book/21_chapter_config_zh.md)
  §4 / §5 / §6, `config.example.toml`, [Ch 26](../../../book/26_chapter_issue_zh.md) 2026-10-02 entry.

## Open question

`cmd_lang` still reads `Toggle language (EN/中文)` and does not mention the persist step. Left as-is
deliberately: `cmd_theme` does not mention its own persist step either, so the two stay parallel.
Worth revisiting only if both descriptions are rewritten as a pair.
