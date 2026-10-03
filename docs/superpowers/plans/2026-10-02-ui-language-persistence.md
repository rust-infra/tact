# Plan: `/lang` persist step

Spec: [2026-10-02-ui-language-persistence-design.md](../specs/2026-10-02-ui-language-persistence-design.md)

1. **Kit i18n** (`crates/agent_tui_kit/src/i18n.rs`)
   - `Language` gains `Eq`, `as_str()` (`en` / `zh`), `parse()` (`Option`, case- and
     whitespace-insensitive, aliases `english` / `cn` / `chinese`).
   - `lang_persist_prompt`, `lang_persisted_tmpl`, `lang_persist_failed_tmpl`,
     `lang_session_only_tmpl` in both locales.
2. **Config** (`crates/tact/src/config/`)
   - `types.rs`: `UiTomlConfig.language: Option<String>`, `UiSettings.language: String`.
   - `resolve.rs`: read `[ui].language`, default `"en"`, no CLI flag; thread through
     `resolve_non_llm` → both `UiSettings` construction sites.
   - `persist.rs`: `update_ui_language_in_toml` via the shared `set_scalar` path.
   - `mod.rs`: `persist_language(&str)`, erroring when there is no `config_path`.
   - Tests: `updates_ui_language_keeping_the_theme_line`,
     `ui_language_is_created_when_the_table_is_missing`.
3. **TUI state** (`crates/tui/src/widgets/state/`)
   - `mod.rs`: `SelectKind::PersistLang { language }`.
   - `app/config.rs`: split `apply_language` (silent, pushes `Messages` to thinking / stream /
     tools) out of `toggle_language` (apply + announce).
   - `app/construct.rs`: `set_configured_language(&str)` — `parse` or warn + English.
4. **TUI handlers** (`crates/tui/src/handlers/{mod.rs,select.rs}`)
   - `C::Lang` calls `start_language_toggle` instead of `toggle_language`.
   - `start_language_toggle` / `open_language_persist_step` / `finish_language_persist`;
     `theme_config_available` renamed `ui_config_available` and shared by both commands.
   - Enter and Esc branches in `handle_select_mode` cover `PersistLang`.
5. **Wiring** (`crates/tui/src/lib.rs`, `crates/tact-ui/src/interactive.rs`)
   - `TuiConfig.language`; `app.set_configured_language(&language)` after `App::new`.
   - Test fixtures: add `language` to every `UiSettings` literal —
     `crates/tact/src/config/mod.rs`, `crates/tact/src/tool/read_image.rs`,
     `crates/tact-ui/src/test_support.rs`, `crates/tact-ui/tests/recovery_compaction.rs`.
     (`cargo check --workspace --all-targets` reports them one wave at a time: `tact-ui`'s lib fails
     first, so the integration-test literal only surfaces after the lib one is fixed. Re-run the
     check after fixing.)
6. **Tests**
   - `cargo test -p tui --lib handlers::select::tests` — the four language persist tests, including
     the end-to-end write against a temp config under `MODELS_TEST_LOCK`.
   - `cargo test -p tui --lib widgets::state::app::config` — silent-apply and round-trip.
   - `cargo test -p tact --lib config` — the two persist tests.
7. **Docs sync** (one pass, per the AGENTS.md trigger table)
   - `config.example.toml`: `[ui] language`.
   - `book/21_chapter_config_zh.md`: §1 table, §4 example, §5 defaults, §6 "no CLI flag" paragraph.
   - `book/23_chapter_tui_zh.md` §6.10: `/lang` beside `/theme`.
   - `book/26_chapter_issue_zh.md`: 2026-10-02 entry.
8. **Gate** — `cargo fmt` first, then `fmt --check` → `clippy --all-targets --all-features -D warnings`
   → `build` → `test`, sequentially.
