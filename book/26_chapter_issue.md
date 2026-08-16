# Engineering Issue Log

> Language: [English](./26_chapter_issue.md) · [中文](./26_chapter_issue_zh.md)

This chapter is a **chronological log of optimizations and bug fixes** that changed user-visible or API-visible behavior. It is not a tutorial: each entry records the problem, the decision, and where the code / design docs live so future work does not rediscover the same trade-offs.

Related process docs: `AGENTS.md` (when to append here), `docs/superpowers/specs/` (design), `docs/superpowers/plans/` (implementation plans).

---

## 0. Purpose

| Goal | Detail |
|------|--------|
| Continuity | Capture *why* a change landed, not only *what* files moved |
| Cross-link | Point at design specs, PRs, and book chapters that teach the subsystem |
| Avoid churn | Prefer one entry per shipped behavior change; do not log pure refactors or test-only edits |

### Entry template

Newest entries first. Each entry should include:

1. **Date / ID** — `YYYY-MM-DD` and optional PR number  
2. **Type** — `optimization` · `bugfix` · `removal` · `docs`  
3. **Symptom / motivation** — what was wrong or expensive before  
4. **Decision** — the chosen contract (not discarded alternatives in full)  
5. **Behavior after** — observable rules agents and users rely on  
6. **Pointers** — code paths, specs, related book chapters  

---

## 1. 2026-08-16 — Task-stats line is localized and drops the wide 📊 icon

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | Ch 23; `crates/tui/src/i18n.rs`, `crates/tui/src/widgets/state/app/messages.rs`, `crates/tui/src/handlers/mouse.rs` |

**Symptom / motivation:** The per-turn stats row was hardcoded as `📊 任务统计：…` (`TASK_STATS_PREFIX` in `messages.rs`), so it stayed Chinese even in English UI mode, and the wide `📊` emoji rendered too large in the log. The `[copy]` affordance was also a hardcoded English string.

**Decision:** The prefix and copy button moved into the i18n `Messages` table: EN `Task stats:` / `[copy]`, ZH `任务统计：` / `[复制]`, with no emoji icons — the wide `📊` prefix and the `🧠` before the model name were both removed. The copy affordance renders **before** the stats body; only clicks inside the button glyphs trigger the copy (the rest of the row text-selects normally). `add_task_stats_block` reads them via `self.msgs()`; `is_task_stats_line` now recognizes every supported language's prefix with an optional leading button **plus** the legacy `📊 任务统计：` rows (so `[copy]` keeps working on sessions persisted before this change); the mouse handler finds the localized button's byte range via `find_task_stats_copy_button`.

**Behavior after:** The stats row renders as `[copy]  Task stats:⏱ mm:ss · model · N tokens …` in English and `[复制]  任务统计：⏱ mm:ss · model · N tokens …` in Chinese, with no wide emoji prefix or model icon. Old sessions' stats rows remain copyable.

**Pointers:** `crates/tui/src/i18n.rs` (`task_stats_prefix`, `task_stats_copy_btn`), `crates/tui/src/widgets/state/app/messages.rs` (`is_task_stats_line`, `find_task_stats_copy_button`, `add_task_stats_block`), `crates/tui/src/handlers/mouse.rs`; tests `task_stats_block_localizes_prefix_and_copy_button`, `task_stats_line_detection_covers_all_languages_and_legacy_rows`.

---

## 1. 2026-08-16 — `install.sh` no longer errors on `tmp: unbound variable` or leaks the clone dir

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | `scripts/install.sh` |

**Symptom / motivation:** Under `set -u`, the installer printed `bash: line 351: tmp: unbound variable` after a successful release install. `try_install_release` set `trap 'rm -rf "$tmp"' RETURN` on a `local tmp`; the RETURN trap is inherited and fires again when the caller `main` returns, where `tmp` is out of scope, so the unguarded `$tmp` expansion aborted with `nounset`. Separately, `main` declared `work` as a `local` and cleaned it with an `EXIT` trap; that trap fires at script exit after `main`'s locals are gone, so `${work:-}` always saw an empty value and the `git clone` directory leaked on the from-source / non-repo path.

**Decision:** (1) The release tmpdir trap is now `trap '[[ -n "${tmp:-}" ]] && rm -rf "$tmp"' RETURN` — the guard makes the inherited caller-context firing a no-op while still cleaning up on `try_install_release`'s own return. (2) `work` is no longer `local`; it is a global initialized to `""` so the existing `EXIT` trap actually removes the clone directory at script exit (including the `die` / `exit 1` paths).

**Behavior after:** `curl … | bash` (and `./scripts/install.sh`) finishes with `Done. Run: tact-ui --help`, exit 0, no `unbound variable` error, and both the release tmpdir and the cloned repository directory are removed.

**Pointers:** `scripts/install.sh` (`try_install_release`, `main`).

---

## 1. 2026-08-16 — `plugin install` no longer panics and parses the official `url` plugin sources

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | Ch 02; `crates/tact-ui/src/main.rs`, `crates/tact/src/plugin/marketplace.rs` |

**Symptom / motivation:** `tact-ui plugin install <plugin>@claude-plugins-official` failed in two ways. First, the main entrypoint's early self-upgrade check used `args.command.take()`, which consumed *any* non-`Upgrade` command and left `args.command = None`; `Plugin` (and `Headless`) then fell through to `run_interactive`, and because `plugin` commands resolve config without an LLM provider (`install_without_llm`), the interactive path panicked at `get_provider()` with "LLM provider not initialized". Second, the official `anthropics/claude-plugins-official` catalog uses a `"source": "url"` object form (150 of its 286 plugins: `url` + `sha`, some with `path`) that `PluginSource::from_catalog_value` did not understand, so the whole catalog failed to parse with "invalid marketplace source: url".

**Decision:** (1) The upgrade early-return now checks with `matches!(args.command, Some(CliCommand::Upgrade { .. }))` and only `take()`s inside the matched branch, so non-upgrade commands survive to the later dispatch. (2) `from_catalog_value` treats both `git-subdir` and `url` as Git repository sources (clone `url`, optional `path`, pinned revision), preferring the `sha` pin and falling back to `ref`.

**Behavior after:** `plugin install` (and `headless`) dispatch correctly; `plugin install frontend-design@claude-plugins-official` clones the official marketplace, parses all 286 entries, and installs `frontend-design` (1 skill) at its pinned revision. Plugin commands never require an LLM provider.

**Pointers:** `crates/tact-ui/src/main.rs` (upgrade early-return), `crates/tact/src/plugin/marketplace.rs` (`RawPluginSource::Object.sha`, `PluginSource::from_catalog_value`); tests `parses_url_plugin_source`, `parses_url_plugin_source_with_subdirectory`, `git_source_falls_back_to_named_ref_without_a_sha`; spec `docs/superpowers/specs/2026-07-20-plugin-install-design.md`.

---

## 1. 2026-08-16 — Overlay list popups stay inside the main area

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | Ch 23; `crates/tui/src/lib.rs`, `crates/tui/src/render/test_harness.rs` |

**Symptom / motivation:** The four overlay popups rendered from the main frame loop (`command_palette`, `select`, `file_picker`, `slash_command`) were centered on the **whole frame** with a height cap of `frame.height - 4`. With a long command/file/option list on a short terminal the popup grew past the log panel and covered the command-line input box and bottom bar; popup list rows interleaved with the input box's border glyphs (outside the popup's `Clear` rect) and read as a shadow-like mess, and the user could no longer see the command being typed.

**Decision:** The popup call sites pass the **main area** (`chunks[1]`: status bar below, input box above) instead of the full frame. Popups now center and cap their height inside the main area only.

**Behavior after:** Palette / select / file-picker / slash popups always stay above the input box and bottom bar regardless of list length or terminal height; the input chrome stays fully visible while filtering.

**Pointers:** `crates/tui/src/lib.rs` (frame loop popup calls), `crates/tui/src/render/test_harness.rs` (`draw_full_ui`, kept in sync), `crates/tui/src/render/popup_scene_tests.rs` (`full_frame_palette_popup_stays_inside_main_area`); Ch 23.

---

## 1. 2026-08-16 — Main-area headings no longer paint the highlight band

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | Ch 23; `crates/tui/src/render/log_style.rs` |

**Symptom / motivation:** The restyle pass (added in the pulldown-cmark migration, #69) painted H1 headings with `theme.highlight` as their background — a leftover of tui-markdown's direct H1 styling. In the log panel the band only covered the glyph columns of the heading, and on a wrapped heading every wrapped row carried the band, so a long heading plus its list read as a multi-row shadow block behind the text. The whole-Markdown path (`MarkdownCell`, e.g. `/skills` pages) never painted the band, so the two main-area paths disagreed.

**Decision:** The restyle pass no longer assigns any background to heading spans (the pulldown renderer emits headings backgroundless). The legacy `Color::Rgb(70, 90, 140)` → `theme.highlight` remap (dead since the fork was dropped) was removed with it.

**Behavior after:** H1 headings render as bold+underlined heading-color text with no background in both main-area paths; no highlight band can appear behind (wrapped) list headings.

**Pointers:** `crates/tui/src/render/log_style.rs` (`restyle_log_line_with_skills`, `heading_keeps_no_background`), `crates/tui/src/render/log_render_tests.rs` (`heading_rows_carry_no_highlight_band`); Ch 23 render pipeline.

---

## 1. 2026-08-15 — `/stats` popup renders through ratatui-markdown directly

| Field | Value |
|-------|-------|
| Type | `optimization` |
| Symptom / motivation | The system-prompt popup (shared by `/stats` session stats and the assembled-system-prompt view) went through the pulldown-cmark pipeline with Tact's width-aware pipe-table pass, laid out at the popup's content width. For a quick statistics popup that extra layout machinery was not worth it. |
| Decision | `render_system_prompt_popup` now renders via `render_markdown_ratatui` (`crates/tui/src/render/render_md.rs`): a plain `ratatui_markdown::markdown::MarkdownRenderer` at the popup's content width, using the same `TuiRichTextTheme` as the Mermaid renderer. The width-aware table pass and Mermaid routing stay for the main-area Markdown cells. |
| Behavior after | The `/stats` and system-prompt popups are laid out by ratatui-markdown's default renderer (tables included); popup tests (`session_stats_popup_renders_gfm_table`) pass unchanged. |
| Pointers | `crates/tui/src/render/popups/system_prompt_popup.rs`, `crates/tui/src/render/render_md.rs` (`render_markdown_ratatui`); `docs/token_usage_schema.md` Session Stats Display. |

---

## 1. 2026-08-15 — Auto-compaction no longer enables thinking on the summary call

| Field | Value |
|-------|-------|
| Type | `optimization` |
| Symptom / motivation | The local compaction summary call forwarded the agent's Claude-style `thinking_budget` (`with_thinking`, clamped below the wire `max_tokens`) and its explicit `reasoning_effort`, and reserved an effort-tiered reasoning share on top of the text budget. Thinking on a handoff summary adds little value and consumes output tokens from the same `max_tokens` envelope (effort models) — the user asked to stop enabling it during auto-compaction. |
| Decision | The summarizer request no longer carries any thinking: no `thinking` block and no `reasoning_effort` are forwarded (main-loop thinking config is untouched), and the input reservation no longer subtracts a thinking budget. The **server-default** reasoning reserve stays for DeepSeek / Kimi K3 only (fixed 75% of the text budget on top): they default thinking ON + effort high server-side even when the request omits effort, so without the reserve their forced reasoning would starve the summary text and force truncation continuations. The native `/responses/compact` request was already `{model, input}`-only; its dead `.with_reasoning_effort` was removed. |
| Behavior after | The compact summary call is a plain non-streaming `create_message` with `max_tokens` = classic text budget (OpenAI / Anthropic) or text + 75% reserve (DeepSeek / Kimi K3); `AgentUpdate::ModelInfo` emitted during compaction reports no thinking/effort. |
| Pointers | `crates/tact/src/agent/mod.rs` (`compact_history_local_with_mode`, `compact_summary_reasoning_reserve_percent`, `compact_responses_native`), book [Ch 5](./05_chapter_compact.md) §summarization call. |

---

## 1. 2026-08-15 — Bottom-bar `out` renamed to `max_out_token` with the real output budget

| Field | Value |
|-------|-------|
| Type | `optimization` |
| Symptom / motivation | The bottom-bar output segment was labeled `out`/`输出` and showed the raw `max_tokens` envelope. For effort-semantic models (openai / deepseek / kimi k3) reasoning counts inside the same envelope as output text, so `out 128K` next to `think high` overstated the tokens actually left for text — the user asked for the segment to show the **max output token** value, with the reasoning share subtracted. |
| Decision | Rename the label to `max_out_token` (language-invariant, both locales) and make the value the effective text-output budget: for effort-semantic models subtract the reasoning share using the same tier convention as the compaction reserve (percent of the text budget added on top → text = envelope × `100/(100+pct)`; `high` → 73K on a 128K envelope). Budget-semantic models (Anthropic-style `thinking_budget`) keep thinking in a separate envelope, so they show the full `max_tokens`. Computed in the TUI from `status_bar.model_max_tokens` + `model_thinking_budget` + `model_reasoning_effort`; no protocol change. |
| Behavior after | Bottom-bar row 2 shows `max_out_token {n}` instead of `out {n}`; `n` equals `max_tokens` for budget/no-think states and `max_tokens × 100/(100+pct)` when an effort is displayed (`none`/no effort → unchanged, `low` → 80%, `medium` → ~67%, `high` → ~57%, `xhigh`/`max` → 50%). |
| Pointers | `crates/tui/src/render/bar.rs` (`format_max_out_tokens`), `crates/tui/src/i18n.rs` (`bottom_out`), book [Ch 23](./23_chapter_tui.md) §6.6, `docs/token_usage_schema.md`. |

---

## 1. 2026-08-15 — Model→context-window mapping overrides manual `model_context_window` config

| Field | Value |
|-------|-------|
| Type | `optimization` |
| Symptom / motivation | `agent.model_context_window` was purely manual (CLI/TOML, default `200_000`) with no model inference. With `deepseek-v4-pro` (real window 1M) the bottom-bar `ctx` meter showed `…/256K` from a stale 256k config and auto-compaction fired at ~80% of that (~205k) instead of ~800k, causing premature compaction. `max_tokens` already had a model-based default (`kimi_k2x → 32_000`) to copy; the window had no equivalent. |
| Decision | Add `model_context_window_for_model(model)` in `resolve.rs` and resolve the window as: **model→window mapping (highest) → CLI/TOML → default `200_000`**. Values follow official model docs (2026-08): OpenAI `gpt-5.6` family + `gpt-5.5` → `1_050_000`, `gpt-5.4` → `1_000_000`, `gpt-5`…`gpt-5.3`/`gpt-5.4-mini` → `400_000`, `gpt-4o` family → `128_000`; Anthropic (API + Claude Code) `claude-sonnet-5`/`claude-fable-5`/`claude-opus-5`/`claude-opus-4-8`/`claude-opus-4-7`/`claude-opus-4-6`/`claude-sonnet-4-6` → `1_000_000`, `claude-sonnet-4-20250514`/`claude-opus-4-20250514`/`claude-haiku-4-5`/`claude-haiku-4-20250514` → `200_000`; DeepSeek V4 → `1_000_000`, `k3-256k` → `256_000`. A mapping match deliberately overrides user file config so a stale manual window can never under-report a well-known model. |
| Behavior after | The `ctx` bottom-bar meter and the derived auto-compact threshold (80% of window) use the mapped window for the mapped models. GPT-5.6/5.5 models show `…/1.05M`, Claude 1M models (incl. Claude Code ids) `…/1M`, DeepSeek V4 `…/1M`, GPT-5.x `…/400K`, GPT-4o `…/128K`, `k3-256k` `…/256K`. Manual `model_context_window` only takes effect for models without a built-in mapping. The nonzero `model_context_window > max_tokens` validation still applies to the resolved value. |
| Pointers | `crates/tact/src/config/resolve.rs` (`model_context_window_for_model`, resolution at ~`:587`); `config.example.toml` `[agent]`; book [Ch 21](./21_chapter_config.md) §5, [Ch 5](./05_chapter_compact.md) §settings tables. |

---

## 1. 2026-08-15 — Markdown body moves to pulldown-cmark; ratatui-markdown kept for Mermaid only

| Field | Value |
|-------|-------|
| Type | `optimization` |
| Plan | `docs/superpowers/plans/2026-08-15-pulldown-cmark-migration.md` |
| Symptom / motivation | The consolidation below landed on a local `ratatui-markdown` fork carrying ~350 lines of patches across 8 files (H4–H6, ordered-list numbers, hard breaks, nested emphasis, CJK flanking, themed code slots) purely so Tact's prose renderer could match `tui-markdown`'s output. The fork is a rebase burden and a path dependency that only resolves on this machine; `steer` and xAI's `grok-build` both parse CommonMark with `pulldown-cmark` and render in their own code instead of forking a Markdown crate. |
| Decision | Replace the fork's block renderer with a `pulldown-cmark` 0.13 event loop (`crates/tui/src/render/pulldown.rs`) that reuses Tact's width-aware pipe-table `format_table`, the `▎` blockquote gutter, fenced-code styling, and Mermaid routing. `ratatui-markdown` becomes an upstream git dependency (`celestia-island/ratatui-markdown` @ `3a8bcbe`, `mermaid` feature) used only for non-sequence Mermaid diagrams; `sequenceDiagram` stays in Tact's own `mermaid_sequence.rs`. The `feat/tact` fork and the `TuiRenderHooks`/`RenderHooks` adapter are removed. |
| Behavior after | Prose/heading/list/task/table/blockquote rendering is owned by Tact from `pulldown-cmark` events. `ENABLE_SMART_PUNCTUATION` is left off so `...` is never turned into `…` (system messages and user text stay byte-stable). GFM task lists now apply to ordered lists too (`1. [X]` → `1. ☑`). Mermaid output is unchanged. |
| Pointers | `crates/tui/src/render/pulldown.rs`, `render_md.rs`; `Cargo.toml` (`ratatui-markdown` git dep + `pulldown-cmark`); book [Ch 23](./23_chapter_tui.md) §6.7; the entry below records the intermediate fork approach. |

---

## 1. 2026-08-15 — Main-area Markdown consolidated onto ratatui-markdown

| Field | Value |
|-------|-------|
| Type | `optimization` |
| Plan | `docs/superpowers/plans/2026-08-15-ratatui-markdown-migration.md` |
| Symptom / motivation | The TUI main area maintained two Markdown stacks in parallel: `tui-markdown` 0.3.x (crates.io) rendered prose / headings / lists in the log panel, while `ratatui-markdown` (celestia-island git fork, branch-pinned) rendered Mermaid and the `/tasks-dag` popup. Two style adapters, two color palettes, and fork workarounds (task-list marker escaping, hardcoded-color remaps in `log_style.rs`, fence-marker bookkeeping) had to be kept in sync. |
| Decision | Consolidate on `ratatui-markdown` (local fork at `../ratatui-markdown`, branch `feat/tact`, based on `chore/update-ratatui-0.30` @ `3a8bcbe`) with capability patches: H4–H6 headings, ordered-list numbers preserved, 4-column-per-level nested indent, recursive nested emphasis (`**bold _x_ italic**`), link URL suffixes, themed inline-code / fenced-code color slots, soft-break collapse + hard-break preservation, space-run preservation, and CJK-aware emphasis flanking. Tact side: `render_plain_markdown` swaps `tui_markdown::from_str_with_options` for fork parse+render with a `TuiRenderHooks` (fences / box chrome hidden, code background applied directly); the blockquote `▎` gutter and the H1 highlight background moved to post/restyle passes; tables render through a `table` RenderHooks adapter that delegates to Tact's width-aware `format_table` (pipe style), so the fork's own `render_table` is unused and kept upstream; triple-click code-block detection now matches the code background instead of ````` ``` ````` markers; the diff popup highlights via `syntect` directly (same Base16 Ocean Dark theme), so `tui-markdown` and the direct `pulldown-cmark` dep are removed. |
| Behavior after | The log panel renders Markdown through one crate. Bullets render as `•` and task items as `☐` / `☑` (previously the literal `-` / `[ ]` text); fenced code keeps the themed background without fence markers; raw copy rows mirror the rendered text (markers are consumed at parse time); `/stats` renders its GFM table as a pipe table via `format_table`; the diff popup keeps syntax highlighting. |
| Pointers | `crates/tui/src/render/render_md.rs` (`TuiRenderHooks`, `render_plain_markdown`, `apply_blockquote_indicator`), `crates/tui/src/render/log_style.rs` (H1 highlight rule), `crates/tui/src/render/popups/diff_popup.rs` (syntect), `crates/tui/src/widgets/state/app/popups.rs` (`find_code_block_containing_logical`), `Cargo.toml` (`ratatui-markdown` path dep + `syntect`); fork repo `../ratatui-markdown` `feat/tact`; [Ch 23](./23_chapter_tui.md) §6.7. |

---

## 1. 2026-08-15 — Thinking, command-output, and read cards drop the redundant line count from their top titles

| Field | Value |
|-------|-------|
| Type | `removal` |
| Symptom / motivation | The Thinking card showed the total line count twice — once in the top title (`🧠 Thinking (N lines)`) and again in the bottom bar (`↕ visible/N lines …`) — and the `bash` command-output card repeated it too (top `Live output (N lines)` / `Command output (N lines)` vs. the bottom `preview/total lines` hint), as did the `read_file` card (top `Read <path> (N lines)`). Whenever both were visible, the top count duplicated the bottom bar's number. |
| Decision | Card top titles no longer carry counts: `🧠 Thinking` (active and completed), `Live output` (running bash), `Command output` (completed bash), `Read <path>` (read_file). The bottom bars remain the single count source (`↕ visible/total lines` on Thinking; `preview/total lines` when the command output overflows the preview). Removed the now-unused `thinking_card_title_pl` field, and `tool_live_output_title_tmpl` lost its `{}` placeholder and was renamed `tool_live_output_title`. |
| Behavior after | Thinking cards read `🧠 Thinking` / `🧠 思考中`; live bash cards read `Live output` / `实时输出`; completed command cards read `Command output`; read cards read `Read <path>`. All line counts live in the card bottom bars. Popup titles unchanged (they already used the command text or bare `Command output`). |
| Pointers | `crates/tui/src/i18n.rs`, `crates/tui/src/render/cells/thinking.rs`, `crates/tui/src/widgets/tool_widget.rs` (`detail_card_title`), `crates/tui/src/render/cells/tool.rs` (`card_bottom_text`); tests `live_output_total_excludes_command_prefix_but_popup_keeps_it`, `log_tool_card_renders_when_scrolled_into_placeholder_rows`; [Ch 23](./23_chapter_tui.md) §render pipeline. |

## 1. 2026-08-15 — Log word-wrap at word boundaries; selection UX made symmetric

| Field | Value |
|-------|-------|
| Type | `optimization` |
| Symptom / motivation | (1) `wrap_line` hard-cut every visual line at the exact display width (`split_at_display_width`), so long URLs, paths and words split mid-word with no continuation hint; (2) wrapped partial selections lost the REVERSED overlay on continuation lines because the wrapped path flattened every span to one base style; (3) double-click word selection only understood ASCII, so double-clicking 中文 selected nothing; (4) selection UX was asymmetric: clicking a whole-Markdown row (or dragging into one) created an invisible selection (the MarkdownCell renderer draws no overlay), and clicking empty space / outside the panel kept a stale selection; (5) mouse hit-testing simulated hard wraps at panel width while rendering wrapped at width − indent, drifting up to the indent width on indented rows. |
| Decision | A shared `wrap_break_offsets` computes visual-line start offsets once and both `wrap_line` and `visual_pos_to_byte_offset` consume it, so render and hit-test cannot disagree. Wrap is greedy word wrap: break at the last whitespace run that fits; hard-cut only when an unbroken word exceeds the line; trailing whitespace rides on the previous line (invisible) so segments stay contiguous. `wrap_line` now re-slices the original styled spans per segment, so per-span styles (REVERSED included) survive onto continuation lines. `find_word_bounds` classifies the char under the cursor into ASCII-word or CJK-run (Han/kana/Hangul) and expands within that class. `handle_log_click`/`handle_mouse_drag`/`handle_log_triple_click` refuse to start or extend a selection on Markdown rows, and clicks on empty space below the log or anywhere outside the panel clear the selection. Hit-testing subtracts the row indent from the wrap width. |
| Behavior after | Words no longer break mid-word (URLs/paths/CJK stay intact until they genuinely exceed the line); selection highlight stays visible across every wrapped line; double-click selects whole 中文 runs; Markdown cards are never silently "selected"; stray clicks clear stale selections instead of keeping them; clicks on indented rows map to the right byte. |
| Pointers | `crates/tui/src/render/util.rs` (`wrap_break_offsets`, `wrap_line`, `visual_pos_to_byte_offset`, `col_to_byte_offset`), `crates/tui/src/widgets/state/app/visibility.rs` (`find_word_bounds`, `is_markdown_row`, `byte_offset_from_log_position`), `crates/tui/src/handlers/mouse.rs` (click/drag/triple-click guards + outside-click clear), `crates/tui/src/render/cells/text.rs`; tests `wrap_break_offsets_prefers_word_boundaries`, `wrap_line_keeps_word_intact_and_preserves_span_styles`, `wrap_break_offsets_agree_with_byte_offset_hit_testing`, `partial_selection_reverses_target_span_across_wrapped_lines`, `double_click_selects_cjk_run`, `click_below_last_message_clears_selection`, `click_on_markdown_row_does_not_create_invisible_selection`, `drag_into_markdown_row_does_not_extend_selection`, `click_outside_log_clears_selection`; [Ch 23](./23_chapter_tui.md) §render pipeline. |

## 1. 2026-08-15 — Log scroll becomes visual; `/skills` paginated

| Field | Value |
|-------|-------|
| Type | `bugfix` |
| Symptom / motivation | The log panel scrolled in logical-message-row units (`log_scroll.offset ± 1` per `j`/`k`/wheel tick). A whole-Markdown message taller than the viewport — e.g. the `/skills` pipe table with ~60 skills ≈ 400+ rendered lines — therefore exposed only its first and last screenful: `resolve_visual_scroll` bottom-pinned at the max logical offset, so the middle rows (where `lark-*` sorts alphabetically) were unreachable in both directions. |
| Decision | The viewport's first visible **visual** line is now authoritative (`LogScroll.visual_top`, `usize::MAX` = pin-to-bottom sentinel); `offset` becomes a derived logical mirror for read-only consumers (mouse hit-testing, code popup). Pure step functions `visual_step_up/down` move half a viewport per `j`/`k` (3 lines per wheel tick) *inside* a cell taller than the viewport and jump across row boundaries otherwise; entering a tall row from below lands on its bottom so upward traversal stays continuous. `resolve_visual_scroll` / `effective_max_logical_scroll` deleted. `/skills` output is additionally paginated into 15-skill chunks, each a separate Markdown message with a `(n/k)` heading. |
| Behavior after | Any cell taller than the viewport (long tables, expanded tool cards) is fully traversable in both directions with `j`/`k`/wheel; `g`/`G` still jump to top/bottom, and auto-follow-the-stream keeps working (`is_log_pinned_to_bottom` compares visual positions). `/skills` renders 15 skills per page with numbered headings. |
| Pointers | `crates/tui/src/widgets/state/app/scroll.rs` (step functions + scroll API), `crates/tui/src/widgets/state/log_scroll.rs` (`visual_top`), `crates/tui/src/render/log.rs` (visual clamp + mirror derivation), `crates/tui/src/handlers/{normal,mouse,mod}.rs` (keys, wheel, `/skills` pagination), `crates/tui/src/widgets/state/app/{agent,messages,visibility}.rs` (pin helpers); regression tests `tall_markdown_cell_is_fully_traversable`, `skills_command_paginates_long_lists`; [Ch 23](./23_chapter_tui.md) §render pipeline. |

## 1. 2026-08-15 — Main-area render polish: markdown indent, theme links, code backgrounds, hidden markers

| Field | Value |
|-------|-------|
| Type | `bugfix` |
| Symptom / motivation | Render-path review found: (1) whole-Markdown messages (`MarkdownCell`, e.g. `/skills`) hugged the left border while streamed replies/tool cards are indented; (2) links used hardcoded palette `Blue` that never adapted to the theme; (3) `is_user_message_line` walked back to the block start per rendered row — quadratic for a long pasted user block; (4) `TextCell` flattened every span's background onto the surface color, so fenced code in streamed replies lost its background (inconsistent with `MarkdownCell`); (5) raw Markdown markers (`# `, `> `, ``` fences) leaked into the rendered text. |
| Decision | (1) `append_markdown` applies the same `LOG_THINKING_INDENT + 1` gutter; (2) links use `theme.heading`, with legacy `Blue` remapped in the restyle pass; (3) a one-pass `user_line_mask` is precomputed per frame and shared by restyle + indent; (4) `TextCell` keeps span-provided backgrounds (code bg, H1 highlight) and the restyle pass only treats code-bg spans as code; (5) styled lines hide fence rows (blank) and strip `#{1,6} ` / `> ` prefixes while `raw_messages` keep the original Markdown for copy, code-block detection and hit-testing. Streamed text also uses the same fg as final rows so completing a reply does not recolor it. |
| Behavior after | Code blocks in streamed replies show their background; H1 keeps its highlight band; quotes render as `▎ text`; headings render without `## `; links adapt per theme; long pastes no longer trigger quadratic row walks; `/skills` and other Markdown notices align with replies. |
| Pointers | `crates/tui/src/render/{log.rs,log_style.rs,render_md.rs}`, `crates/tui/src/render/cells/{text.rs,markdown.rs}`, `crates/tui/src/widgets/state/app/{popups.rs,visibility.rs}`; tests `span_backgrounds_survive_rendering`, `heading_keeps_no_background`, `user_line_mask_matches_the_per_row_walk`, `hardcoded_blue_links_remap_to_theme_heading`, `render_markdown_fenced_code_block`, `render_markdown_heading_markers_are_stripped`, `indented_cell_shifts_content_right`; [Ch 23](./23_chapter_tui.md) §render pipeline. |

## 1. 2026-08-14 — Cron scheduling feature removed

| Field | Value |
|-------|-------|
| Type | `removal` |
| Symptom / motivation | The cron feature (`cron_create` / `cron_list` / `cron_delete`) only persisted schedule records; nothing ever evaluated the expressions or injected the stored prompts into `agent_loop`. Users who asked for a reminder got a false sense of security — the record existed but would never fire. Building an in-process tick loop was deemed wrong-headed: it would require the interactive TUI process to stay resident and would duplicate the system cron that already runs reliably. |
| Decision | Remove the entire feature: `crates/tact/src/cron/` (scheduler + simulation), `crates/tact/src/store/cron_store/` (trait + SQLite impl), `crates/tact/src/tool/cron.rs` (tools), `ToolContext.cron_scheduler` field, registry routes, startup wiring in `headless.rs` / `interactive.rs`, the `cron_tasks` table row in docs, the TUI tool-name map, and tests. Book chapter 16 (EN/ZH) deleted; chapter 1 / 4 / 7 / 12 / 14 / 15 / 19 / 23, `index.md`, `mindmap.md`, and `ARCHITECTURE.md` cron references cleaned up. |
| Behavior after | No `cron_*` tools; the model can no longer create scheduled prompts. Existing `cron_tasks` rows and the legacy `.tact/cron/` files are left untouched on disk (dead data, removable manually). |
| Pointers | Removed files: `crates/tact/src/cron/*`, `crates/tact/src/store/cron_store/*`, `crates/tact/src/tool/cron.rs`, `book/16_chapter_cron*.md`; edited: `crates/tact/src/lib.rs`, `crates/tact/src/tool/{mod,registry}.rs`, `crates/tact/src/tool/test_support.rs`, `crates/tact/src/store/mod.rs`, `crates/tact-ui/src/{headless,interactive}.rs`, `crates/tact-ui/tests/{subsystem_tools.rs,harness/mod.rs}`, `crates/tui/src/widgets/tool_widget.rs`, `book/01_chapter_store*`; [Ch 1](./01_chapter_store.md), [Ch 7](./07_chapter_tool.md). |

## 1. 2026-08-14 — Background output stored hybrid: full log file + `output_path` on the record

| Field | Value |
|-------|-------|
| Type | `optimization` |
| Symptom / motivation | `background_run` output lived only in the `background_tasks` DB record, capped at the first 50,000 chars — and the cap kept the *beginning* of long logs (usually the least useful part) while dropping the ending where errors live. The model had no way to deep-inspect a big output: polling `check_background` pulled the whole ≤50k JSON into context, and `grep`/`tail` were useless because there was no file. |
| Decision | Hybrid storage: the DB record keeps metadata + the first 50k chars (unchanged, cheap to poll), while the **full** stdout+stderr stream is appended as it arrives to `<workdir>/.tact/background/<id>.log`. `BackgroundTaskRecord` gains `output_path` (SQLite column `output_path TEXT NOT NULL DEFAULT ''`, added via `PRAGMA table_info` + `ALTER TABLE` migration for existing DBs). Log-file creation is best-effort — failure degrades to the capped record only. The `check_background` listing appends `(log: <path>)` per line. |
| Behavior after | Polled JSON includes `output_path`; the agent can `bash tail <path>` / `grep error <path>` on the full log instead of ingesting a 50k blob. The log file exists from the moment the task starts (record is written with the path before spawn), so a long-running task can be inspected live. |
| Pointers | `crates/tact/src/background.rs` (`BackgroundTaskRecord.output_path`, `open_log_file`, `log_write`, `run_background_process`), `crates/tact/src/store/background_store/sqlite.rs` (schema + migration + upsert/read), `crates/tact/src/tool/background_run.rs` (listing); tests `run_writes_full_output_to_log_file_and_truncates_db_record`, `migrates_legacy_table_without_output_path`; [Ch 13](./13_chapter_background.md) §2, §3, §6, §8; [Ch 1](./01_chapter_store.md). |

## 1. 2026-08-13 — Plan-mode read-only shell classification hardened against newline command separators

| Field | Value |
|-------|-------|
| Type | `bugfix` |
| Symptom / motivation | `split_plain_command` skipped `\n` / `\r` as plain whitespace, but to `sh -c` a bare newline (and `\r` in CRLF input) is a command separator, not a word separator. A command like `ls\nrm file` therefore passed the plain-command split, classified the safelisted first word `ls` as Read, and was auto-allowed in plan mode — letting a second, mutating command ride along silently. |
| Decision | The splitter now returns `None` as soon as it meets a bare `\n` / `\r` while scanning separators, so any newline-separated multi-command string stays unclassified (falls back to `Write` / prompt). A literal newline inside single or double quotes is still a word character and remains accepted. In the same pass, git global-option handling was unified into a single `GIT_GLOBAL_OPTIONS` table (each entry carrying whether it consumes the next token) that drives both `find_git_subcommand`'s skip logic and `git_has_unsafe_global_option`, so the two checks cannot drift apart again. |
| Behavior after | `ls\nrm file`, `echo hi\nrm -f x`, CRLF variants and leading/trailing bare newlines are all classified **Write** (prompted / denied in plan mode); `echo "line1\nline2"` and `cat "file\nname"` (quoted literal newlines) remain Read. |
| Pointers | `crates/tact/src/tool/readonly_shell.rs` (`split_plain_command`, `GIT_GLOBAL_OPTIONS`, `find_git_global_option`, `git_has_unsafe_global_option`); regression tests in the same file; [Ch 10](./10_chapter_permission.md) §7. |

## 1. 2026-08-13 — OpenAI-compatible Chat Completions surfaces transport failures as `LlmError::Request`

| Field | Value |
|-------|-------|
| Type | `bugfix` |
| Symptom / motivation | The OpenAI-compatible adapter reported send/connection/response-read failures as `LlmError::Unsupported("HTTP request failed: …")`, conflating "this endpoint can't do something" with "the request never went through"; token counts were cast `u64 as u32` (truncating, misleading on oversized values); a malformed tool-call `arguments` payload was silently replaced by `{}` with no trace. |
| Decision | New `LlmError::Request(String)` variant for request transport / deserialization errors (API HTTP errors keep `HttpError`). Both streaming and non-streaming paths now send the already-serialized JSON bytes (`body` + explicit `Content-Type`) instead of re-serializing via `.json()`. Token counts convert `u64 → u32` saturating (`u32_token_count`). Malformed tool-call arguments log at `debug` (error, tool name, raw args) and fall back to an empty object. |
| Behavior after | A dead endpoint / dropped connection surfaces `request error: …` instead of `unsupported: …`; oversized token counts saturate instead of wrapping; malformed tool args are visible in debug logs. |
| Pointers | `crates/tact_llm/src/error.rs` (`LlmError::Request`), `crates/tact_llm/src/openai/compatible/mod.rs` (`OpenAiAdapter` chat/stream paths, `u32_token_count`, `tool_use_block_from_parts`); [Ch 22](./22_chapter_llm.md). |

## 1. 2026-08-13 — TUI input box soft-wraps long lines and maps the caret through wrapped rows

| Field | Value |
|-------|-------|
| Type | `bugfix` |
| Symptom / motivation | Input-box height and line stats counted only explicit `\n` splits, so a single overlong line overflowed past the box's 3-row cap, and the caret/scroll math (logical rows) disagreed with the rendered text (caret drawn on the wrong row/column for long input). |
| Decision | `render/input.rs` gained `wrap_line` (character-boundary soft-wrap, CJK double-width aware; `Paragraph` stays unwrapped and draws exactly those rows) and `caret_in_wrapped` (logical cursor column → display row/column). Box height, line stats, scroll clamping, and cursor placement all now operate on display rows. |
| Behavior after | Long input lines wrap inside the box instead of overflowing; height auto-expands with wrapped rows (1–3 display rows + border); caret and scroll follow the wrapped row. Submitted text is unchanged. |
| Pointers | `crates/tui/src/render/input.rs` (`wrap_line`, `caret_in_wrapped`); `crates/tui/src/lib.rs` (input height); tests in `input.rs` (`wrap_line_splits_at_column_width`, `caret_in_wrapped_maps_logical_column_to_display_row`, `input_box_soft_wraps_overlong_line`, `input_box_scrolls_to_caret_on_wrapped_line`); [Ch 23](./23_chapter_tui.md) §6.2, §6.6. |

## 1. 2026-08-13 — Plan mode runs provably read-only shell commands (`ls`, `grep`, …)

| Field | Value |
|-------|-------|
| Type | `optimization` |
| Symptom / motivation | Plan mode denies every `Write`-classified tool, and shell commands were always classified `Write` (except `sudo ` / `su ` → `High`), so even `ls` / `grep` — the inspection commands a planning agent needs most — were hard-denied in plan mode. |
| Decision | `PermissionPolicy::ShellCommand::resolve` now classifies a command string as `Read` when it is provably read-only, via a new conservative classifier `crates/tact/src/tool/readonly_shell.rs`: (1) a plain-command split that rejects any shell metacharacter (pipes, redirections, `$`, backticks, globs, escapes, …) so the classification cannot disagree with what `sh -c` runs; (2) a safelist of programs whose options alone cannot write (`ls`, `grep`, `cat`, `head`, `tail`, `wc`, `git status/log/diff/show/branch`, `find`/`rg`/`base64`/`sed` with dangerous flags excluded, …), mirroring OpenAI Codex's `is_known_safe_command` (`codex-rs/shell-command/src/command_safety/is_safe_command.rs`). Anything ambiguous stays `Write` — the classifier is deliberately false-negative-biased so a mutation can never run silently under plan mode. |
| Behavior after | In plan mode `ls -la`, `grep -rn x .`, `git status` run without prompting; `cargo test`, pipes, redirections, unknown programs and unsafe options (`find -delete`, `git push`, …) are still denied. `bash` and `background_run` share the same classification, so read-only commands are also auto-allowed in Default mode. |
| Pointers | `crates/tact/src/tool/readonly_shell.rs`; `crates/tact/src/tool/metadata.rs` (`ShellCommand::resolve`); tests in `crates/tact/src/tool/readonly_shell.rs` and `crates/tact/src/permission/mod.rs` (`plan_mode_allows_readonly_shell_commands_and_denies_others`); [Ch 10](./10_chapter_permission.md) §2, §4, §7. |

## 1. 2026-08-12 — async-openai switched from `vendor/async-openai` to a locally maintained fork at `../async-openai`

| Field | Value |
|-------|-------|
| Type | `docs` (dependency management) |
| Symptom / motivation | The vendored copy under `vendor/async-openai` (2026-08-10 entry) worked, but it duplicated the whole crate in-tree: every upstream sync meant diffing, re-applying patches and keeping the crate-level doctests green under Tact's minimal feature set. |
| Decision | The fork now lives as its own repository at `../async-openai` (clone of `https://github.com/rust-infra/async-openai`, branch `feat/tact`, commit `ca74607` = upstream main at 0.41.3), maintained directly with four local commits: (1) typed `context_management: Option<Vec<ContextManagementParam>>` field on `CreateResponse`; (2) `ReasoningEffort::Max` variant; (3) doctest feature gates for the two `client.chat()` examples; (4) package renamed `async-openai-local` with `[lib] name = "async_openai"` (examples dropped from the fork workspace since they still reference the upstream package name). Tact's workspace dependency becomes `async-openai-responses = { package = "async-openai-local", path = "../async-openai/async-openai", version = "0.41.3", features = ["responses", "byot"] }`; `vendor/async-openai/` is deleted. Code keeps `use async_openai_responses::…` unchanged. |
| Behavior after | No user-visible change: the wire body still carries `context_management` when a threshold is configured. Maintenance moved out-of-tree: patch the local fork (`/Users/rg/Projects/async-openai`, branch `feat/tact`) instead of re-vendoring. |
| Pointers | `/Users/rg/Projects/async-openai` (fork, commits `7de8bb4` / `5e22785` / `12488eb` on `feat/tact`); `Cargo.toml` `async-openai-responses` dependency; `crates/tact_llm/src/openai/responses/convert.rs` (`create_response` builder injection); [Ch 22](./22_chapter_llm.md) §6.2. |

## 1. 2026-08-12 — Worktree storage migrated from JSON files to SQLite (`WorktreeStore`)

| Field | Value |
|-------|-------|
| Type | `optimization` |
| Symptom / motivation | Worktree metadata + audit log lived in a single JSON index (`worktrees/index.json`, `Store<WorktreeIndex>`) with read-modify-write and no transaction; duplicate-name checks raced with index writes. |
| Decision | Worktree state moved into the existing `<workdir>/.tact/tact.db` as `worktrees` + `worktree_events` tables via a new async `WorktreeStore` trait (`crates/tact/src/store/worktree_store/`, `SqliteWorktreeStore` with sqlx). `worktrees.name` is UNIQUE (concurrency backstop); the autoincrement `id` preserves insertion order; `worktree_events` orders by its own `id`. Added `session_id` column + index, filled from the tool context at `worktree_create`. `WorktreeManager` became an async facade over `Box<dyn WorktreeStore>`; `SharedWorktreeManager` dropped its mutex (`Arc<WorktreeManager>`, the pool serializes writes — `worktree_run` no longer blocks other worktree tools). Legacy `worktrees/index.json` is not read anymore and is left on disk. With this change, no domain module uses the JSON store (`StoreRoot`/`Store`/`CollectionStore` remain as a generic primitive with their own unit tests). |
| Behavior after | Lanes and events persist in `tact.db` (old `worktrees/index.json` entries are gone unless exported manually); `worktree_*` surfaces unchanged; `session_id` appears in worktree records. |
| Pointers | `crates/tact/src/store/worktree_store/{mod,sqlite}.rs`, `crates/tact/src/worktree/mod.rs`, `crates/tact/src/tool/worktree.rs`, `crates/tact-ui/src/{headless,interactive}.rs`; [Ch 1](./01_chapter_store.md) §5–6, [Ch 15](./15_chapter_worktree.md) §2–5. |

## 1. 2026-08-12 — Team storage migrated from JSON files to SQLite (`TeamStore`)

| Field | Value |
|-------|-------|
| Type | `optimization` |
| Symptom / motivation | The roster lived in a single JSON index (`team/config.json` with a `TeamConfig` wrapper) and inboxes as one JSONL file per owner (`team/inbox/{owner}.json`). Both used read-modify-write with no transaction and no cross-process lock; duplicate-name checks raced with roster writes. |
| Decision | Team state moved into the existing `<workdir>/.tact/tact.db` as `teammates` + `inbox_messages` tables via a new async `TeamStore` trait (`crates/tact/src/store/team_store/`, `SqliteTeamStore` with sqlx). `teammates.name` is the PRIMARY KEY; duplicate spawn is rejected with `INSERT OR IGNORE` + `rows_affected == 0` (keeps the `teammate {name} already exists` error and stays race-free). `inbox_messages` gains an autoincrement `id` so reads preserve insertion order (the legacy JSONL append semantics) plus an `owner` index. `TeammateManager` became an async facade over `Box<dyn TeamStore>`; `SharedTeammateManager` dropped its mutex (`Arc<TeammateManager>`, the pool serializes writes). Legacy JSON files are not read anymore and are left on disk. |
| Behavior after | Roster and inboxes persist in `tact.db` (old `team/` JSON entries are gone unless exported manually); `spawn_teammate` / `broadcast` / `read_inbox` / `plan_approval` / `shutdown_*` surfaces unchanged; cross-process inbox writes no longer race on file appends. |
| Pointers | `crates/tact/src/store/team_store/{mod,sqlite}.rs`, `crates/tact/src/team.rs`, `crates/tact/src/tool/team.rs`, `crates/tact-ui/src/{headless,interactive}.rs`; [Ch 1](./01_chapter_store.md) §5–6, [Ch 14](./14_chapter_team.md) §3–5. |

## 1. 2026-08-12 — Cron & background tasks migrated from JSON files to SQLite (`CronStore` / `BackgroundStore`)

| Field | Value |
|-------|-------|
| Type | `optimization` |
| Symptom / motivation | Cron persisted as a single JSON index (`cron/scheduled_tasks.json` with a `next_id` counter) and background as one JSON file per record (`background/tasks/{id}.json` plus an in-memory `Mutex<HashMap>` mirror). Both used read-modify-write with no transaction and no cross-process lock; the background manager held duplicate state (disk + memory) that could drift. |
| Decision | Cron and background moved into the existing `<workdir>/.tact/tact.db` as `cron_tasks` + `background_tasks` tables via new async traits `CronStore` (`crates/tact/src/store/cron_store/`) and `BackgroundStore` (`crates/tact/src/store/background_store/`), mirroring the `TaskStore` pattern. `cron_tasks` ids come from `INTEGER PRIMARY KEY AUTOINCREMENT` surfaced as 8-hex strings (`format!("{rowid:08x}")`) — same wire contract as the legacy index; `background_tasks` keeps the timestamp-millis hex `id` with a `CHECK`-constrained status. Both tables gained a `session_id` column + index, filled from the tool context at `cron_create` / `background_run`. `CronScheduler` / `BackgroundManager` became async facades; `SharedCronScheduler` dropped its mutex (`Arc<CronScheduler>`) and `BackgroundManager` dropped its in-memory mirror (the DB is the single source of truth; the spawned tokio task writes back through a cloned store handle). Legacy JSON files are not read anymore and are left on disk; `TactPath::cron_dir()` / `CRON_SUBDIR` removed as dead code. |
| Behavior after | Cron ids restart at `00000001` (legacy entries are gone unless exported manually from `.tact/cron/`); `cron_*` / `background_*` / `/background` surface unchanged; startup orphan repair (`running` → `error`) now sweeps the table; `session_id` appears in cron JSON and background records. |
| Pointers | `crates/tact/src/store/cron_store/{mod,sqlite}.rs`, `crates/tact/src/store/background_store/{mod,sqlite}.rs`, `crates/tact/src/cron/mod.rs`, `crates/tact/src/background.rs`, `crates/tact/src/tool/{cron,background_run}.rs`, `crates/tact-ui/src/{headless,interactive,driver}.rs`; [Ch 1](./01_chapter_store.md) §5–6, [Ch 13](./13_chapter_background.md) §2–5. (Ch 16 was removed with the cron feature on 2026-08-14.) |

## 1. 2026-08-11 — Tasks migrated from JSON files to SQLite (`TaskStore`)

| Field | Value |
|-------|-------|
| Type | `optimization` |
| Symptom / motivation | Tasks persisted as one JSON file per record (`tasks/task_{id}.json`) plus a `tasks/index.json` next-id counter. ID allocation and dependency edges (`blockedBy` / `blocks` mirrored on both records) were read-modify-write with no transaction and no cross-process lock; completing a task required an O(n) scan to clear edges. |
| Decision | Tasks moved into the existing `<workdir>/.tact/tact.db` as `tasks` + `task_dependencies` tables via a new `TaskStore` trait (`crates/tact/src/store/task_store/`, `SqliteTaskStore` with sqlx). Edges are rows (composite PK, `INSERT OR IGNORE`), no mirrored fields, no foreign keys; every mutation runs in a `BEGIN IMMEDIATE` transaction, completion clears edges with one `DELETE`. IDs come from `INTEGER PRIMARY KEY AUTOINCREMENT` (`TaskIndex` removed). `TaskManager` became an async facade over `Box<dyn TaskStore>`; `SharedTaskManager` dropped its mutex (`Arc<TaskManager>`, the pool serializes writes). Added `session_id` column + index, filled from the tool context at `task_create`. Legacy JSON files are not read anymore and are left on disk. `tokio` features `macros` + `rt-multi-thread` added to `crates/tact/Cargo.toml` so `#[tokio::test]` works when building `-p tact` alone. |
| Behavior after | New task IDs start at 1 (old 1–233 records are gone unless exported manually from `.tact/tasks/`); dependency updates are atomic; `task_*` tools unchanged on the surface (`session_id` appears in task JSON/snapshots). |
| Pointers | `crates/tact/src/store/task_store/{mod,sqlite}.rs`, `crates/tact/src/task/mod.rs`, `crates/tact/src/tool/task.rs`; [Ch 1](./01_chapter_store.md) §6, [Ch 19](./19_chapter_persistent_tasks.md) §2–3. |

## 1. 2026-08-11 — Summarizer thinking budget clamped below `max_tokens`; Kimi K3 default reasoning reserve

| Field | Value |
|-------|-------|
| Type | `bugfix` |
| Symptom / motivation | The 2026-08-10 summarizer budget change forwarded the configured Claude-style thinking budget to the compact summary request with the same `with_thinking(self.thinking_config())` call as the main loop, but the summarizer's `max_tokens` is independently capped at `min(20% × window, 2,000)`. Anthropic requires `budget_tokens < max_tokens` on the wire, so a default 8k/32k thinking budget produced an invalid request (`thinking.budget_tokens = 8,000` with `max_tokens = 2,000`) and local compaction failed with a 400 for every Anthropic user with thinking enabled. Separately, the reasoning reserve only treated DeepSeek as default-reasoning, but Kimi K3 also defaults thinking ON + effort high server-side, so Kimi summaries could still be starved by reasoning without an explicit effort. |
| Decision | New `compact_summary_thinking(configured_budget, summary_max_tokens)` clamps the forwarded budget to `summary_max_tokens - 1` (and disables thinking entirely for a degenerate ≤ 1-token output budget), applied to both the initial and continuation summarizer requests via a small builder closure. The input-side reservation still subtracts the configured budget (conservative). `compact_summary_reasoning_reserve_percent` now reserves the default high tier (75%) for `ProviderKind::Kimi` as well as DeepSeek. |
| Behavior after | Anthropic compaction with a large thinking budget sends `budget_tokens = max_tokens - 1` instead of failing with a 400; a budget that already fits passes through unchanged. Kimi K3 with no explicit effort gets the same 75% reasoning reserve as DeepSeek. |
| Pointers | `compact_summary_thinking` + `compact_summary_reasoning_reserve_percent` in `crates/tact/src/agent/mod.rs` (`compact_history_local_with_mode`); tests `compact_summary_thinking_clamps_below_max_tokens`, `local_compact_clamps_thinking_budget_below_summary_max_tokens`, `compact_summary_reasoning_reserve_percent_tiers`; [Ch 5](./05_chapter_compact.md) §5 step 3. |

## 1. 2026-08-10 — Vendor async-openai locally as `async-openai-local` for typed `context_management`

| Field | Value |
|-------|-------|
| Type | `docs` (dependency management) |
| Symptom / motivation | OpenAI's Responses API officially supports `context_management` / `compact_threshold` (server-side compaction), and the official Python/Node SDKs expose it typed, but the Rust `async-openai` crate (latest 0.41.3, as of 2026-08-10) only defines `ContextManagementParam` without wiring it into `CreateResponseArgs` — so Tact had to inject it through the byot JSON path (`body["context_management"] = serde_json::json!(...)`). |
| Decision | Vendor the async-openai 0.41.3 source under `vendor/async-openai` and rename the package to `async-openai-local` so the path dependency only satisfies the Responses-protocol 0.41.x dependency and cannot collide with the legacy `async-openai 0.20` used by the Chat Completions path (`async-openai-responses = { package = "async-openai-local", path = "vendor/async-openai", ... }` in the workspace manifest; code keeps `use async_openai_responses::…`, no reference changes). Two source divergences from upstream: a typed `context_management: Option<Vec<ContextManagementParam>>` field on `CreateResponse`, and a `Max` variant on `ReasoningEffort` (upstream stops at `Xhigh`; DeepSeek / Kimi K3 accept `max`). `convert.rs` now builds both through the typed builder — `context_management(...)` setter and `Reasoning { effort: request.reasoning_effort.map(Into::into), summary }` via `impl From<OpenAiReasoningEffort> for ReasoningEffort` in `crates/tact_llm/src/types.rs` — instead of `serde_json::json!` / `Value::String` injection. `vendor/async-openai/README.fork.md` documents how to sync with upstream. |
| Behavior after | No user-visible change: the wire body still carries `context_management` when a threshold is configured. Maintenance is now local: new Responses fields can be added to the vendor without waiting for the upstream Rust crate. |
| Pointers | `vendor/async-openai/` (`README.fork.md`, `src/types/responses/response.rs`); `Cargo.toml` `async-openai-responses` dependency; `crates/tact_llm/src/openai/responses/convert.rs` (`create_response` builder injection); [Ch 22](./22_chapter_llm.md) §6.2. |

## 1. 2026-08-10 — Clear error when a Responses endpoint does not implement `/responses/compact`

| Field | Value |
|-------|-------|
| Type | `bugfix` |
| Symptom / motivation | Compatible `/responses` endpoints (e.g. `opencode.ai/zen/go/v1`) often do not implement `POST /responses/compact` and answer 404 with an HTML page. The SDK's `compact_byot` then surfaced a JSON-deserialization error that dumped the entire HTML body, and the message could even match the transient-error retry list ("unavailable"), causing pointless backoff retries before failing. |
| Decision | `compact()` in the Responses adapter now sends the compact request through the raw shared HTTP client (same transport the SDK uses) so the status code is inspectable. HTTP 404/405 maps to `LlmError::Unsupported("endpoint does not support POST /responses/compact (HTTP {status}): native Responses compaction is not implemented by base URL {base_url}")` — deliberately worded to avoid the transient-error keywords so no retry loop is entered. Other non-2xx statuses use the existing `LlmError::HttpError { status, body }`. |
| Behavior after | On an endpoint without `/responses/compact`, triggering compaction immediately shows the clear message naming the missing endpoint and base URL (no HTML dump, no retries); the session state is left untouched. |
| Pointers | `compact()` in `crates/tact_llm/src/openai/responses/mod.rs`; test `compact_reports_missing_endpoint_clearly`; [Ch 22](./22_chapter_llm.md) §6.2, [Ch 5](./05_chapter_compact.md). |

## 1. 2026-08-10 — Responses adapter recovers when a compatible stream ends without a terminal event

| Field | Value |
|-------|-------|
| Type | `bugfix` |
| Symptom / motivation | A compatible `/responses` endpoint (e.g. `opencode.ai/zen/go/v1`) occasionally closes the SSE stream without any terminal event (`response.completed` / `response.incomplete` / `response.failed`). `ResponsesStreamState::finish()` hard-failed with `unsupported response state: OpenAI Responses stream ended without a terminal event`, aborting the whole agent turn even though the stream had delivered a complete `output_item.done` sequence or visible text. |
| Decision | A clean EOF without a terminal event is now treated as terminal when the stream itself is complete: if every announced item finished (`output_item.done` sequence contiguous and no pending `added`), the output is reconstructed from the done sequence; otherwise streamed visible text is recovered (same branch as the existing compatible-endpoint recovery). A minimal completed `Response` is synthesized and flows through the existing normalization/recovery paths, so stop reason inference (including `ToolUse` for tool calls) and provider-state baseline construction are unchanged. A missing compaction boundary (`pending_compactions` non-empty) and an empty stream remain hard protocol errors — recovery must never silently drop a compacted baseline. |
| Behavior after | Turns that previously died with "stream ended without a terminal event" now complete from the done sequence / streamed text when the response was fully delivered; genuinely empty or compaction-incomplete streams still fail loudly. |
| Pointers | `finish()` in `crates/tact_llm/src/openai/responses/stream.rs`; tests `no_terminal_event_recovers_from_complete_done_sequence`, `no_terminal_event_recovers_visible_text`, `no_terminal_event_empty_stream_is_error`, `no_terminal_event_with_pending_compaction_is_error`; [Ch 22](./22_chapter_llm.md) §6.2. |

## 1. 2026-08-10 — Local compaction reserves output for reasoning / thinking tokens

| Field | Value |
|-------|-------|
| Type | `bugfix` |
| Symptom / motivation | `compact_history_local_with_mode` (the summarizer behind every non-OpenAI-Responses compaction) sized the summarizer's `max_tokens` as `min(20% × window, 2,000)` and treated it as the **text** budget, but reasoning-effort providers (OpenAI o-series / DeepSeek / Kimi K3) count reasoning tokens inside that same `max_tokens` envelope. With an explicit `high`/`max` effort — or on DeepSeek, which defaults thinking ON + effort high server-side — reasoning burned most of the 2,000-token budget, leaving too little for the summary text, so every call hit `StopReason::MaxTokens` and the continuation loop (≤3, each with the same shared cap) ended up accepting a best-effort partial summary. The compact request also never forwarded the configured Claude-style thinking budget (unlike the main loop), and the input reservation ignored it. |
| Decision | Split the summarizer output budget: the summary **text** keeps the classic `min(20% × window, 2,000)`; when a reasoning effort is configured (or the provider is DeepSeek with no explicit effort) a tiered reserve is added **on top** (25/50/75/100% of the text budget for minimal|low / medium / high / xhigh|max, DeepSeek default ≈ high = 75%), so `max_tokens` on the wire = text + reserve and reasoning never starves the text. The summarizer now forwards the configured thinking budget (`with_thinking(self.thinking_config())`, matching the main loop) and subtracts it from the input-side reservation. No effort semantics are changed — this is budget accounting only. |
| Behavior after | Compaction summaries with reasoning effort configured (or on DeepSeek) get a larger wire `max_tokens` (e.g. high effort on a 128k window → 2,000 + 1,500 = 3,500) while the text portion still receives its full classic budget; the summarizer request carries the same thinking config as main-loop turns; the input reservation accounts for both reasoning and thinking headroom, and bails with the existing "too small" error when the window cannot fit the prompt after those reservations. |
| Pointers | `compact_summary_reasoning_reserve_percent` + budget math in `crates/tact/src/agent/mod.rs` (`compact_history_local_with_mode`); tests `compact_summary_reasoning_reserve_percent_tiers`, `local_compact_reserves_reasoning_budget_and_forwards_thinking`, `local_compact_input_reservation_subtracts_thinking_budget`; [Ch 5](./05_chapter_compact.md) §5 step 3. |

## 1. 2026-08-10 — `background_run` streams live output to the tool card (bash-like)

| Field | Value |
|-------|-------|
| Type | `optimization` |
| Symptom / motivation | `background_run` buffered the whole command output with `Command::output()` and its tool card finalized instantly ("started"), so users saw nothing until they polled `check_background` — unlike `bash`, whose card streams output live. |
| Decision | Add a keep-live card contract: `ToolPresentationInfo.keep_live` (mapped from new `LiveOutputPolicy::Background`) makes the TUI leave the card active after `StepFinished`; the manager now reads stdout/stderr incrementally (`read_pipe` + `Utf8Decoder` + ~50ms throttled `ToolProgress`, live preview keeps last ~4 KB) and closes the card with a new `AgentUpdate::BackgroundTaskFinished { tool_id, success, message, output }` carrying ✓/✗, elapsed time and the capped final output. `background_run` passes a `BackgroundProgressSink` (tool_id + `ui_tx`) into `SharedBackgroundManager::run`; record persistence and the 120s timeout are unchanged. |
| Behavior after | `background_run cargo build` shows a spinner + live build output in the TUI card, then finalizes with ✓/✗ and duration when the process exits — even if the agent turn already ended. The model still has no completion push and must poll `check_background`. |
| Pointers | `crates/tact/src/background.rs` (`BackgroundProgressSink`, `run_background_process`); `crates/tact/src/tool/background_run.rs`; `LiveOutputPolicy::Background` in `crates/tact/src/tool/metadata.rs`; `AgentUpdate::BackgroundTaskFinished` in `crates/protocol/src/agent.rs`; TUI `on_step_finished` / `on_background_task_finished` in `crates/tui/src/widgets/state/app/agent.rs`; [Ch 13](./13_chapter_background.md), [Ch 25](./25_chapter_protocol.md). |

## 1. 2026-08-10 — `/background` slash command for background job status

| Field | Value |
|-------|-------|
| Type | `optimization` |
| Symptom / motivation | Background jobs started with `background_run` could only be inspected by asking the model to call the `check_background` tool; there was no direct TUI affordance, so users had to type a prompt just to poll a task. |
| Decision | Add a TUI slash command `/background` (`/background <id>` for one task) backed by a new `UserCommand::QueryBackground(Option<String>)`. The command driver (`crates/tact-ui/src/driver.rs`) calls the shared `ToolContext.background_manager.check(id)` — the same code path as the `check_background` tool — and emits `AgentUpdate::MdInfo` with a `## ⚙️ Background Tasks` fenced code block (or `AgentUpdate::Error` for an unknown id). The command is listed in `PALETTE_COMMANDS`, localized in `i18n.rs` (EN/ZH), and rendered with the `🖥` palette emoji. |
| Behavior after | `/background` prints one line per task (id, status, command); `/background <id>` prints the task's pretty JSON; an unknown id shows an error. No new state, no completion push — the command only reads the persisted/in-memory records. |
| Pointers | `UserCommand::QueryBackground` in `crates/protocol/src/agent.rs`; driver match arm in `crates/tact-ui/src/driver.rs`; `PALETTE_COMMANDS` in `crates/tui/src/widgets/state/mod.rs`; `execute_palette_command` in `crates/tui/src/handlers/mod.rs`; [Ch 13](./13_chapter_background.md), [Ch 23](./23_chapter_tui.md) §3. |

## 1. 2026-08-09 — Hosted web search for OpenAI Responses (`protocol = "responses"`)

| Field | Value |
|-------|-------|
| Type | `optimization` |
| PR | https://github.com/laohanlinux/tact/pull/62 (branch `feat/responses-web-search`) |
| Symptom / motivation | The Responses adapter only sent function tools, so OpenAI `protocol = "responses"` sessions had no hosted (provider-executed) web search; users had to wire an MCP `web_search` function tool or drop to Chat Completions. |
| Decision | Hosted web search is a **Responses-protocol capability**, independent of the endpoint/provider: the adapter injects `Tool::WebSearch` on every ordinary `/responses` request whenever `protocol = "responses"` is chosen (`create_response(..., native_web_search = true)`; `false` only for `/responses/compact`, which accepts no tools) — for OpenAI, DeepSeek, and custom OpenAI-compatible endpoints alike, with no per-provider switch (`OpenAiResponsesAdapter` has no `native_web_search` flag; `ResponsesCapabilities::hosted_tools` includes `WebSearch` for every Responses endpoint). The provider executes the search server-side; Tact only renders a tool card from real Step events (`StepStarted` on `output_item.added`, `StepFinished`/`StepFailed` on the first `output_item.done` per index; `in_progress`/`searching` at `done` is a defensive failure). `web_search_call` never becomes `ContentBlock::ToolUse`, and stop reason stays `completed`. Compatible endpoints that emit the search action as a `queries` array instead of the singular `query` are handled by `wire::normalize_web_search_call_query` (fills `query` for typed parsing only; raw items are replayed verbatim). `AgentUpdate::StepFailed` gained `arg_summary` so failed cards keep the query in the title. DeepSeek keeps the code path but remains rejected at config resolution (#57) until its Responses support is re-enabled. |
| Behavior after | Any `protocol = "responses"` session — OpenAI, DeepSeek, or custom OpenAI-compatible — automatically gets hosted web search; the TUI shows a `🔍 Web Search` card with the query as title and sources as expandable detail; failures carry status/query/action diagnostics. |
| Pointers | `crates/tact_llm/src/openai/responses/{convert,stream,wire,mod}.rs`, `crates/tact_llm/src/provider.rs` (`build_openai_responses`), `crates/tui/src/widgets/tool_widget.rs`, AGENTS.md "Hosted tools (Provider-executed) — design invariants", [Ch 22 §6.2.1.1](./22_chapter_llm.md). |

## 1. 2026-08-09 — Task-stats `[copy]` copies the last turn

| Field | Value |
|-------|-------|
| Type | `bugfix` |
| Symptom / motivation | After a turn finished, users had no one-click way to copy that turn's conversation from the task-stats row. |
| Decision | Append a `[copy]` button on each `📊 任务统计：` row; click copies log text from after the previous stats row (or session start) up to but not including the current stats row, skipping blanks and task-end separators. |
| Behavior after | Click `[copy]` on a stats line → clipboard gets that turn's user/assistant content; earlier turns are excluded. |
| Pointers | `add_task_stats_block` / `copy_turn_ending_at_stats` in `messages.rs`; mouse hit in `handlers/mouse.rs`; regression `copy_turn_ending_at_stats_copies_last_turn_only`. |

## 1. 2026-08-09 — Mermaid diagram copy popup (double-click → source)

| Field | Value |
|-------|-------|
| Type | `bugfix` |
| Symptom / motivation | Successfully rendered Mermaid diagrams discarded their fence body when splicing ASCII art into the log, so users could not recover the source for re-editing. |
| Decision | Keep a `MermaidBlock { start_idx, end_idx, source }` per successful render; double-click opens a Mermaid popup; popup `y` copies the source. Log selection yank stays ASCII. |
| Behavior after | Double-click any diagram row → source popup (`y` / `j/k` / `Esc`); failed Mermaid still uses the code-card path. |
| Pointers | Spec `docs/superpowers/specs/2026-08-09-mermaid-diagram-copy-popup-design.md`; `finish_stream_code_block`; `popups/mermaid_popup.rs`; regressions `log_renders_streamed_mermaid_without_code_card`, `mermaid_popup_copy_uses_source_not_ascii`. |

## 1. 2026-08-09 — Mermaid sequence self-messages draw a U-shaped loop

| Field | Value |
|-------|-------|
| Type | `bugfix` |
| Symptom / motivation | Self-messages (`A->>A`) rendered as a one-cell stub `<│◀`, which looked like broken chevrons rather than a return-to-self arrow. |
| Decision | Render self-messages as a two-row box-drawing loop (`│──┐` / `│◀─┘`, or left-side `┌──│` / `└─▶│` on the last participant) with the label beside the loop. |
| Behavior after | Self calls read as a clear U-turn on the lifeline; last-column self-messages loop left so the shape stays inside the diagram. |
| Pointers | `crates/tui/src/render/mermaid_sequence.rs` (`self_loop_rows`); regressions `self_message_draws_u_shaped_loop`, `self_message_on_last_participant_loops_left`. |

## 1. 2026-08-09 — Mermaid sequence labels no longer drop characters or shift columns

| Field | Value |
|-------|-------|
| Type | `bugfix` |
| Symptom / motivation | The custom `sequenceDiagram` renderer dropped any label glyph whose display cells overlapped a lifeline (e.g. `submitTask` → `ubmitTask` / `submi│Task`), and left a ghost space after each width-2 CJK glyph so label rows were wider than lifeline/arrow rows — lifelines looked jagged and arrows appeared broken on multi-participant diagrams. |
| Decision | In `label_row`, clear continuation cells of wide glyphs to empty spans, and reflow label characters past occupied lifeline cells instead of skipping them. |
| Behavior after | Long ASCII and CJK arrow labels keep every character (split around `│` when needed) and every diagram row shares the same display width, so lifelines stay vertically aligned. |
| Pointers | `crates/tui/src/render/mermaid_sequence.rs` (`label_row`); regressions `cjk_label_keeps_same_display_width_as_lifeline_row`, `long_ascii_label_is_not_eaten_by_lifelines`, `self_message_keeps_lifeline_intact`. |

## 1. 2026-08-08 — TUI renders Mermaid sequence diagrams with its own renderer

| Field | Value |
|-------|-------|
| Type | `bugfix` |
| Symptom / motivation | The upstream `ratatui-markdown` sequence renderer mishandled three common inputs: `participant A as 用户` aliases were shown verbatim, the `+`/`-` activation shorthand (`A->>+B`) created phantom participant columns (`+B`, `-B`, …), and 2-column CJK arrow labels could overwrite a lifeline (or be dropped), so labelled arrows looked misaligned. |
| Decision | Route `sequenceDiagram` fences in the TUI through Tact's own renderer (`crates/tui/src/render/mermaid_sequence.rs`); all other Mermaid diagram types keep using `ratatui-markdown`. The new renderer parses `as` aliases, strips `+`/`-` activation prefixes before participant lookup, and places label glyphs by display column only when every cell of the glyph's width is free. |
| Behavior after | Only declared participants render as columns; `A->>+B` targets participant `B`; CJK labels stay centered between lifelines and never overwrite a `│`. Unparseable sources still fall back to ordinary code rendering. |
| Pointers | `crates/tui/src/render/mermaid_sequence.rs`; routing: `crates/tui/src/render/render_md.rs` (`render_mermaid_block`); regression tests in `mermaid_sequence.rs`. |

## 1. 2026-08-08 — Subagent model picker uses its own provider

| Field | Value |
|-------|-------|
| Type | `bugfix` |
| Symptom / motivation | `/model-subagent` merged configured subagent models with API model IDs fetched from the main agent's active provider, so separate main/subagent providers could show the wrong models. |
| Decision | Query the `/models` endpoint using the resolved subagent provider's `base_url` and `api_key`; keep configured provider `models = [...]` as the primary candidates and preserve cache keying by `(base_url, api_key)`. |
| Behavior after | The subagent picker shows configured and API-discovered models belonging to the subagent provider. The main `/model` picker keeps using the main provider. |
| Pointers | `crates/tact_llm/src/models.rs`, `crates/tui/src/handlers/select.rs`; regression test `explicit_provider_model_query_uses_subagent_credentials`; design: `docs/superpowers/specs/2026-08-08-subagent-model-picker-provider-design.md`; plan: `docs/superpowers/plans/2026-08-08-subagent-model-picker-provider.md`. |

## 1. 2026-08-08 — DeepSeek and Kimi Responses remain configuration-gated

| Field | Value |
|-------|-------|
| Type | `bugfix` |
| Symptom / motivation | The generic Responses adapter can be constructed for OpenAI-compatible endpoints, but DeepSeek/Kimi native compaction and state-continuation behavior are not verified to the production contract. Allowing them through normal config would make unsupported fallback behavior look supported. |
| Decision | Keep DeepSeek and Kimi `protocol = "responses"` rejected at config resolution. Lower-level adapter construction remains available for isolated endpoint tests; production configuration uses Chat Completions until native Responses capabilities are verified. |
| Behavior after | DeepSeek/Kimi users receive a clear configuration error instead of entering an unverified Responses path. OpenAI and explicitly configured custom OpenAI-compatible providers retain their existing Responses routes. |
| Pointers | `crates/tact/src/config/resolve.rs`; provider construction: `crates/tact_llm/src/provider.rs`; related design: `docs/superpowers/specs/2026-08-08-openai-responses-complete-design.md`; compaction behavior: Ch 5. |


## 1. 2026-08-08 — OpenAI Responses preserves unknown wire items

| Field | Value |
|-------|-------|
| Type | `bugfix` |
| Symptom / motivation | The typed `async-openai` Responses enum rejects a newly introduced output item before Tact can preserve it, making forward-compatible provider state impossible. Tact also had no explicit request extension for Responses-only fields outside the shared Chat/Anthropic request model. |
| Decision | Parse the raw Responses envelope before typed normalization; normalize known items while retaining unknown input/output items as raw JSON. Add a `ResponsesRequestOptions` boundary consumed only by the Responses adapter, and expose conservative provider capability metadata without forking `async-openai` until a reproducible SDK blocker exists. |
| Behavior after | Unknown harmless stream events no longer abort a response. Unknown output items survive ordinary and streamed turns, session state serialization, and the next Responses request. Responses-only request options do not appear in Chat Completions or Anthropic payloads. |
| Pointers | `crates/tact_llm/src/openai/responses/wire.rs`, `request_options.rs`, `stream.rs`, `provider.rs`; design: `docs/superpowers/specs/2026-08-08-openai-responses-complete-design.md`; plan: `docs/superpowers/plans/2026-08-08-responses-compatibility-foundation.md`; compaction: Ch 5 and `docs/compaction.md`. |


## 1. 2026-08-08 — Main-area Markdown renders complete Mermaid fences as terminal diagrams

| Field | Value |
|-------|-------|
| Type  | `optimization` |
| Related | `crates/tui/src/render/render_md.rs`, `crates/tui/src/widgets/state/app/agent.rs`, `crates/tui/src/widgets/state/app/visibility.rs`, `crates/tui/src/widgets/state/stream_state.rs`, Ch 23 §6.7 |
| Symptom / motivation | Every explicit-language streamed fence — including ```mermaid — was promoted to a `CodeBlock` card overlay when it closed, so Mermaid source appeared as syntax-tinted code instead of a diagram. |
| Decision | Route complete, top-level `mermaid` fences through a shared `render_mermaid_block` helper (`ratatui-markdown::mermaid::render_mermaid` with the app-theme adapter) in `render_md.rs`; mark the buffered streamed fence as Mermaid in `stream_state.rs`; on a valid closed fence, `agent.rs` / `visibility.rs` splice the diagram lines into the log instead of pushing a `CodeBlock`. The code-card fallback is kept for invalid, unsupported, or unclosed Mermaid so no source is ever dropped. |
| Behavior after | A complete ```mermaid fence renders as a themed terminal diagram at the log width (nominal 80 columns in fixed-width Markdown paths); a valid streamed Mermaid block closes without creating a code card; invalid/unsupported/unclosed Mermaid and ordinary explicit-language fences keep the existing code-card path; width re-layout and viewport scrolling use the existing log layout/cache behavior. |
| Pointers | `render_md.rs` (`render_mermaid_block`, `route_mermaid_fences`) and tests `render_mermaid_sequence_returns_terminal_lines`, `render_markdown_mermaid_flowchart_uses_box_art`, `render_markdown_invalid_mermaid_falls_back_to_code`, `render_markdown_unclosed_mermaid_fence_keeps_source`; `stream_state.rs` (`code_block_is_mermaid`); `agent.rs` (`finish_stream_code_block`), `visibility.rs` (`flush_stream_pending`); regressions in `render_gap_tests.rs` (`log_renders_streamed_mermaid_without_code_card`, `log_falls_back_to_code_card_for_invalid_streamed_mermaid`, `flush_renders_streamed_mermaid_without_trailing_newline`, `flush_falls_back_to_code_card_for_unclosed_streamed_mermaid`), `cells/markdown.rs` (`markdown_cell_renders_mermaid_at_the_requested_width`); spec `docs/superpowers/specs/2026-08-08-mermaid-main-rendering-design.md`; plan `docs/superpowers/plans/2026-08-08-mermaid-main-rendering.md`; docs `book/23_chapter_tui*.md` §6.7 |

---


## 1. 2026-08-06 — OpenAI Responses exposes detailed reasoning summaries

| Field | Value |
|-------|-------|
| Type | `bugfix` |
| Related | `crates/tact_llm/src/openai/responses/convert.rs`, Responses reasoning request construction |
| Symptom / motivation | Ordinary OpenAI Responses requests asked for `reasoning.summary = auto`, so the streamed thinking block could contain only a short provider-selected summary even when reasoning was enabled. |
| Decision | Keep the Responses API `summary` field, but request `ReasoningSummary::Detailed` (`"detailed"`) whenever Tact enables reasoning. No changes are needed to stream parsing, which already consumes reasoning summary deltas. |
| Behavior after | OpenAI Responses thinking blocks request and display the detailed reasoning summary rather than the automatic summary level. |
| Pointers | Request conversion and regression assertion: `crates/tact_llm/src/openai/responses/convert.rs`; related Responses adapter: `crates/tact_llm/src/openai/responses/`. |



| Field | Value |
|-------|-------|
| Type | `bugfix` |
| Related | `crates/tui/src/render/log.rs`, `crates/tui/src/render/cells/text.rs`, `crates/tui/src/render/log_render_tests.rs` |
| Symptom / motivation | The main log area wrapped lines to the full panel content width, then added a left indent while drawing ordinary messages. Full-width messages were consequently clipped by several columns at the right edge; selection redraws also used the wrong wrap width. |
| Decision | Subtract each message's actual indent before caching wrapped lines; use the same reply indent for the streaming row. Make `TextCell` selection wrapping use the already-available width after indentation. |
| Behavior after | Full-width ordinary, nested, and streaming text wraps within its actual drawable width, so right-edge characters are preserved. |
| Pointers | Layout and wrap cache: `render/log.rs`; text drawing: `render/cells/text.rs`; regression test: `log_full_width_nested_line_wraps_before_indentation_clip`. |

| Field | Value |
|-------|-------|
| Type  | `bugfix` |
| Related | `crates/tact/src/agent/mod.rs` (`set_thinking_budget` / `set_reasoning_effort`), `crates/tact/src/config/mod.rs` (`update_llm_model_and_*`, `update_subagent_*`), `crates/tact/src/config/persist.rs` (TOML removals), `crates/tui/src/render/bar.rs` (`format_think_segment`), Ch 23 §6.6 |
| Symptom / motivation | On OpenAI Chat Completions (effort semantics), the bottom bar showed `think high(32K)`: `high` was the real `reasoning_effort`, but the `32K` was a stale `thinking_budget` left over from a previous budget-semantic model (claude / kimi-for-coding). The budget is meaningless for effort models and is never sent on the wire, yet it rendered next to the effort because neither the runtime setters, the in-memory config updaters, nor the TOML persist path cleared the other field. |
| Decision | Make effort and budget **mutually exclusive** end to end: `set_thinking_budget` clears `reasoning_effort` and `set_reasoning_effort` zeroes `thinking_budget`; the config update fns (`update_llm_model_and_thinking_budget` / `update_llm_model_and_reasoning_effort` and their subagent twins) do the same; the TOML persist fns remove the opposite key from the provider/subagent entry. In the status bar, `format_think_segment` now gives effort precedence (effort present → `think high`, stale budget ignored; budget only → `think 32K`), and an effort without any budget still renders instead of disappearing. |
| Behavior after | Effort-semantic models show `think high` (never `think high(32K)`); budget-semantic models show `think 32K`. Picking an effort removes the stored `thinking_budget` from config.toml; picking a budget removes `reasoning_effort`. Legacy configs that still contain both fields render effort-only and self-heal on the next `/model` persist. |
| Pointers | `format_think_segment` + tests in `crates/tui/src/render/bar.rs`; setters in `crates/tact/src/agent/mod.rs`; config updaters in `crates/tact/src/config/mod.rs`; TOML removals + tests in `crates/tact/src/config/persist.rs`; driver test `set_reasoning_effort_clears_stale_thinking_budget`; TUI test `applying_effort_pick_clears_stale_thinking_budget`; docs `book/23_chapter_tui*.md` §6.6 |

---

## 1. 2026-08-06 — Recovery retry messages include the underlying error

| Field | Value |
|-------|-------|
| Type  | `optimization` |
| Related | `crates/tact/src/recovery.rs` (`error_summary`), `crates/tact/src/agent/mod.rs` (backoff / compact-retry emit sites), Ch 6 §Recovery messages |
| Symptom / motivation | `[Recovery] backoff (1/10): retrying in 1.9s` said *when* the next retry happened but never *why* — the underlying transport error (timeout, connection reset, rate limit, …) was invisible, so a user watching 8+ backoff ticks had no idea what was failing. The compaction summary retries (`[compact retry 1/3] retrying in 1.9s`) had the same defect. |
| Decision | Add `error_summary` in `recovery.rs`: collapse whitespace/newlines to a single line, truncate at 200 chars with an ellipsis. The main-loop backoff message appends the full anyhow chain (outer context → root cause, joined by `": "`); both compaction retry messages append the client error string. Existing tags, counters, and delay text are unchanged, so tests matching `contains("Recovery") && contains("backoff")` keep passing. |
| Behavior after | Recovery retries report the reason, e.g. `[Recovery] backoff (2/10): retrying in 4.3s — http request failed: error sending request for url`. |
| Pointers | `error_summary` + unit tests in `crates/tact/src/recovery.rs`; emit sites in `crates/tact/src/agent/mod.rs`; docs `book/06_chapter_recovery*.md` §Recovery messages |

---

## 1. 2026-08-06 — Unknown provider names allowed as custom OpenAI-compatible providers

| Field | Value |
|-------|-------|
| Type  | `optimization` |
| Related | `crates/tact_llm/src/types.rs` (`ProviderKind::Custom`, `FromStr`), `crates/tact_llm/src/provider.rs` (`build_client`, `model_uses_effort`), `crates/tact_llm/src/hook_select.rs` (`body_hook_for`), `crates/tact_llm/src/models.rs` (`is_models_query_supported`), `crates/tact/src/config/resolve.rs` (`resolve_provider_kind`, `resolve_llm`, `resolve_subagent`), Ch 21 §3–§4 |
| Symptom / motivation | `ProviderKind::from_str` rejected any name outside `anthropic | openai | deepseek | kimi`, so `llm.provider = "moonshot"` (or any self-hosted / gateway provider) failed with "unknown provider" even though the entry carried a working OpenAI-compatible `base_url`. The config layer could not express third-party OpenAI-compatible endpoints. |
| Decision | Add `ProviderKind::Custom(String)` for every non-built-in name. Custom providers reuse the OpenAI protocol end to end: `build_client` dispatches to the OpenAI-compatible adapter (`chat_completions` default, `responses` opt-in), `body_hook_for` follows the same endpoint heuristics as `openai`, `/v1/models` supplementation is supported, and `reasoning_effort` is accepted. They have **no default `base_url`** — resolve fails with "base_url not configured" unless the entry sets one. Built-in gates are unchanged: `responses` protocol stays limited to `openai | deepseek | custom`, `reasoning_effort` to OpenAI-compatible providers (all but anthropic); the map-key validation loop in `resolve_llm` was removed (custom keys are no longer an error). `ProviderKind` lost `Copy` (it now owns a `String`); method receivers changed to `&self`. |
| Behavior after | `llm.provider` / `--provider` accepts any name. Non-built-in names behave as custom OpenAI-compatible providers and require an explicit `base_url` in `[llm.providers.<name>]`. Missing active entries still error at resolve time. |
| Pointers | `ProviderKind` in `crates/tact_llm/src/types.rs`; tests `provider_kind_from_str_accepts_unknown_as_custom` (tact_llm), `custom_provider_resolves_with_openai_protocol` / `custom_provider_without_base_url_errors` / `custom_provider_in_map_resolves` (tact config resolve); docs `book/21_chapter_config*.md` §3–§4, `config.example.toml` |

---

## 1. 2026-08-06 — Account poller reports each outage once instead of every backoff tick

| Field | Value |
|-------|-------|
| Type  | `optimization` |
| Related | `crates/tact-ui/src/account.rs` (`poll_loop`, `spawn_poller`), `crates/tui/src/widgets/state/app/agent.rs` (`handle_account_update` flash), Ch 22 §9 |
| Symptom / motivation | `spawn_poller` forwarded every failed balance / usage query as `AccountUpdate::Error`. On a persistent outage (e.g. network down) the TUI flashed an error every 10 s → 20 s → … → 5 min, forever — a notification storm ("骚扰") that obscured the real state of the app. |
| Decision | Extract the loop into a testable `poll_loop(query, tx, next_delay)` and add an `error_notified` flag: only the **first** failure of a consecutive outage is forwarded; later retries stay silent while backoff continues. A successful query resets the flag, so a fresh outage after recovery reports once again. `NotSupported` still terminates the loop silently (unchanged), and the explicit startup query + `/balance` command keep their one-shot error reporting (user-triggered, not spam). |
| Behavior after | One outage = one flash message, then silent retries with backoff until recovery; after recovery the normal 5–15 s polling resumes and the next outage flashes once again. |
| Pointers | `poll_loop` / `spawn_poller` in `crates/tact-ui/src/account.rs`; tests `poller_forwards_error_once_per_outage_then_resumes`, `poller_stops_on_not_supported_without_error_flash`; docs `book/22_chapter_llm*.md` §9 |

---

## 1. 2026-08-06 — Kimi Code usage quota query restricted to the official `https://api.kimi.com/coding` endpoint

| Field | Value |
|-------|-------|
| Type  | `bugfix` |
| Related | `crates/tact_llm/src/account.rs` (`query_kimi_code_usage`, `kimi_usage_url_from_base_url`), `crates/tact_llm/src/provider.rs` (`is_kimi_usage_supported`, `is_account_query_supported`), `crates/tact-ui/src/account.rs` (`query_once`), Ch 22 §3 / §9 |
| Symptom / motivation | `kimi_usage_url_from_base_url` derived `{origin}/v1/usages` from any configured base URL, so a `kimi-for-coding` model behind a custom OpenAI-compatible proxy would send the proxy's API key to a guessed usage endpoint. `is_kimi_usage_supported` was `is_kimi_coding(&model)` — true for any proxy serving `kimi-for-coding` — so the TUI quota widget polled proxies too. |
| Decision | Mirror the DeepSeek / Kimi balance "credential boundary": `kimi_usage_url_from_base_url` returns the official URL only for HTTPS with the exact host `api.kimi.com` and a `/coding` path (a `/v1` suffix is accepted); anything else returns `None` and `query_kimi_code_usage` bails with "Kimi Code usage API is only available for the official endpoint https://api.kimi.com/coding". `ProviderInfo::is_kimi_usage_supported` requires the same official host/path, so `is_account_query_supported` is false for proxy configurations and the TUI hides the quota widget. `is_kimi_coding` itself is unchanged — it still identifies the Kimi Code platform (including proxies) for wire shape. |
| Behavior after | Kimi Code usage polling works only when `base_url` targets the official `https://api.kimi.com/coding` endpoint. Custom proxies (even with `kimi-for-coding`) report "not supported"; the API key is never sent to `api.kimi.com` from a proxy configuration. |
| Pointers | `kimi_usage_url_from_base_url` + test `kimi_usage_url_derivation` in `crates/tact_llm/src/account.rs`; `is_kimi_usage_supported` + test `is_kimi_usage_supported_only_for_official_endpoint` in `crates/tact_llm/src/provider.rs`; docs `book/22_chapter_llm*.md` §3 / §9 |

---

## 1. 2026-08-06 — DeepSeek balance query restricted to the official `https://api.deepseek.com` endpoint

| Field | Value |
|-------|-------|
| Type  | `bugfix` |
| Related | `crates/tact_llm/src/account.rs` (`query_deepseek_balance`, `deepseek_balance_url_from_base_url`), `crates/tact_llm/src/provider.rs` (`is_deepseek_balance_supported`, `is_account_query_supported`), `crates/tact-ui/src/account.rs` (`query_once`), Ch 22 §3 / §9 |
| Symptom / motivation | `query_deepseek_balance` derived `{origin}/user/balance` from any configured base URL, so DeepSeek models behind a custom OpenAI-compatible proxy would send the proxy's API key to a guessed balance endpoint. DeepSeek only serves `GET /user/balance` on the official host; the fallback was wrong and could leak credentials to the wrong host or surface confusing 404/403 errors. |
| Decision | Mirror the Kimi "credential boundary": `deepseek_balance_url_from_base_url` returns the official URL only for an empty base URL (config default) or HTTPS with the exact host `api.deepseek.com` (a `/v1` suffix is accepted); anything else returns `None` and `query_deepseek_balance` bails with "DeepSeek balance API is only available for the official endpoint https://api.deepseek.com". `ProviderInfo::is_deepseek_balance_supported` gates `is_account_query_supported`, so the TUI bottom-bar balance widget is hidden for proxy configurations instead of showing errors. |
| Behavior after | DeepSeek balance polling / `/balance` works only when `base_url` targets the official endpoint. Custom proxies (even with a `deepseek-*` model) report "not supported"; the API key is never sent to `api.deepseek.com` from a proxy configuration. |
| Pointers | `deepseek_balance_url_from_base_url` + test `deepseek_balance_url_derivation` in `crates/tact_llm/src/account.rs`; `is_deepseek_balance_supported` + test `is_deepseek_balance_supported_only_for_official_endpoint` in `crates/tact_llm/src/provider.rs`; docs `book/22_chapter_llm*.md` §3 / §9 |

---

## 1. 2026-08-06 — `/tasks-dag` popup does not show tasks added while it is open

| Field | Value |
|-------|-------|
| Type  | `bugfix` |
| Related | `crates/tui/src/widgets/state/app/agent.rs` (`on_tasks_changed`), `crates/tui/src/render/popups/task_dag_popup.rs`, Ch 23 (TUI) |
| Symptom / motivation | The `/tasks-dag` popup rendered its Mermaid lines once when opened. `TasksChanged` updates refreshed `task_panel.snapshot` but never the popup, and the render loop only re-renders when the popup width changes — so tasks created while the popup was open (or between open and render) never appeared until closing and reopening. |
| Decision | `on_tasks_changed` now refreshes an open DAG popup: it re-runs `render_task_dag_lines` with the latest snapshot at the popup's current `render_width` (falling back to `DEFAULT_DAG_RENDER_WIDTH` before the first width-aware frame) and swaps `lines`/`mermaid_source` in place, keeping the scroll offset. This complements the existing width-change re-render in `render_task_dag_popup`; both paths are idempotent. |
| Behavior after | Newly created tasks appear in the open `/tasks-dag` popup immediately (next render frame) without closing it. |
| Pointers | `on_tasks_changed` in `crates/tui/src/widgets/state/app/agent.rs`; regression test `tasks_dag_popup_refreshes_when_new_tasks_arrive` |

---

## 1. 2026-08-06 — `/tasks-dag` renders missing dependency edges (asymmetric task store)

| Field | Value |
|-------|-------|
| Type  | `bugfix` |
| Related | `crates/tact/src/task/mod.rs` (`update`, `clear_dependency`), `crates/tui/src/widgets/state/task_dag.rs`, Ch 23 (TUI) |
| Symptom / motivation | `/tasks-dag` showed task nodes but **no dependency arrows** for relationships created via `task_update`'s `addBlockedBy`: `update` mirrored `addBlocks` into the blocked task's `blocked_by`, but `addBlockedBy` never mirrored the blocker's `blocks` (outgoing DAG edge). `tasks_to_mermaid` draws edges only from `blocks`, so those dependencies were invisible. Additionally, `clear_dependency` (task completion) removed the completed id from others' `blocked_by` but left the completed task's own `blocks` — a ghost edge source. |
| Decision | `update` now mirrors both directions: `add_blocked_by` also pushes the current task into each blocker's `blocks` (deduped, sorted), symmetric with the existing `add_blocks` branch. `clear_dependency` additionally clears the completed task's `blocks`. Because `update` holds a copy fetched before `clear_dependency`, the local copy is cleared too so the final write-back does not resurrect ghost edges. |
| Behavior after | Every dependency — whether set via `addBlocks` or `addBlockedBy` — renders as `T{blocker} --> T{blocked}` in `/tasks-dag`. Completing a task removes its outgoing edges. |
| Pointers | `crates/tact/src/task/mod.rs` (`update` add_blocked_by branch, `clear_dependency`); tests `update_add_blocked_by_creates_reverse_outgoing_edge`, `completing_task_clears_blocked_by` |

---

## 1. 2026-08-06 — Task completion shows a stats block (elapsed · model · tokens)

| Field | Value |
|-------|-------|
| Type  | `optimization` |
| Related | `crates/tui/src/widgets/state/app/messages.rs` (`add_task_stats_block`), `crates/tui/src/widgets/state/app/agent.rs` (`TaskComplete` branch), Ch 23 (TUI) |
| Symptom / motivation | After a task finished, the log showed only the task-end separator (elapsed label); token consumption and model name were visible only in the bottom status bar, which resets when the next task starts. A `// TODO Add task stats block` marker sat in the `TaskComplete` branch. |
| Decision | The `TaskComplete` branch now calls `add_task_stats_block()` right after `add_task_end_separator()`. The block reads already-frozen state — `last_prompt_elapsed_secs` (set by the separator), `status_bar.model_name` (from `ModelInfo`) and `status_bar.token_*` (from `TokenUsage`) — so no new stats struct or duplicate collection was introduced (YAGNI). It renders one markdown line `📊 任务统计：⏱ mm:ss · 🧠 model · N tokens (prompt X · completion Y · cache Z · reasoning W)` through the existing `add_system_message` path; empty parts (no model, zero tokens) are omitted. |
| Behavior after | Every completed task leaves a persistent stats line under the end separator showing elapsed time, model name, and token breakdown when available. Cancelled or failed tasks do not show the block. |
| Pointers | `add_task_stats_block` in `crates/tui/src/widgets/state/app/messages.rs`; tests `task_complete_appends_task_stats_block`, `task_stats_block_skips_empty_parts` in `crates/tui/src/widgets/state/app/agent.rs` |

---

## 1. 2026-08-06 — `/tasks-dag` renders Mermaid via ratatui-markdown (replaces meraid)

| Field | Value |
|-------|-------|
| Type  | `optimization` |
| Related | `crates/tui/src/widgets/state/task_dag.rs`, `crates/tui/src/render/popups/task_dag_popup.rs`, `crates/tui/src/theme.rs`, root `Cargo.toml` (`ratatui-markdown` git dep), Ch 23 (TUI) |
| Symptom / motivation | The DAG popup used the `meraid` crate's Mono renderer (plain text, no theme, no markdown structure) and the workspace already carried an unused `ratatui-markdown` git dependency (its branch name `update-ratatui-0.30` was stale — the real branch is `chore/update-ratatui-0.30`, so the dependency could not even resolve). |
| Decision | `/tasks-dag` now renders through `ratatui-markdown`: `tasks_to_mermaid` still emits `flowchart TD`, then `render_task_dag_lines` wraps it as `## Tasks DAG` + ` ```mermaid ` block + a `### Legend` list mapping `#id` back to each task's subject (node labels stay narrow: status glyph `○`/`◐`/`✓` + `#id`, since the fork's mermaid grammar terminates `[...]` text at the first `]`). A `DagTheme` adapter maps the app `Theme` into `RichTextTheme`/`MermaidTheme` (light/dark via `MermaidTheme::for_background`), and the popup re-renders at its actual width on first frame (`render_width` cache). The root dependency was fixed to the correct branch with `default-features = false, features = ["markdown", "mermaid"]` (drops `image`/`scroll`/`tree`/`viewer`). |
| Behavior after | The popup shows a themed box-drawing flowchart plus a legend listing task subjects; the `y` copy shortcut still copies the raw Mermaid source. `meraid` is no longer a dependency of the tui crate. |
| Pointers | `crates/tui/src/widgets/state/task_dag.rs` (`tasks_to_mermaid`, `render_task_dag_lines`, `DagTheme`), `crates/tui/src/render/popups/task_dag_popup.rs` (width-aware re-render), tests `tasks_dag_popup_renders_mermaid_markdown`, `ratatui_markdown_renders_diagram_and_legend` |

---

## 1. 2026-08-06 — Compaction summary continues after MaxTokens truncation

| Field | Value |
|-------|-------|
| Type  | `bugfix` |
| Related | Ch 5 §3 (Summarization call), Ch 5 §4 (Validation), `crates/tact/src/agent/mod.rs` (`compact_history_local_with_mode`), `crates/tact/src/recovery.rs` (`MAX_CONTINUATION_ATTEMPTS`, `continuation_message`) |
| Symptom / motivation | When the compaction summary LLM call returned `MaxTokens` (output limit hit), the summary loop treated it as an invalid stop reason and bailed with `compaction summary ended with invalid stop reason: MaxTokens`. Compaction then failed outright even though the partial summary was perfectly usable — and with reasoning models the summary frequently runs up against its output budget. |
| Decision | The summarization call now has two independent recovery axes: transient transport errors still retry with bounded backoff; `MaxTokens` truncation appends the partial summary as an assistant message plus a continuation prompt (same `continuation_message` selector as the main loop: direct-resume on attempt 1, convergence prompt on later attempts) and re-calls, up to `MAX_CONTINUATION_ATTEMPTS` (3). The request is rebuilt per attempt with the growing message history `[User(summary prompt), Assistant(partial), User(continue), …]`. When continuations are exhausted, the partial summary is accepted as best-effort instead of failing (the Codex-style rebuild keeps recent real user messages anyway); `MaxTokens` is therefore no longer an "invalid stop reason". |
| Behavior after | A truncated summary emits `[compact continue n/3] summary truncated, continuing` Info updates and merges all partial blocks into the final summary; compaction succeeds instead of erroring. Refusal / other abnormal stop reasons and empty text still fail without replacing the old context. |
| Pointers | `crates/tact/src/agent/mod.rs` (`compact_history_local_with_mode` summary loop, stop-reason validation), tests `local_compact_continues_truncated_summary`, `local_compact_continues_through_multiple_truncations`, `local_compact_accepts_partial_summary_when_continuations_exhausted`, `crates/tact-ui/tests/recovery_compaction.rs` (`compact_summary_continues_truncated_response`), Ch 5 §3/§4 |

---

## 1. 2026-08-06 — First run auto-writes default config to ~/.tact/config.toml

| Field | Value |
|-------|-------|
| Type  | `feature` |
| Related | Ch 21 §3 (Config Sources and Priority), `config.example.toml`, `crates/tact/src/config/load.rs` |
| Symptom / motivation | The install script ships only the binary — no config. First launch with no config file always failed at resolve with "LLM provider not configured", and the user had to know to copy `config.example.toml` manually (the doc said so, but nothing told them at runtime). |
| Decision | `load_toml_config` now writes the default template to `~/.tact/config.toml` when no config exists on any search path, then parses and returns it. The template is embedded at compile time via `include_str!("../../../../config.example.toml")` so the first-run default never drifts from the checked-in example (currently `deepseek` + `protocol = "chat_completions"`). Only the user-global location is auto-created; project-level candidates (`./.tact/config.toml`, `./config.toml`) are never written so repos stay clean. A hint is printed: `[config] no config found; wrote default template to ... — edit it to add your API key`. |
| Behavior after | First run with no config anywhere: tact-ui creates `~/.tact/config.toml` (template with placeholder `api_key`), prints the edit hint, then fails at resolve on the placeholder key exactly as before — the user edits the file and launches again. Existing configs are never touched or overwritten. Explicit `--config /path` that does not exist is still an error (never auto-created). Write failure (unknown/unwritable HOME) falls back to the previous empty-default behavior. |
| Pointers | `crates/tact/src/config/load.rs` (`DEFAULT_CONFIG_TEMPLATE`, `write_default_config`, `load_toml_config`), `config.example.toml` (header comment), Ch 21 §3 |

---

## 1. 2026-08-06 — Session stats track RTK output-filter metrics

| Field | Value |
|-------|-------|
| Type  | `optimization` |
| Related | `docs/token_usage_schema.md` (Session Stats Display), `crates/tact/src/hook/rtk_filter.rs`, `crates/tact/src/stats.rs` |
| Symptom / motivation | With `tools.rtk_filter = true`, bash outputs are piped through `rtk pipe`, but nothing measured whether the filter actually ran, how much output it removed, or how long it took — users could not tell if RTK was saving tokens or silently passing everything through. |
| Decision | `SessionStats` gains six relaxed-atomic counters (`rtk_calls`, `rtk_success_calls`, `rtk_failure_calls`, `rtk_saved_chars`, `rtk_input_chars`, `rtk_elapsed_ms`) mutated directly by the post-tool hook. Atomics (not plain `u64`) are required because hooks only receive `&Agent` (immutable). An attempt counts as a success only when `rtk pipe` exits 0 with non-empty stdout; saved chars are `raw_len − filtered_len` (saturating, counted in chars not bytes) on success only. `rtk_input_chars` accumulates every attempt's raw length (success or failure) so the session-wide savings rate reflects failed attempts as zero savings. The end-of-session summary gains an `RTK tokens saved` estimate using the 1 token ≈ 4 chars length heuristic and an `RTK savings rate` row (saved chars / input chars). |
| Behavior after | Whenever at least one RTK attempt was recorded, the session stats popup / exit summary shows `RTK calls (s/f)`, `RTK chars saved`, `RTK tokens saved` (chars/4), `RTK savings rate` (saved/input %), and `RTK time`. Rows are hidden entirely when `rtk_filter` is off or no bash output was filtered. Failed bash executions (`StepStatus::Failed`) are excluded from both filtering and RTK stats — their output reaches the LLM intact. |
| Pointers | `crates/tact/src/stats.rs` (`SessionStats::record_rtk`, RTK rows in `summary()`), `crates/tact/src/hook/rtk_filter.rs` (`pipe_through_rtk` → `(output, succeeded, elapsed_ms)`, `saved_chars`, `should_filter`, `create_rtk_post_tool_hook`), `crates/tact/src/hook/mod.rs` (`PostToolUseFn` now receives `StepStatus`), `docs/token_usage_schema.md` |

---

## 1. 2026-08-05 — Unify tool-family card labels (background + team)

| Field | Value |
|-------|-------|
| Type  | `optimization` |
| Related | Ch 7, Ch 26 (2026-07-28 entry: Distinct tool-card labels) |
| Symptom / motivation | Two families still had mixed labels: `background_run` (`⚙️ Background Run`) vs `check_background` (`🔍 Check`) — the bare `Check` did not say what it checks; and team collaboration tools `send_message` / `broadcast` / `read_inbox` / `plan_approval` (`✉️ Send` / `📢 Broadcast` / `📬 Inbox` / `✅ Approve`) had no `Team` prefix while `spawn_teammate` / `list_teammates` (`👥 Team Spawn` / `👥 Team List`) did. |
| Decision | Background family: rename `check_background` to `⚙️ Background Check`, sharing the `⚙️ Background` prefix with `background_run`. Team family: add the `Team` family name to the four collaboration tools, keeping their distinct icons (`✉️ Team Send` / `📢 Team Broadcast` / `📬 Team Inbox` / `✅ Team Approve`). Update the TUI `tool_display_name` fallback to match metadata in both families. Task stays on `# Task…` human titles via `format_task_tool_title`. |
| Behavior after | Every family reads as one: `⚙️ Background Run` / `⚙️ Background Check`; `👥 Team Spawn` / `👥 Team List` / `✉️ Team Send` / `📢 Team Broadcast` / `📬 Team Inbox` / `✅ Team Approve`; `⏰ Cron …`; `🌿 Worktree …`; `🔌 Shutdown …`. |
| Pointers | `crates/tact/src/tool/background_run.rs` (`CHECK_BACKGROUND_METADATA`); `crates/tact/src/tool/team.rs`; `crates/tui/src/widgets/tool_widget.rs` (`tool_display_name`) |

---

## 1. 2026-08-05 — `/model` 按 provider 分流 budget/effort + model→档位映射 + effort/model per-agent

| Field | Value |
|-------|-------|
| Type | `feature` |
| Related | `docs/superpowers/specs/2026-08-05-llm-presets-design.md`, Ch 21 (config: `[llm.model_profiles]`, `reasoning_effort` 校验), Ch 22 (§2 ProviderInfo 静态化, §6.3 wire 表) |
| Symptom / motivation | `/model` 对任何 provider 都弹同一 5 档 thinking budget，但 openai/deepseek/kimi-k3 实际发的是 `reasoning_effort`；effort 无选择入口、运行时不可改；effort 是进程全局共享（subagent 污染主 agent）；OpenAI Responses 的 effort 是 client 构建时 snapshot，运行时修改不生效；"模型↔档位"无静态配置。 |
| Decision | 1) `/model`（及 `/model-subagent`）第二步按 `model_uses_effort` 分流：openai/deepseek/kimi k3/k3-256k → effort 选择器（deepseek 3 档 low/high/max，kimi k3 3 档，openai 6 档 minimal..max，无 none 档）；anthropic/kimi coding 系 → budget 选择器。2) 新增 `[llm.model_profiles."<model>"]`（`thinking_budgets` / `reasoning_efforts` 数组）限定第二步档位，TOML 逐字段覆盖内置 `builtin_model_profiles()`。3) **effort/model per-agent**：`CreateMessageParams.reasoning_effort` + `AgentSettings.model/reasoning_effort`；删全局 `set_model`、`ProviderInfo.reasoning_effort`、`current_reasoning_effort_from_budget` 及 budget→effort 波段映射（无存量兼容）；`/model` 改发 `UserCommand::SetModel` / `SetReasoningEffort`（busy 排队）。4) wire 注入全部从 request 读；DeepSeek 纯 effort 驱动（None=不传，默认 ON+high，按官方文档）；Kimi k3 支持 effort（None=默认 high；不提供关闭 thinking——会路由到 K2.6）；OpenAI Responses `create_response` 不再 snapshot effort。5) 持久化：effort 语义写 provider/subagent 的 `reasoning_effort` 字段（`[llm.model_profiles]` 是静态选项集合，不被持久化触碰）；resolve 校验放宽为 openai/deepseek/kimi。 |
| Behavior after | `/model` 选 openai/deepseek/kimi-k3 模型 → effort 选择器（映射档位或 provider 默认）；选 anthropic/kimi-coding 模型 → budget 选择器（现状）。运行中改 model/effort 只影响当前 agent（主/subagent 独立），wire 立即跟随；Responses 也跟随。持久化后重启生效。Kimi 关闭思考被路由到 K2.6 的行为未提供 UI 入口。 |
| Pointers | `crates/tui/src/handlers/select.rs`（分流/选择器）、`crates/tact/src/config/{types,resolve,persist,mod}.rs`（model_profiles/校验/持久化）、`crates/tact_llm/src/{provider,deepseek,kimi,openai/*}.rs`（per-request effort/wire）、`crates/tact/src/agent/mod.rs`（SetModel/SetReasoningEffort）、Ch 21/22。 |

---

## 2. 2026-08-04 — `tact upgrade` self-upgrade command

| Field | Value |
|-------|-------|
| Type | `feature` |
| Related | README (CLI / self-upgrade), Ch 21 (config: `install_without_llm` path) |
| Symptom / motivation | Users had no in-place way to move to a newer release; upgrades meant re-running `scripts/install.sh` (or rebuilding from source) by hand. |
| Decision | Add a `tact upgrade` CLI subcommand. It scans the GitHub releases list (`GET /repos/{repo}/releases?per_page=100`) for the newest non-draft, non-prerelease release whose assets include `tact-ui-v<ver>-<triple>.tar.gz` for the current platform — skipping tags published without build assets (e.g. `v1.1.1` initially shipped with zero assets) — downloads the archive, verifies it against the release's published `SHA256SUMS`, and atomically replaces the running binary on Unix. Flags: `--check` (print only), `--yes` (skip the y/N prompt), `--repo owner/name` or `TACT_UPGRADE_REPO` (track a fork). Windows is not yet supported in-place; the command points users at `scripts/install.ps1`. The command resolves config via `install_without_llm` so no LLM provider is required. |
| Behavior after | `tact-ui upgrade --check` prints current vs latest-installable; `tact-ui upgrade` prompts and (on `y`/`--yes`) downloads, verifies SHA-256, and replaces the executable, printing a restart hint. A mismatched checksum aborts before replacement. |
| Pointers | `crates/tact/src/upgrade.rs` (`run_upgrade`, `find_latest_release_with_asset`, `replace_current_binary`), `crates/tact/src/config/cli.rs` (`CliCommand::Upgrade`), `crates/tact-ui/src/main.rs` (dispatch), README §3 Run |

---

## 1. 2026-08-04 — Google voice transcription honors standard proxy environment variables

| Field | Value |
|-------|-------|
| Type | `bugfix` |
| Related | Ch 21, Ch 23 |
| Symptom / motivation | The Google Speech-to-Text client was constructed with `reqwest::ClientBuilder::no_proxy()`. On networks where `speech.googleapis.com` is reachable only through `HTTP_PROXY` / `HTTPS_PROXY` / `ALL_PROXY`, recording completed but transcription bypassed the configured proxy and timed out or failed to connect. |
| Decision | Remove the Google client's forced proxy bypass and let reqwest apply its standard proxy-environment resolution. Keep API-key-safe error reporting: connection errors are not rendered with the request URL because the Google API key is currently carried in the query string. |
| Behavior after | Google voice transcription follows the process proxy environment, including the corresponding lowercase variable names and `NO_PROXY`; direct connections still work when no proxy is configured. A child-process regression test verifies that a request reaches a configured HTTP proxy rather than the original host. |
| Pointers | `crates/tact/src/voice/transcriber.rs` (`GoogleTranscriber::new`, `google_transcriber_honors_http_proxy`); Ch 21 (voice configuration), Ch 23 (TUI voice flow) |

---

## 1. 2026-08-02 — Pre-push hook no longer leaks `GIT_DIR`/`GIT_WORK_TREE` into `cargo test`

| Field | Value |
|-------|-------|
| Type | `bugfix` |
| Related | `scripts/check-rust.sh`, `.githooks/pre-push`, `crates/tact-ui/tests/subsystem_tools.rs` |
| Symptom / motivation | Git exports `GIT_DIR` / `GIT_WORK_TREE` into hook processes. The pre-push hook ran `cargo test -p tact-ui`, and the `worktree_create_lists_and_shows_status` test spawned `git` commands that inherited those variables. Instead of operating on the test's isolated temp repo, the commands targeted the real repo: the test's `git worktree add` registered a stray worktree, and its setup `git init`/`git add`/`git commit` appended destructive `init` commits to the active branch's HEAD — so `git push` could push a repository that had just been polluted by its own pre-push test run. |
| Decision | Clear the hook-injected variables in both hook entry points before any child process runs: `unset GIT_DIR GIT_WORK_TREE` at the top of `.githooks/pre-push` and of `scripts/check-rust.sh`. Also re-point `core.hooksPath` at `.githooks` so git actually uses the version-controlled hook (the previously installed `.git/hooks/pre-push` was a stale inline copy). |
| Behavior after | `git push` runs fmt/clippy/build/test with a clean git environment; integration tests create worktrees and commits only inside their `tact-tool-test-*` temp dirs, never in the real repo. |
| Pointers | `.githooks/pre-push`; `scripts/check-rust.sh`; `scripts/install-git-hooks.sh`; `crates/tact/src/worktree/mod.rs` (still `current_dir`-based; hooks must not leak env); `crates/tact-ui/tests/subsystem_tools.rs` |

---

## 1. 2026-08-02 — Google Cloud API-key voice transcription provider

| Field | Value |
|-------|-------|
| Type | `feature` |
| Related | Ch 21, Ch 23 |
| Symptom / motivation | Voice input supported OpenAI-compatible transcription and local `whisper.cpp`, but users with a Google Cloud Speech-to-Text API key had no direct provider. |
| Decision | Add `VoiceProvider::Google` using synchronous `POST {base_url}/speech:recognize?key=...` with base64 LINEAR16 mono 16 kHz WAV JSON. Reuse `voice.api_key`, `voice.language`, and `voice.model`; default to `https://speech.googleapis.com/v1` and `latest_short`. Limit Google recordings to `1..=60` seconds. Service Accounts, OAuth, long-running recognition, streaming, and automatic segmentation remain out of scope. |
| Behavior after | Configuring `provider = "google"` sends a short recording to Google Cloud and concatenates returned `results[].alternatives[0].transcript` values into the existing TUI input flow. Missing keys, HTTP failures, malformed/empty responses, and cancellation are reported without exposing credentials. |
| Pointers | `crates/tact/src/config/{types.rs,resolve.rs}`; `crates/tact/src/voice/transcriber.rs`; `docs/superpowers/specs/2026-08-02-google-voice-transcription-design.md`; Ch 21, Ch 23 |

---

## 1. 2026-08-02 — Compaction handoff is now a typed message cell

| Field | Value |
|-------|-------|
| Type | `optimization` |
| Related | Ch 5 |
| Symptom / motivation | The Codex-style rebuild appended the handoff as a plain `Role::User` text message; the only "special handling" was string-prefix matching (`is_summary_message`). The model could not distinguish a system-generated handoff from a real user turn, consecutive `[User: summary][User: prompt]` messages risked provider-side merging, and detection was fragile (prefix-only, lost on non-Text cells). |
| Decision | Make the handoff a first-class message cell: `MessageKind::Summary` on `tact_llm::Message` (`#[serde(skip)]`, in-memory only — the Anthropic wire, OpenAI conversion, and JSONL transcripts stay byte-identical) plus `<context-handoff>` … `</context-handoff>` framing in the cell text. Detection is by kind first, with `SUMMARY_PREFIX` / tag string fallback for sessions reloaded from the SQLite store (which persists only role + content). |
| Behavior after | `build_compacted_history` / `compacted_context` emit a framed, kind-marked cell: `<context-handoff>\nThis conversation was compacted…\n\n{summary}\n</context-handoff>`. `collect_user_messages` skips it by type; reloaded sessions are re-detected by content. Wire format is unchanged for Normal messages; Anthropic never sees `kind`. |
| Pointers | `crates/tact_llm/src/content.rs` (`MessageKind`, `Message::with_kind/is_summary`); `crates/tact/src/compact/mod.rs` (`summary_message`, `is_summary_message`, `build_compacted_history`, `compacted_context`); `crates/tact/src/store/session_store/sqlite.rs` (`load_session`); `book/05_chapter_compact.md` |

## 1. 2026-08-02 — DeepSeek can now use the OpenAI Responses protocol

| Field | Value |
|-------|-------|
| Type | `feature` |
| Related | Ch 21, Ch 5 |
| Symptom / motivation | `protocol = "responses"` was rejected for every non-OpenAI provider, so DeepSeek was pinned to Chat Completions even though the Responses adapter is endpoint-agnostic and the DeepSeek endpoint can serve `/responses`. |
| Decision | Accept `responses` for the DeepSeek provider in `resolve_llm` and route `ProviderInfo::build_client()` by protocol: DeepSeek + `chat_completions` keeps the dedicated `DeepSeekAdapter`; DeepSeek + `responses` builds the same generic `OpenAiResponsesAdapter` used by OpenAI, pointed at the DeepSeek `base_url`. Automatic `context_management` compaction, `reasoning.effort` from `thinking_budget`, and Responses conversation-state continuation apply unchanged. Kimi and Anthropic still reject `responses`. |
| Behavior after | A DeepSeek entry may set `protocol = "responses"`; requests go to `{base_url}/responses` with automatic compaction and reasoning semantics. Explicit `POST /responses/compact` is not implemented by the DeepSeek endpoint (live-verified 2026-08-02), so DeepSeek + Responses compacts through the local summary pipeline and clears the stale baseline; OpenAI Responses keeps the strict no-fallback contract. The default remains `chat_completions`. |
| Pointers | `crates/tact/src/config/resolve.rs` (`resolve_llm` validation); `crates/tact_llm/src/provider.rs` (`build_client`); `docs/superpowers/specs/2026-08-02-deepseek-responses-design.md`; `docs/superpowers/plans/2026-08-02-deepseek-responses.md`; Ch 21 (config), Ch 5 (compaction) |

## 1. 2026-08-01 — Responses compact threshold now reaches ordinary `/responses` requests (native `context_management`)

| Field | Value |
|-------|-------|
| Type | `bugfix` |
| Related | Ch 5, Ch 22, Ch 23 |
| Symptom / motivation | `responses_compact_threshold` (and its derived value) was resolved and validated, but the resolved threshold was never handed to the Responses adapter: ordinary `stream_message` / `create_message` calls built `/responses` bodies with `context_management` hard-disabled (`None`). Automatic provider-side compaction was therefore silently off in production, and only the explicit `/responses/compact` path worked. |
| Decision | Wire the resolved threshold through the whole configuration → adapter chain and send it on **every ordinary** `/responses` request: `LlmSettings.provider_info()` → `ProviderInfo.responses_compact_threshold` → `OpenAiResponsesAdapter` → `create_response` (`context_management: [{ "type": "compaction", "compact_threshold": N }]`). Native state is persisted and replayed: the opaque baseline (`input_items`, `compaction_id`, `logical_context_hash`) is committed atomically with messages and replayed verbatim on later requests. Endpoints lacking native Responses compaction are unsupported — **no** local summary fallback. |
| Behavior after | A configured/derived threshold produces `context_management` on every ordinary `/responses` request (stream and non-stream). The endpoint may compact the baseline automatically mid-conversation; a returned `compaction` item round-trips as opaque state and is never rendered. Explicit compaction (`/compact`, auto trigger, recovery) sends `POST /responses/compact` and replaces the baseline atomically; diagnostics show item count and compaction id only, never `encrypted_content`. Regression tests assert the wire body carries `context_management` when configured and omits it when not. |
| Pointers | `crates/tact_llm/src/openai/responses/convert.rs` (`create_response` → `context_management`); `crates/tact_llm/src/openai/responses/mod.rs` (`OpenAiResponsesAdapter::build_wire_request`, wiremock regression tests); `crates/tact_llm/src/provider.rs` (`ProviderInfo.responses_compact_threshold`); `crates/tact/src/config/types.rs` (`LlmSettings::provider_info`); `crates/tact/src/config/resolve.rs` (threshold derivation); `crates/tact/src/agent/mod.rs` (`compact_responses_native`, atomic `replace_persisted_context_and_state`); `docs/token_usage_schema.md` (automatic vs explicit compaction accounting); Ch 5, Ch 22, Ch 23 |

---

## 1. 2026-08-01 — Empty fenced block after markdown list no longer hijacks the tail line into a code card

| Field | Value |
|-------|-------|
| Type | `bugfix` |
| Related | Ch 23, Ch 24 |
| Symptom / motivation | In the TUI log stream, an empty-language fenced block (plain ```) appearing immediately after an in-progress markdown list/paragraph could be promoted into a standalone code card too early. The trailing line that followed the fence then rendered inside the code card instead of staying in normal markdown flow, making the tail line look “swallowed” or mis-rendered. This was a Tact rendering bug, not a Responses-protocol issue. |
| Decision | Keep the existing code-card path for real streamed code blocks (for example ```rust), but stop promoting **empty-language** fences into code cards when they appear directly after an in-progress markdown paragraph/list. In that case, keep the fence line in the markdown paragraph buffer and let the normal markdown renderer handle it. Add a high-level log regression test for the list → empty fence → tail-line case, plus a low-level markdown test proving the parser layer itself did not lose the tail line. |
| Behavior after | A markdown list followed by an empty fence snippet no longer turns the remaining tail line into a `Click for full code` card. Real language-tagged streamed code blocks still render as code cards. |
| Pointers | `crates/tui/src/widgets/state/app/agent.rs` (stream fence promotion guard); `crates/tui/src/render/render_gap_tests.rs` (`log_markdown_list_then_empty_fence_stays_in_markdown_flow`); `crates/tui/src/render/render_md.rs` (`render_markdown_list_then_fenced_code_then_list_tail`); Ch 23, Ch 24 |

## 1. 2026-07-28 — Theme detection fallback wrong theme (Ink vs Retro)

| Field | Value |
|-------|-------|
| Type | `bugfix` |
| Related | Ch 23 |
| Symptom / motivation | `detect_terminal_theme()` doc comment said "Fallback: Retro" but the code returned `ThemeName::Ink`. The unit test matching this contract (`test_detect_terminal_theme_env_vars`) expected the fallback to be `Dark`, `Light`, or `Retro`, so it failed on `Ink`. CI broke consistently for any runner without `COLORFGBG` / `COLORTERM` and no macOS dark-mode override. |
| Decision | Change the fallback return from `ThemeName::Ink` to `ThemeName::Retro`, matching the doc comment and test expectation. |
| Behavior after | When no terminal theme env vars are set, `detect_terminal_theme()` returns `Retro` (neutral dark) instead of `Ink`. |
| Pointers | `crates/tui/src/theme_detection.rs` |

---

## 1. 2026-07-28 — Log left-border scrollbar residue

| Field | Value |
|-------|-------|
| Type | `bugfix` |
| Related | Ch 23 |
| Symptom / motivation | On Ink and similar themes, wide graphemes in Thinking card titles (e.g. 🧠) could briefly desync some terminals' cursors while the accent scrollbar thumb (formerly `█`) was painted. Ghost thumb cells then stuck on the Log left border as intermittent light-blue “shadows”. Because unchanged border cells are skipped by `Buffer::diff`, the residue persisted across frames. |
| Decision | After content and scrollbar draw, re-stamp the left vertical border every frame and mark those cells `CellDiffOption::AlwaysUpdate`. Switch the thumb glyph to half-block `▐` so a momentary desync is less visually harsh. |
| Behavior after | The left border is force-emitted each frame in the theme `border` color; accent residue from wide-title desync cannot persist on the chrome column. |
| Pointers | `crates/tui/src/render/log.rs` (`restamp_log_left_border`); `crates/tui/src/render/log_render_tests.rs` |

---

## 1. 2026-07-28 — Distinct tool-card labels for CRUD-style tool families

| Field | Value |
|-------|-------|
| Type  | `optimization` |
| Related | Ch 7, Ch 13–16, Ch 23 |
| Symptom / motivation | Cron / worktree / team family tools shared one display label (e.g. all cron ops showed `⏰ Cron`). Header titles looked identical unless the user parsed `arg_summary` JSON. Generic `visual_kind` also ignored metadata `display_name` and always used the TUI fallback map. |
| Decision | Append the verb to each shared family label (`⏰ Cron Create` / `Delete` / `List`, same pattern for Worktree / Team / Shutdown). Align `tool_display_name` fallbacks. Prefer non-empty presentation `display_name` when it differs from the raw tool id so metadata is the source of truth for Generic tools. Leave Task alone — it already uses `# Task…` human titles via `format_task_tool_title`. |
| Behavior after | Tool cards show distinct action labels at a glance. `background_run` / `check_background` fallbacks match metadata (`⚙️ Background Run` / `⚙️ Background Check`). |
| Pointers | `crates/tact/src/tool/{cron,worktree,team}.rs`; `crates/tui/src/widgets/tool_widget.rs` (`display_name_from_presentation`, `tool_display_name`) |

---

## 1. 2026-07-28 — Bash tool card label restored (`$ Bash`)

| Field | Value |
|-------|-------|
| Type  | `bugfix` |
| Related | Ch 7, Ch 23 |
| Symptom / motivation | After binding builtin `ToolPresentation` beside handlers, `bash` used `display_name: "$ Shell"`. The TUI card showed **Shell** even though the tool id and legacy fallback remain `bash` / `$ Bash`. |
| Decision | Set `BASH_METADATA.presentation.display_name` back to `"$ Bash"`. Runtime still spawns `sh -c` (unchanged). |
| Behavior after | Tool cards and titles render `$ Bash` again for the `bash` tool. |
| Pointers | `crates/tact/src/tool/bash.rs`; fallback still `$ Bash` in `crates/tui/src/widgets/tool_widget.rs` |

---

## 1. 2026-07-28 — Voice keybind ate all keyboard input

| Field | Value |
|-------|-------|
| Type  | `bugfix` |
| Related | Ch 21, Ch 23 |
| Symptom / motivation | With `voice.voice_keybind` set, the TUI `if let Some(keybind) = … else if …` chain treated *any* key as handled by the voice branch whenever the option was present. Non-matching keys never reached `handle_insert_mode` / Normal dispatch, so the input box appeared to reject typing. |
| Decision | Match the configured shortcut first; only then skip normal dispatch. On non-match, fall through to slash / overlay / mode handlers unchanged. |
| Behavior after | `voice_keybind = "ctrl+g"` toggles recording only on that chord. All other keys type and navigate as before. Unset keybind keeps the previous mouse-only path. |
| Pointers | `crates/tui/src/lib.rs` (key event dispatch); `crates/tui/src/widgets/state/app/voice.rs` (`toggle_voice_recording`); Ch 21 `[voice]`, Ch 23 §6.6 |

---

## 1. 2026-07-28 — Input title-bar border restored; voice button centered

| Field | Value |
|-------|-------|
| Type  | `bugfix` |
| Related | Ch 23 |
| Symptom / motivation | Centering the voice label with space-padding plus a background style overwrote the Block top-border cells between the Input title and `🎙 Voice`, so the horizontal line looked “eaten”. |
| Decision | Render the left Input title and the voice label as two `Block` titles (left + `Alignment::Center`) instead of one padded span. Click hit-testing uses the same centered geometry. |
| Behavior after | With voice enabled, the top border remains visible between the Input label and the centered voice control on wide enough terminals. Narrow widths may still collide (ratatui left title paints after center). |
| Pointers | `crates/tui/src/render/input.rs` (`voice_title`, `update_voice_button_area`); Ch 23 §6.6 |

---

## 1. 2026-07-28 — Configurable voice recording keybind

| Field | Value |
|-------|-------|
| Type  | `feature` |
| Symptom / motivation | Voice recording could only be started via mouse click on the title-bar button. Keyboard-centric users had no way to trigger it without reaching for the mouse. |
| Decision | Add `voice.voice_keybind` config option accepting `ctrl+<char>` format (e.g. `"ctrl+g"`, `"ctrl+r"`). When set, pressing the configured shortcut toggles voice recording in any input mode (idle → record, recording → stop). When unset (default), voice remains mouse-only. The active keybind is shown in the help panel (`Ctrl+?`) under Global shortcuts. Only an exact keybind match consumes the event. |
| Behavior after | `[voice] voice_keybind = "ctrl+g"` in config.toml enables keyboard-triggered voice. The shortcut works globally (any input mode). Non-matching keys still reach Insert/Normal handlers. The help panel dynamically shows the configured key. Empty, multi-character, or non-ctrl keybinds are rejected at config resolution. |
| Pointers | Config: `crates/tact/src/config/types.rs`, `config/resolve.rs`, `config.example.toml`; TUI dispatch: `crates/tui/src/lib.rs` (global shortcut section), `crates/tui/src/widgets/state/app/voice.rs`; Help: `crates/tui/src/widgets/help_widget.rs`, `render/popups/help.rs`; i18n: `crates/tui/src/i18n.rs` (`help_voice_record_tmpl`); Ch 21, Ch 23 |

---

## 1. 2026-07-28 — Permission: shell Write risk, settings allow for High, headless ask defaults

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | Ch 10 |

**Symptom / motivation:** Three logic bugs: (1) `PermissionPolicy::ShellCommand` classified non-elevated commands as Read, so `bash` / `background_run` / `worktree_run` bypassed Default-mode prompts; (2) headless `ask_user` always denied, which made Default mode unusable without a TUI; (3) High-risk tools ignored settings **allow** rules and always asked.

**Decision:** Non-elevated shell → Write; `sudo`/`su` → High. Non-interactive `ask_user(tool, risk)` allows Write/Read once and denies High. Settings Deny/Allow apply at all risks; High without Deny/Allow still asks and skips the in-session bare allowlist.

**Behavior after:** Normal shell calls prompt (or headless-allow) like other writes. Project allow rules can approve High for a matching input pattern. Unattended High still needs Auto mode or an explicit allow rule.

**Pointers:** `crates/tact/src/permission/mod.rs`, `crates/tact/src/tool/metadata.rs`, `crates/tact/src/agent/tool_dispatch.rs`; Ch 10.

---

## 1. 2026-07-28 — `/model` thinking budget not synced to status bar

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | Ch 21, Ch 23 |

**Symptom / motivation:** After `/model` saved a new thinking budget (e.g. 32K), the bottom bar could still show the previous value (e.g. `think high(64K)`). Persist succeeded; the running agent and bar did not.

**Decision:** `UserCommand::SetThinkingBudget` is processed only after an in-flight task finishes. That task’s older `ModelInfo` overwrote the TUI’s optimistic update, and `set_thinking_budget` did not emit a fresh `ModelInfo`. Emit `ModelInfo` from `set_thinking_budget`, and expand/sync session `max_tokens` in the TUI apply path so `out` / `think` stay aligned.

**Behavior after:** Confirming a budget updates the status bar immediately; when the queued agent command runs, another `ModelInfo` resyncs `thinking_budget` and any auto-expanded `max_tokens`.

**Pointers:** `crates/tact/src/agent/mod.rs` (`set_thinking_budget` / `emit_model_status`), `crates/tact/src/config/mod.rs` (`update_llm_model_and_thinking_budget`), `crates/tui/src/handlers/select.rs` (`apply_model_and_budget_pick`), `crates/tact-ui/src/driver.rs`.

---

## 1. 2026-07-28 — Clickable voice-to-text input (title bar)

| Field | Value |
|-------|-------|
| **Type** | optimization |
| **Related** | Ch 21, Ch 23; `docs/superpowers/specs/2026-07-28-voice-to-text-design.md`; `docs/superpowers/plans/2026-07-28-voice-to-text-input.md` |

**Symptom / motivation:** Keyboard-only input is awkward for long prompts on macOS; users wanted hands-free capture with a chance to review before submit.

**Decision:** Add `[voice]` config (independent API key), `tact::voice` worker (cpal capture → WAV → OpenAI-compatible transcription), and a right-aligned title-bar button in the TUI. Successful transcripts insert at the UTF-8 cursor; `/help` in a transcript stays plain text until Enter. Recording/transcription run off the event loop; `Esc` or Stop cancels.

**Behavior after:** `enabled = false` (default) hides the control. `enabled = true` shows the button; missing `[voice].api_key` flashes a config hint on click. No interim transcription, auto-submit, or local Whisper in this release.

**Pointers:** `crates/tact/src/voice/`, `crates/tui/src/widgets/state/voice.rs`, `crates/tui/src/render/input.rs`, `crates/tui/src/handlers/mouse.rs`, `crates/tui/src/handlers/insert.rs`, `crates/tui/src/lib.rs`, `crates/tact-ui/src/interactive.rs`.

---

## 1. 2026-07-28 — Subagent metadata rendered in tool-card header

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | Ch 12, Ch 23; `docs/token_usage_schema.md` |

**Symptom / motivation:** Subagent `TokenUsage` and `ModelInfo` were forwarded to the shared parent UI channel as `ToolProgress` inline chunks, producing repetitive `⚡ N tokens` and `🤖 Model: …` lines in the output stream. TokenUsage also overwrote the main agent's bottom-bar meters.

**Decision:** Introduce `AgentUpdate::ToolMeta` — a dedicated update path that writes model name and token count directly to the parent tool card's header row, alongside the existing phase/duration info. The forwarder no longer emits `ToolProgress` chunks for these events and no longer forwards them to the shared channel. The tool-card meta row now shows `🤖 {model} · ⚡ {total}` for subagent invocations.

**Behavior after:** Bottom bar consistently shows main-agent token stats. Subagent model and token total appear in the tool card's meta row (e.g. `⠋ Running · 🤖 deepseek-v3 · ⚡ 4.2K · 3.2s`), updated live via `ToolMeta` and preserved on completion. No inline clutter in the output stream.

**Pointers:** `crates/tact/src/tool/subagent_ui.rs`, `crates/tui/src/widgets/tool_widget.rs`, `crates/tui/src/render/cells/tool.rs`, `crates/tui/src/widgets/state/app/agent.rs`, `crates/protocol/src/agent.rs`; `docs/token_usage_schema.md`; Ch 12, Ch 23.

---

## 1. 2026-07-27 — Permission settings persistence (JSON-based dynamic rules)

| Field | Value |
|-------|-------|
| **Type** | docs |
| **Related** | Ch 7, Ch 21; `docs/superpowers/specs/2026-07-27-permission-settings-design.md`; `docs/superpowers/plans/2026-07-27-permission-settings.md` |

**Symptom / motivation:** Permission decisions were only stored in session-scoped memory (`always_allowed_tools`). The "Always allow this tool" choice granted every invocation of a bare tool name with no parameter awareness, persisted nowhere between sessions, and there was no way to pre-configure deny or ask rules without modifying `config.toml` (a TOML file not designed for dynamic rule writes).

**Decision:** Introduce JSON-based permission settings with two scopes: `$HOME/.tact/settings.json` (global) and `<workdir>/.tact/settings.json` (project). Rules use a Claude-like tool-and-argument syntax (`tool(field:pattern)`) with glob matching. Precedence is `deny > ask > allow`, independent of array order. Project writes are atomic (temp file + rename), preserve unknown JSON fields, and suppress duplicates. Malformed files or invalid rules are soft failures (warn + skip). High-risk confirmation remains mandatory regardless of allow rules.

**Behavior after:** Dynamic allow/ask/deny rules live in JSON settings files — not in `config.toml`. "Always allow this tool" writes a parameter-aware rule (e.g. `bash(command:cargo test *)`) to the project file. Missing files are empty policies. The TOML `[permission].mode` continues to control mode only (`default` | `plan` | `auto`). Plan and Auto mode semantics are unchanged.

**Pointers:** `crates/tact/src/permission/settings.rs`, `crates/tact/src/permission/mod.rs`, `crates/tact/src/consts.rs`, `crates/tact/src/agent/tool_dispatch.rs`, `crates/tact/src/tool/subagent.rs`, `crates/tact-ui/src/interactive.rs`, `crates/tact-ui/src/headless.rs`; `docs/superpowers/specs/2026-07-27-permission-settings-design.md`; `docs/superpowers/plans/2026-07-27-permission-settings.md`; `docs/state_machines.md §5`; `config.example.toml`; Ch 7, Ch 21.

## 1. 2026-07-27 — Log scroll restores the theme background

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | Ch 23; `docs/superpowers/specs/2026-07-27-log-scroll-artifact-design.md`; `docs/superpowers/plans/2026-07-27-log-scroll-artifact-fix.md` |

**Symptom / motivation:** After scrolling away from a code-card or other styled Log content, a normal text row could retain a prior frame's background style. The artifact was especially visible on the dark Ink theme as a shadow behind text.

**Decision:** Keep the Log viewport reset and make `TextCell` explicitly apply the active `theme.bg` while writing each normal glyph. The rule is theme-independent; card and overlay layers keep their existing backgrounds and order.

**Behavior after:** Any ordinary Log row newly exposed by scrolling has the active theme's background, while its foreground styling and selection reverse modifier remain intact. No Ink-only branch or global terminal clearing policy is used.

**Pointers:** `crates/tui/src/render/log.rs`; `crates/tui/src/render/cells/text.rs`; `crates/tui/src/render/log_render_tests.rs`; `docs/superpowers/specs/2026-07-27-log-scroll-artifact-design.md`; Ch 23.

---

## 1. 2026-07-27 — Subagent popup shows its model

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | Ch 12, Ch 23; `docs/token_usage_schema.md` |

**Symptom / motivation:** The live/completed `spawn_subagent` popup showed the child call's token total, cache rate, and prompt context, but not the model that produced them. The agent emits `ModelInfo`, but the subagent UI forwarder discarded that event.

**Decision:** Format the child `ModelInfo` as a structural popup-transcript line: `🤖 Model: {model}`. Keep it on the `ToolProgress` path rather than forwarding it to the shared parent UI channel.

**Behavior after:** Every child model call adds its model name to that child popup alongside its existing token line. The parent bottom bar retains the parent agent's model name (see 2026-07-28 for the matching TokenUsage fix).

**Pointers:** `crates/tact/src/tool/subagent_ui.rs`; `docs/token_usage_schema.md`; Ch 12, Ch 23.

---

## 1. 2026-07-27 — Ink themes + unified popup chrome

| Field | Value |
|-------|-------|
| **Type** | optimization |
| **Related** | Ch 21, Ch 23; `docs/tui_rendering.md` |

**Symptom / motivation:** Default theme was `retro`; popup overlays had inconsistent border types, hardcoded colors, no shared chrome.

**Decision:** Added `ink`/`ink-light` themes with pixel-matched colors, `heading`/`version`/`muted` Theme fields, unified `render_popup_chrome` for all overlays. Default changed to `ink`.

**Behavior after:** Default theme is `ink`; all overlay popups share a consistent border, title bar (bold title, `[x]` hint), and footer layout; popup code is DRY.

**Pointers:** `crates/tui/src/theme.rs`, `crates/tui/src/render/popups/mod.rs`, `crates/tui/src/render/render_md.rs`, `crates/tact/src/config/resolve.rs`

---

## 1. 2026-07-26 — Subagent tool renamed `task` → `spawn_subagent`

| Field | Value |
|-------|-------|
| **Type** | optimization |
| **Related** | Ch 7, Ch 10, Ch 11, Ch 12, Ch 19 |

**Symptom / motivation:** The subagent spawn tool was named `task`, sharing a prefix with the four persistent-task tools (`task_create` / `task_get` / `task_list` / `task_update`) while meaning something entirely different. Models and readers conflated "the `task` tool finished" with "the task record is complete" — an observed failure left a checklist item Pending after its subagent had returned. Ch 1 / 11 / 12 / 19 each carried a disambiguation note as a workaround.

**Decision:** Rename the tool to `spawn_subagent` (verb + object, matching its description); wrapper type `TaskTool` → `SpawnSubagentTool`, handler `task()` → `spawn_subagent()`. The persistent-task tools keep the `task_*` prefix. `spawn_subagent` remains `CapabilityRisk::High` and remains a scheduling barrier.

**Behavior after:** The model-facing tool name is `spawn_subagent`; no tool named `task` exists. Restored sessions containing historical `task` tool_use blocks still load — `load_history` renders only `Text` blocks and the router resolves names only for live dispatch, so an absent name causes no error. The in-memory `always_allowed_tools` list is session-scoped, so nothing needs migrating.

**Pointers:** `crates/tact/src/tool/subagent.rs`, `crates/tact/src/tool/registry.rs`, `crates/tact/src/permission/mod.rs`

---

## 1. 2026-07-26 — `TasksChanged` no longer appends a Log card

| Field | Value |
|-------|-------|
| **Type** | removal |
| **Related** | Ch 19, Ch 23 |

**Symptom / motivation:** `on_tasks_changed` used to append a `📋 # Task.N · …` system message, duplicating the `task_*` tool row that already renders the same title. Commit `4116c23` commented the emission out as collateral damage rather than removing it, leaving `format_tasks_log_card` behind `#[allow(dead_code)]` and `tasks_changed_shows_panel_and_appends_log` red.

**Decision:** Keep the tool row as the only Log representation. Delete `format_tasks_log_card`, `focus_changed_task`, and `primary_action_for_change`; rewrite the test to assert the sticky updates while the Log length stays unchanged. `AgentUpdate::TasksChanged` keeps its `reason` field — producers and the protocol are unchanged.

**Behavior after:** A `task_create` / `task_update` call produces one Log row (the tool card) plus a sticky refresh, never two.

**Pointers:** `crates/tui/src/widgets/state/app/agent.rs`, `crates/tui/src/widgets/state/task_panel.rs`

---

## 1. 2026-07-26 — Sticky host separates tabs from body

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | Ch 23 |

**Symptom / motivation:** `sticky_host_content_height` reserved `1 + body` rows and the renderer drew the body at `inner.y + 1`, so the tab row (`[Tasks] [Subagent] …`) sat flush against `── Pending ──` / the subagent log, with the Log box border immediately above. Everything read as one crowded block.

**Decision:** Reserve one extra row (`2 + body` for Tasks, `3 + header + lines` for Subagent) and draw a muted full-width `─` rule between the tab row and the body.

**Behavior after:** The expanded sticky shows tabs, a hairline, then content. Collapsed height is unchanged at one row.

**Pointers:** `crates/tui/src/render/task_panel.rs`

---

## 1. 2026-07-26 — Bash non-zero exit is Failed

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | Ch 7 |

**Symptom / motivation:** `bash` collected `ExitStatus` but ignored it, so `cargo test` failures and other non-zero exits still rendered as `Success · …` while stdout/stderr showed the error.

**Decision:** After the process exits cleanly (no timeout/cancel/pipe failure), `!status.success()` returns `Err` via `error_with_partial` (`exit code N` or `terminated by signal`), mapping to `StepStatus::Failed` with captured output retained for the model.

**Behavior after:** Non-zero shell exits show Failed in the TUI; zero exits unchanged.

**Pointers:** `crates/tact/src/tool/bash.rs`

---

## 1. 2026-07-25 — Subagent sticky tab (clean main Log)

| Field | Value |
|-------|-------|
| **Type** | optimization |
| **Related** | Ch 12, Ch 23; `docs/superpowers/specs/2026-07-25-subagent-sticky-pane-design.md` |

**Symptom / motivation:** Subagent shared the parent `ui_tx`, so Stream/Step/Thinking mixed into the main Log and child `TokenUsage` overwrote the bottom bar.

**Decision:** Tag subagent updates as `AgentUpdate::Subagent`; sticky host tabs Tasks | Subagent; main Log keeps only the parent `task` tool row; `RequestSelect*` passthrough; first-run auto-tab, later badge.

**Behavior after:** Nested work is visible under Subagent; main Log and ctx meter stay parent-scoped during `task`.

**Pointers:** `crates/tact/src/tool/subagent_ui.rs`, `crates/tui/src/widgets/state/subagent_pane.rs`, `crates/tui/src/render/task_panel.rs`

---

## 1. 2026-07-25 — Subagent sessions linked via `ref_id`

| Field | Value |
|-------|-------|
| **Type** | optimization |
| **Related** | Ch 1, Ch 12; `docs/superpowers/specs/2026-07-25-subagent-session-ref-design.md` |

**Symptom / motivation:** `task` subagents had no `session_id` / store — turns, token usage, and DeepSeek `user_id` isolation were missing; crashes mid-`task` lost all subagent history.

**Decision:** Each subagent gets a new session row with `sessions.ref_id` = parent id (`''` if parent has none). `list_sessions` returns only top-level (`ref_id = ''`). `delete_session` cascades children. No `SessionLock` on children.

**Behavior after:** Subagent messages / `token_usages` persist under the child id; `--list-sessions` stays parent-only; deleting a parent removes its children.

**Pointers:** `crates/tact/src/tool/subagent.rs`, `crates/tact/src/store/session_store/sqlite.rs`, `ToolContext.session_id` / `session_store`

---

## 1. 2026-07-25 — Ctx meter visible at low usage

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | Ch 23; `docs/token_usage_schema.md` |

**Symptom / motivation:** With a 1M context window, ~1% usage (`13.7K/1M`) painted `▏` (1/8 block). Next to `·` that hairline read as an empty bar, so the numeric `1%` looked wrong.

**Decision:** Any positive fractional cell clamps to at least `▍` (3/8); never fall back to `·` for `frac > 0`.

**Behavior after:** Non-zero ctx usage always shows a clearly filled partial in `[…]` (e.g. 1% → `[▍·······]`).

**Pointers:** `crates/tui/src/render/bar.rs` (`partial_block_char` / `render_usage_bar`)

---

## 1. 2026-07-25 — Task tool titles, short Log cards, sticky tree, `/tasks-dag`

| Field | Value |
|-------|-------|
| **Type** | optimization |
| **Related** | Ch 11, 19, 23, 25; `docs/superpowers/specs/2026-07-25-task-tool-ui-redesign.md` |

**Symptom / motivation:** `task_*` tools dumped raw JSON; Log cards repeated full checklists; dependency graph was hard to see in-terminal.

**Decision:** Human tool titles (`# Task.N · …`); sticky defaults expanded as a `blocks` tree with `#id`; `/tasks-dag` opens a Mermaid DAG popup (nodes: status + id only; rendered via ratatui-markdown since 2026-08-06). `TaskSnapshot` carries `blocks`/`blocked_by`. Log does **not** append task system cards (progress lives in sticky + tool rows).

**Behavior after:** Readable tool rows; sticky tree; slash DAG viewer; no task system spam in Log.

**Pointers:** `crates/tact/src/task/display.rs`, `crates/tui/src/widgets/state/task_panel.rs`, `crates/tui/src/widgets/state/task_dag.rs`

---

## 1. 2026-07-25 — Task checklist renders fully (no `… +N`)

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | Ch 19, Ch 23 |

**Symptom / motivation:** Log detail cards and sticky expand capped the checklist at 6 rows (`… +N`), so an 8-task board looked incomplete even when all items were updated.

**Decision:** Drop `STICKY_BODY_CAP`; sticky height and Log cards list every task.

**Behavior after:** Full checklist in both sticky expand and each `TasksChanged` Log card.

**Pointers:** `crates/tui/src/widgets/state/task_panel.rs`

---

## 1. 2026-07-25 — Serialize persistent `task_*` tools in one turn

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | Ch 11, Ch 19 |

**Symptom / motivation:** Models often emit many `task_update` / `task_create` calls in one turn. When those ran in the same wave, TaskManager updates and `TasksChanged` UI events interleaved, producing a jammed Log and a single incomplete progress card.

**Decision:** Classify `task_create` / `task_update` / `task_get` / `task_list` as writers of a synthetic `__tact_tasks__` resource so they always land in separate waves (order preserved) while still overlapping unrelated `read_file` calls.

**Behavior after:** Within one assistant tool batch, task tools run one-at-a-time; each mutating call can emit its own `TasksChanged` in order.

**Pointers:** `crates/tact/src/agent/tool_schedule.rs`

---

## 1. 2026-07-24 — Persistent task progress sticky + Log card

| Field | Value |
|-------|-------|
| **Type** | optimization |
| **Related** | Ch 19, Ch 23, Ch 25; `docs/superpowers/specs/2026-07-24-task-progress-panel-design.md` |

**Symptom / motivation:** Persistent tasks (`task_create` / `task_update`) only appeared as ordinary tool JSON/text in the Log. There was no always-visible checklist and no structured timeline card for mutations.

**Decision:** Emit `AgentUpdate::TasksChanged` after successful mutating tools. TUI keeps a sticky strip under the Log via an **outer layout split** (Log internals unchanged), collapsed by default with click-to-expand, and appends a Log detail card on each change. Hide the sticky when no pending/in_progress items remain; do not show on resume until the first `TasksChanged` this session.

**Behavior after:**

- Sticky one-liner: `▸ Tasks done/total · focus` (click expands full checklist)
- Each `TasksChanged` adds a system Log checklist card
- `task_get` / `task_list` do not emit

**Pointers:** `crates/protocol/src/agent.rs`, `crates/tact/src/tool/task.rs`, `crates/tui/src/render/task_panel.rs`, `crates/tui/src/render/layout.rs`

---

## 1. 2026-07-24 — Remove redundant `[Log]` from bottom bar

| Field | Value |
|-------|-------|
| **Type** | removal |
| **Related** | Ch 23 |

**Symptom / motivation:** Bottom bar Row 1 always started with `[Log]` even
though the UI is permanently single-column log-only, so the focus label added
noise without information.

**Decision:** Drop the focus segment from `render_bottom_bar` Row 1. Top status
bar may still mention Log where useful; bottom bar starts with cwd / uptime.

**Behavior after:** Row 1 no longer shows `[Log]`; first segment is workspace
path (then uptime, branch, optional account).

| Pointer | Path |
|---------|------|
| Code | `crates/tui/src/render/bar.rs` |

---
## 2. 2026-07-24 — Slash popup Esc hint + priority over overlay

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | Ch 23 |

**Symptom / motivation:** Opening `/` while the agent was busy felt "stuck":
no Esc-to-close hint on the title, and Esc could be swallowed by a thinking/diff
overlay instead of dismissing the slash list.

**Decision:** Append shared `popup_close_hint` (`[Esc] Close`) to the slash
popup title (including empty state). Route Insert+slash keys before
`handle_overlay_key` so Esc always closes the slash popup first.

**Behavior after:** Slash popup title shows Esc close; Esc dismisses slash
without clearing typed input; overlay Esc only after slash is closed.

| Pointer | Path |
|---------|------|
| Code | `crates/tui/src/render/popups/slash_command.rs`, `crates/tui/src/lib.rs` |

---
## 3. 2026-07-24 — Idle bottom-bar `Up` ticks without CPU spin

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | Ch 23 |

**Symptom / motivation:** Fully idle TUI never dirtied on poll timeout, so
`Up MM:SS` froze until the next key/mouse/agent event.

**Decision:** On idle poll (~1000 ms), dirty only when the displayed uptime
whole-second changes. Active statuses still dirty for spinners; poll intervals
unchanged. Done keeps force-repaint via `should_repaint`.

**Behavior after:** Idle `Up` advances about once per second; no faster idle
redraw loop.

| Pointer | Path |
|---------|------|
| Code | `crates/tui/src/lib.rs` (`on_poll_timeout`) |

---
## 4. 2026-07-24 — Prompt elapsed moves to task-end separator

| Field | Value |
|-------|-------|
| **Type** | optimization |
| **Related** | Ch 23 |

**Symptom / motivation:** Bottom-bar `Elapsed` competed with cwd/branch/balance
and was easy to miss relative to the response it measured.

**Decision:** Freeze prompt duration into the task-end sentinel
(`\x07tact-task-end\x1f{secs}`) and render it centered on the accent rule
(`──── Elapsed 00:03 ────`). Remove elapsed from the bottom bar.

**Behavior after:** Each completed/cancelled task shows its duration on the
trailing separator; bottom row 1 no longer shows `Elapsed`.

| Pointer | Path |
|---------|------|
| Code | `crates/tui/src/render/cells/separator.rs`, `widgets/state/app/popups.rs`, `render/bar.rs` |

---

## 5. 2026-07-24 — Bottom bar readability restore

| Field | Value |
|-------|-------|
| **Type** | optimization |
| **Related** | Ch 23, `docs/token_usage_schema.md` |

**Symptom / motivation:** After the icon-only polish, the bottom bar was hard to
decode (`8K/32K`, bare `∑` / `▣`, faint ` · ` separators). Thinking effort level
was not shown even though `model_reasoning_effort` was already available.

**Decision:** Short i18n labels beside icons; thinking shows effort+budget
(`high(32K)`); row 1 uses ` │ `, row 2 uses two spaces; cache as `缓存%` /
`cache%`; last-call total as `∑ₜₒₖ`; ctx meter fill uses mid-height `■` / `·`
inside `[]`.

**Behavior after:** Readable two-row bar without a legend; same underlying
token/cache numbers. Narrow drop order: cache → uptime → path → ∑ → ctx.

| Pointer | Path |
|---------|------|
| Spec | `docs/superpowers/specs/2026-07-24-bottom-bar-readability-design.md` |
| Plan | `docs/superpowers/plans/2026-07-24-bottom-bar-readability.md` |
| Code | `crates/tui/src/render/bar.rs`, `crates/tui/src/i18n.rs` |

---

## 6. 2026-07-24 — Slash popup: Tab completes, Enter runs skills

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Related** | Ch 2, Ch 23 |

**Symptom / motivation:** After restoring Insert-mode `Tab` for the slash
popup, **Tab** and **Enter** still did the same thing for skills (both only
filled `/name `). Users could not tell complete vs run apart.

**Decision:** Slash popup **Tab** always autocompletes to `/name `. **Enter**
invokes skills and runs built-ins immediately. `/plugin` (needs a subcommand)
still only autocompletes. Command palette Enter on a skill still prefills
Insert (undo-friendly).

**Behavior after:** Pick a skill in `/` → Tab to edit args, or Enter to run
now.

**Pointers:** `crates/tui/src/handlers/insert.rs`, Ch 2 §7, Ch 23 slash skills.

---

## 7. 2026-07-24 — TUI left Execution Plan panel removed

| Field | Value |
|-------|-------|
| **Type** | removal |
| **Related** | Ch 23, Ch 25 |

**Symptom / motivation:** The left plan panel duplicated information already
visible in the log (tool blocks appear on `StepStarted`), while adding
`Tab` focus switching, an `e` visibility toggle, a draggable divider, and a
`panel_split_ratio` layout knob that most users never touched. The extra
panel-focus state also complicated mouse hit testing and keyboard handling.

**Decision:** Remove the panel UI entirely; keep `PlanStep` tracking as an
internal, panel-less store (`app.plan.steps` / `steps_set`) so step data
stays available for future consumers. The log is now permanently
single-column. `FocusedPanel` keeps only its `Log` variant. Delete `Tab`
focus switching, the `e` toggle, and divider drag/resize; `j`/`k`/`g`/`G`/`y`/`Y`/`V`
now always act on the log. `Insert`-mode `Tab` for slash-command
autocompletion (previously shadowed by the global `Tab` handler) now fires
correctly since nothing above it in `lib.rs` intercepts `Tab` first.

**Behavior after:** `render_main_area` always renders the log panel at full
width; there is no plan panel, divider, or panel-focus indicator in the top
or bottom bar. `StepAdded` still updates `app.plan.steps` for internal
bookkeeping but never draws a dedicated panel.

**Pointers:** `crates/tui/src/widgets/state/plan_panel.rs`,
`crates/tui/src/render/layout.rs`, `crates/tui/src/widgets/state/mod.rs`
(`FocusedPanel`), `crates/tui/src/handlers/normal.rs`,
`crates/tui/src/handlers/mouse.rs`, `book/23_chapter_tui*.md`.

---

## 8. 2026-07-24 — Project config file renamed `tact.toml` → `config.toml`

| Field | Value |
|-------|-------|
| **Type** | docs |
| **Related** | Ch 21 |

**Symptom / motivation:** Auto-discovery listed `./tact.toml` while user-global /
`.tact/` paths already used `config.toml`, which was easy to misplace.

**Decision:** Search `./config.toml` instead of `./tact.toml`. Rename
`tact.example.toml` → `config.example.toml`.

**Behavior after:** Discovery order is `./.tact/config.toml`, `./config.toml`,
`~/.tact/config.toml`. Explicit `--config` unchanged.

**Pointers:** `crates/tact/src/config/load.rs`, `book/21_chapter_config*.md`,
`config.example.toml`.

---

## 9. 2026-07-24 — Session Stats GFM cells padded for plain-text alignment

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Spec** | `docs/superpowers/specs/2026-07-24-session-stats-table-design.md` |

**Symptom / motivation:** End-of-session `eprintln` of `SessionStats::summary()`
printed unpadded GFM (`| Elapsed | 1.2s |` next to longer metric names), so
pipe columns did not line up in the terminal after `tact-ui` exited.

**Decision:** Keep GFM pipe tables for tui-markdown. Pad header / body cells to
the per-column max width (right-align numeric columns from `:` separators).

**Behavior after:** CLI / headless / TUI exit summaries show aligned columns in
monospace; `/stats` popup still renders via tui-markdown box tables.

**Pointers:** `crates/tact/src/stats.rs`, `docs/token_usage_schema.md`
(Session Stats Display).

---

## 10. 2026-07-24 — Extra `skill_dirs` + project-local `.tact/skills`

| Field | Value |
|-------|-------|
| **Type** | optimization |
| **Spec** | `docs/superpowers/specs/2026-07-24-extra-skill-dirs-design.md` |

**Symptom / motivation:** Only fixed skill roots existed; teams could not point at
shared or vendor skill trees. The old `<workdir>/skills/` root also sat outside
`.tact/`.

**Decision:** Replace `<workdir>/skills/` with `<workdir>/.tact/skills/`. Add
optional `[agent].skill_dirs = [...]` (relative to workdir; `~` expands). Load
order: `.tact/skills` → `~/.tact/skills` → `~/.agents/skills` → `.claude/skills`
→ config extras → plugin cache. Missing dirs soft-skipped.

**Behavior after:** Config can append skill roots that override earlier
same-named standalone skills. Bare `<workdir>/skills/` is no longer scanned.

**Pointers:** `crates/tact/src/consts.rs`, `crates/tact/src/skill/mod.rs`,
`crates/tact/src/config/types.rs`, `config.example.toml`, Ch 2.

---

## 11. 2026-07-24 — `/skills` list via tui-markdown (no pipe table)

| Field | Value |
|-------|-------|
| **Type** | bugfix |

**Symptom / motivation:** `/skills` built a Skill/Description pipe table through
`format_table`. Long frontmatter descriptions made each row wider than the log
panel; visual wrap shattered `|` columns into unreadable fragments.

**Decision:** Keep the titled block + blank separators. Emit wrap-friendly
markdown (`**\`name\`**` then description paragraph) and render with
`render_markdown_tui` / tui-markdown. Do **not** use a GFM table here (unlike
Session Stats): catalog descriptions are too wide for fixed columns in the log.

**Behavior after:** `/skills` shows one skill name + description block per entry;
text wraps cleanly at any panel width. Namespace names (`plugin:skill`) unchanged.

**Pointers:** `crates/tui/src/handlers/mod.rs` (`show_skills_command`,
`skills_list_markdown`).

---

## 12. 2026-07-24 — Session Stats as GFM tables via tui-markdown

| Field | Value |
|-------|-------|
| **Type** | bugfix |
| **Spec** | `docs/superpowers/specs/2026-07-24-session-stats-table-design.md` |

**Symptom / motivation:** `/stats` fed comfy-table UTF8 box output through
`render_markdown_tui`. Soft breaks became spaces, so the whole table collapsed
into one wrapped line and looked unreadable in the popup.

**Decision:** Keep `SessionStats::summary() -> String`. Emit **GFM pipe tables**
(with right-aligned numeric columns). TUI keeps using `render_markdown_tui` /
[tui-markdown](https://github.com/joshka/tui-markdown) table rendering (Unicode
box borders). Drop the `comfy-table` dependency. CLI / headless print the same
markdown source.

**Behavior after:** Session Statistics popup shows aligned box tables; exit
summaries are GFM markdown. Counters and visibility rules unchanged.

**Pointers:** `crates/tact/src/stats.rs`,
`crates/tui/src/widgets/state/app/agent.rs`, `docs/token_usage_schema.md`.

---

## 13. 2026-07-24 — Session Stats rendered with comfy-table

| Field | Value |
|-------|-------|
| **Type** | optimization |
| **Spec** | `docs/superpowers/specs/2026-07-24-session-stats-table-design.md` |
| **Plan** | `docs/superpowers/plans/2026-07-24-session-stats-table.md` |
| **Superseded by** | §7 (GFM + tui-markdown) |

**Symptom / motivation:** End-of-session Tool calls rows used ad-hoc space
padding, so columns drifted as names and timings grew.

**Decision:** Keep `SessionStats::summary() -> String`. Render a head
Metric/Value table, an optional Tool calls table
(`Tool | Count(s/f) | Total | Avg`), then a trailing Metric/Value table for
tool aggregates / cache / reasoning. *(Originally used `comfy-table` UTF8
boxes; that path conflicted with TUI markdown — see §7.)*

**Behavior after:** Same counters and visibility rules; layout is aligned
tables instead of free-form lines.

**Pointers:** `crates/tact/src/stats.rs`, `docs/token_usage_schema.md`
(Session Stats Display).

---

## 14. 2026-07-24 — `/model` supplements config from `/v1/models`

| Field | Value |
|-------|-------|
| **Type** | optimization |
| **Spec** | `docs/superpowers/specs/2026-07-24-openai-models-api-design.md` |
| **Plan** | `docs/superpowers/plans/2026-07-24-openai-models-api.md` |

**Symptom / motivation:** `/model` required a hand-maintained `models = [...]`
list; providers already expose `GET /v1/models`.

**Decision:** Config remains primary; API appends missing ids; conflicts keep
config; fetch once per `(base_url, api_key)` on first `/model`; Anthropic skipped;
failures soft-fail to config-only / empty hint.

**Behavior after:** See Ch 21 `/model` section.

**Pointers:** `crates/tact_llm/src/models.rs`, `crates/tui/src/handlers/select.rs`,
Ch 21, Ch 22 (account-style queries).

---

## 15. 2026-07-24 — `read_file` pagination and `batch_read` removal

| Field | Value |
|-------|-------|
| **Type** | optimization + removal |
| **PR** | [#50](https://github.com/rust-infra/tact/pull/50) |
| **Spec** | `docs/superpowers/specs/2026-07-24-read-file-pagination-design.md` |
| **Plan** | `docs/superpowers/plans/2026-07-24-read-file-pagination.md` |

### 6.1 Symptom

`read_file` loaded the whole file with `read_to_string`, then silently discarded the tail with `chars().take(50000)`. That conflicted with line-based `offset` / `limit`, gave the model no recovery signal (hallucination risk — see [Ch 20](./20_chapter_hallucination.md)), and competed with dispatch-level `persist_large_output` (30k characters → `<persisted-output>`).

`batch_read` was a second multi-file API with its own 200k-character hard cap, duplicating schedule / recent-file special cases.

### 6.2 Decision

1. Delete `batch_read`. Parallel multi-file reads use concurrent `read_file` waves.  
2. Stream lines with Tokio `BufReader` (no whole-file buffer for the page).  
3. Bound pages with prefixed constants in `read_file.rs`:

```rust
const READ_FILE_MAX_OUTPUT_TOKENS: usize = 25_000;
const READ_FILE_DEFAULT_MAX_LINES: usize = 2_000;
```

Token estimate: existing `approx_token_count` (`ceil(UTF-8 bytes / 4)`).  
4. No per-line character limit (a single oversized line errors; never silent mid-line cut).  
5. Incomplete **implicit** / default pages return a leading marker:

```text
[PARTIAL view — lines {start}-{end}; continue with offset={next}]

{joined lines}
```

6. **Explicit** `offset` and/or `limit` that still exceed the token budget → **error** (do not silently return less than requested).  
7. `run_native_tool` **skips** `persist_large_output` when `name == "read_file"`.  
8. Tool `description` stays short — limits are enforced at runtime, not duplicated in the schema blurb.

### 6.3 Behavior after

| Case | Result |
|------|--------|
| Small file, no args | Full content, no PARTIAL |
| File longer than 2000 lines, no args | First 2000 lines + PARTIAL with `offset=2001` |
| Token budget hit on implicit read | Complete lines that fit + PARTIAL with next `offset` |
| Explicit range over token budget | `Err` asking to reduce `limit` / shrink the section |
| Single line alone over budget | `Err` (cannot recover via line offset) |
| Offset past EOF | Empty string |
| Large `read_file` vs bash / MCP | `read_file` never gets `<persisted-output>`; others still may |

### 6.4 Pointers

| Area | Path |
|------|------|
| Implementation | `crates/tact/src/tool/read_file.rs` |
| Persist exemption | `crates/tact/src/agent/tool_dispatch.rs` (`run_native_tool`) |
| Tool registration | `crates/tact/src/tool/registry.rs` (no `BatchReadTool`) |
| Approx tokens | `crates/tact/src/utils/truncate.rs` |
| Tool chapter | [Ch 7](./07_chapter_tool.md) |
| Compaction / spill | [Ch 5](./05_chapter_compact.md), `docs/compaction.md` |

---

## 16. 2026-07-24 — Bottom bar visual polish

| Field | Value |
|-------|-------|
| **Type** | optimization |

**Symptom / motivation:** The bottom bar mixed emoji, long bilingual labels (`Elapsed:`, `Balance:`, `cache hit:`), and mixed separators (`│` / `|`). Both rows used a single `Paragraph` style, giving flat color hierarchy that was hard to scan.

**Decision:** Replace emoji with narrow Unicode icons (`◷`, `⊙`, `⎇`, `¤`, `∑`, `▣`). Unify separators to ` · `. Compact model limits to `8k/32k` format and collapse verbose balance/quota strings. Render with ratatui `Line` / `Span` segments: dim icons & separators, bright primary values, accent branch, success/error balance.

**Behavior after:** Two-row bottom bar with consistent iconography and color hierarchy. Pure formatting helpers (`format_model_compact`, `format_balance_entry`, `format_quota_window`, `format_cache_pct`) are unit-testable without a terminal. Narrow-width drop order removes uptime → path on row 1, cache → tokens → meter on row 2.

| Area | Path |
|------|------|
| Spec | `docs/superpowers/specs/2026-07-24-bottom-bar-polish-design.md` |
| Plan | `docs/superpowers/plans/2026-07-24-bottom-bar-polish.md` |
| Implementation | `crates/tui/src/render/bar.rs`, `crates/tui/src/i18n.rs` |
| Docs | `docs/tui_rendering.md` (Bottom Bar section) |
| Rendering framework | [Ch 23](./23_chapter_tui.md) |

---

## Related Docs

- [Tool System](./07_chapter_tool.md)
- [Context Compaction](./05_chapter_compact.md)
- [Hallucination in Agent Loops](./20_chapter_hallucination.md)
- [AGENTS.md](../AGENTS.md) — documentation sync triggers including this chapter
