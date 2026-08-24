# Subagent Popup Sectioned Rendering — Implementation Plan

- **Date:** 2026-08-24
- **Spec:** `docs/superpowers/specs/2026-08-24-subagent-sectioned-popup-design.md`
- **Task:** #44

## Steps

1. **Protocol** — `crates/protocol/src/tool_output.rs`
   - Add `SubagentSection` enum (Context default / Thinking / Tool) and `SubagentSectionBlock { section, text }`.
   - Add `section: SubagentSection` field to `ToolOutputChunk`; keep `stdout/stderr/other` defaults; add `with_section`.
   - Add `pub const THINKING_SECTION_HEADER: &str = "🧠 Thinking";`.
   - `ToolOutputBuffer`: add `sections` accumulation in `push_chunks` (ANSI-filtered text, merge same-section), `sections()`, `take_sections()`.
   - Update struct literals in `crates/tact/src/background.rs` (×3), `crates/tact/src/tool/bash.rs`, `crates/agent_tui_kit/src/components/tool.rs` test literal.
   - Add/extend protocol tests.

2. **Forwarder** — `crates/tact/src/tool/subagent_ui.rs`
   - Use `THINKING_SECTION_HEADER` in `format_thinking_block`.
   - Tag chunks: thinking → Thinking, step started/finished/failed → Tool, stream/info/error → Context (default).
   - Update tests to assert section tags; keep text assertions byte-identical.

3. **Component handoff** — `crates/agent_tui_kit/src/components/tool.rs` + `crates/agent_tui_kit/src/widgets/tool_widget.rs`
   - `ToolRenderOutput` gains `detail_sections: Option<Vec<SubagentSectionBlock>>`.
   - Builder `build()` sets `detail_sections: None`.
   - `on_step_finished` subagent branch: `output.detail_sections = Some(active.live_output.take_sections());`.
   - Add `detail_sections: None` to struct literals: `crates/agent_tui_kit/src/render/cells/tool.rs` test helper, `crates/tui/src/render/popup_scene_tests.rs` seed helper.

4. **Popup rendering** — `crates/agent_tui_kit/src/render/popups/subagent_popup.rs`
   - `prepare_subagent_popup`:
     - Gather `(sections, prompt, content_len)` from live buffer / completed block (with `detail_full` legacy fallback as one Context block).
     - Group into canonical order Thinking / Tools / Context, skip empty; strip `THINKING_SECTION_HEADER` from thinking blocks.
     - Build document: live → plain styled lines with headers + prompt label; completed → `## header` markdown with `**Prompt:**`, then `render_markdown_with_tables` + `decorate_headings`.
     - Headers only when ≥2 non-empty sections.
     - Feed `plan_markdown_display` + `PopupLayoutCache` exactly as today.

5. **Tests** — `crates/tui/src/render/popup_scene_tests.rs`
   - Update `seed_subagent_popup` with `detail_sections` (None) so existing tests exercise the legacy fallback.
   - Add: sectioned completed popup shows headers in order + bodies + prompt under Context; single-section transcript has no headers; live mode renders headers; blank rows carry `theme.bg` (buffer-level).
   - Existing markdown-decoration tests must keep passing.

6. **Docs**
   - `book/26_chapter_issue.md` + `_zh.md`: newest-first 2026-08-24 entry (sectioned subagent popup; link spec + plan).
   - Check `book/23_chapter_tui*.md` for subagent popup description; update if it describes the flat transcript.

## Gates

- `env -u http_proxy -u https_proxy -u all_proxy cargo test -p tact -p tact-protocol -p agent_tui_kit -p tact-ui` focused on touched modules first, then the full package gates.
- `cargo fmt --check` / clippy on touched crates.
- One cargo invocation at a time.
