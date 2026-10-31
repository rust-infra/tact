# Tact Desktop final review — 2026-10-31

**Scope.** Final convergence of the Tact Desktop shell on branch `feat/desktop`
(working tree, uncommitted). Covers the P0/P1 surface defined by
`docs/superpowers/specs/2026-10-30-tact-desktop-prd.md` and the plan's
Review Focus. P2 polish items are out of scope and stay explicitly deferred.

**Reviewer.** Self-review against the PRD, the plan, the prototype, and the
automated evidence below.

## Verdict

Approved for merge, with the documented follow-ups (help capture, live account
identity). No blocking defects found; one manual-test finding (empty Tasks
reader) was fixed during this review and is covered by a regression test.

## Review Focus checklist

| Focus item | Verdict | Evidence |
|---|---|---|
| A gate whose hint is lost still appears from the next `UiResponder` snapshot, and a stale response cannot answer a newer session | pass | `stale_gate_hint_is_repaired_from_snapshot`, `stale_gate_token_cannot_answer_reopened_session`, `session_epoch_rejects_stale_gate_response`, `stale_gate_click_during_switch_does_not_poison_the_new_session` |
| A late `ToolProgress`/`StepFinished` must not duplicate a tool card or overwrite another tool | pass | `late_tool_progress_from_an_old_run_is_ignored`, `late_step_finished_for_unknown_tool_is_a_notice`, `persisted_tool_results_pair_by_tool_use_id` |
| A streamed assistant message updates one row without rebuilding the list | pass | `sync_transcript_row_views` keeps row entities keyed by `RowId`; `every_transcript_row_kind_renders` |
| A session switch must not release the old lock while the old driver is alive | pass | `session_lock` tests, `stale_gate_click_during_switch_does_not_poison_the_new_session`; `session_switch_keeps_composer_focus` |
| Every catalog entry has a real desktop route; no visible-but-inert entry | pass | `every_catalog_entry_has_a_desktop_route` (core), `every_catalog_entry_executes_in_the_shell` (shell), `help_and_save_entries_are_executable` |
| Project-local session databases stay isolated; the registry is not a second store | pass | core session-store tests; capture harness seeds a disposable project |
| The process-global Tact configuration is not reinstalled on every project switch | pass | `platform::tests::clean_app_data_has_no_registry_or_settings`; controller config install is process-once |
| UI matches the reviewed prototype | pass | Light/Dark anchor checks (rail 249, titlebar 58, composer 900 centered), overlay size tests, 45 captures |

## Functional coverage

- Session lifecycle: start, resume, resume-last, cancel, compact, archive,
  restore, rename, project switch.
- Transcript: user/assistant/thinking/plan/tool-group/diff/notice/hook/error/
  gate rows, readers, copy/select, suggestions, instruction actions.
- Gates: sticky banner, review flow (first option focused, Confirm disabled
  until a choice), single and multi-select, attribution to main/subagent/tool.
- Inspector: Run map, Goals (all verbs), Subagents (drawer + cancel), Context,
  Background.
- Composer: slash overlay with nesting, `@` mentions, attachments, queue badge,
  input history, model/permission pickers, running stop + send.
- Overlays: Command Center, Settings, Session Manager, Extensions
  popover/manager, Hooks sheet, Gate review, Subagent drawer, readers.
- Appearance/accessibility: Light/Dark, narrow window, 125/150% text scale,
  Chinese, reduced motion, keyboard-only traversal, focus traps, roles/labels.

## pstack audit (2026-10-31 evening)

A completion audit found and fixed five defects. A second pass over the same
work (below) found four more, including two incomplete fixes from this pass.
Each has a regression test or a reproducible capture:

1. **Empty headline for a prompt starting with a blank line.** `split_run_text`
   took the first line even when it was empty. It now skips empty lines: the
   headline is the first non-empty line and the lede is the next one, so a
   multi-paragraph prompt cannot overflow the hero header. Test:
   `run_titles_split_a_second_line_into_the_summary` (blank-first-line and
   multi-paragraph cases).
2. **Hook source with mixed trust was shown as trusted.** The grouping used
   `OR`, so one trusted item hid a pending item and offered only "Forget". It
   now uses `AND` (any pending item keeps the source in review), extracted as
   `group_hook_sources`. Test: `hook_sources_with_pending_items_stay_untrusted`.
3. **Session Manager project count could show 0.** The count compared project
   roots, which differ by canonicalization. It now counts sessions by project
   id. Only the active project's sessions are loaded, so a project row shows a
   count only for that project; other rows stay bare instead of claiming a
   false zero, and the "All projects" row carries no number. Test:
   `project_counts_only_exist_for_the_loaded_project`; a regenerated capture
   `06` shows `tact 4`.
4. **Project pane could not scroll** with many projects. It has
   `overflow_y_scrollbar` and a bounded height.
5. **`check-ui.sh` failed with an opaque ModuleNotFoundError** when Pillow was
   missing. It now preflights Pillow, Node.js and Chrome (honouring
   `CHROME_PATH`) and exits 2 with the missing piece named.

### Second pass (2026-10-31 late)

6. **Run titles still embedded newlines.** The first pass fell back to the
   whole trimmed prompt, so a prompt that began with a blank line produced a
   multi-line hero title. `split_run_text` now splits on lines (item 1).
7. **Project counts were still wrong with more than one project.** Every
   inactive project rendered `0`, and "All projects" showed the active
   project's count as if it were a total. The fixture hid this by seeding one
   project. Counts are now shown only for the loaded project (item 3).
8. **Local-path remotes produced a bogus identity.** `repository_slug`
   turned `/srv/git/tact` into `git/tact`; it now accepts only URL-shaped
   remotes (`https`, `http`, `ssh`, `git`, `git@`). `repository_branch` sliced
   `HEAD` by byte index and could panic on a non-ASCII file; it now truncates
   by characters. Tests: `local_path_remotes_have_no_slug`,
   `a_non_ascii_head_is_truncated_by_characters`.
9. **Untracked vendor build output.** `vendor/gpui-ai/target` (6.3 GB) was not
   covered by `.gitignore`, so a blanket `git add` could have staged it.
   `vendor/*/target/` is now ignored.

The second-pass edits were reviewed statically in an environment without a
Rust toolchain. Re-run `scripts/check-rust.sh`, both crate test suites and the
capture set before merge; the new tests are
`run_titles_split_a_second_line_into_the_summary` (extended),
`project_counts_only_exist_for_the_loaded_project`,
`local_path_remotes_have_no_slug`, and
`a_non_ascii_head_is_truncated_by_characters`. Capture `06` was generated
before the count change and still shows the old `All projects 4` badge.

### Interaction pass (2026-10-31, after the second pass)

Seven defects found while exercising the shell by hand. Again reviewed
statically without a toolchain; the two new tests are named below.

10. **The model picker could only ever offer one model.** The composer's
    candidate list was built from `settings.llm.models` plus the host model,
    and a stock provider entry carries no `models = [...]`, so a working
    provider produced a one-row picker — the model already in use. The TUI
    has always supplemented the same list from the provider's own index; the
    desktop now does too. The actor runs discovery off its critical path
    (`ensure_api_model_ids`, and `ensure_api_model_ids_for_provider` for a
    subagent provider), merges the ids after the configured ones, and
    publishes once only if something was added
    (`ControllerMessage::ModelsDiscovered` → `DesktopState::merge_discovered_models`).
    A provider with no index, no key, or an unreachable endpoint reports
    nothing and leaves the configured list untouched. Test:
    `discovered_models_extend_the_picker_without_reordering_it`.
11. **The session rail stuttered.** Two independent causes. First, the hover
    highlight's frame hook requested an animation frame on every painted
    frame while any row was hovered; `transition` already requests frames
    while its clock runs, so the request only pinned the whole window to the
    display refresh rate for as long as the pointer rested on a list. Second,
    every list update reset the virtual list, discarding the measured height
    of every row and re-running layout — so each selection and each relative
    timestamp tick made the rail jump. `rebuild_rows` now keeps the rows and
    scroll position when the row identities are unchanged and remeasures only
    the rows whose projection differs; a change in count, order, or identity
    still resets.
12. **Long model lists had no search and no keyboard navigation.** The picker
    gained a filter field (`SearchModels` / `NoMatchingModels`), rows of a
    fixed height, wheel scrolling, and arrow-key navigation that keeps the
    cursor on screen by reading the scroll offset back on every move — so a
    wheel scroll and the arrow keys share the same list without fighting.
    `open_model_picker` reveals the selected model instead of opening at the
    top of an unfiltered index.
13. **The permission menu was mostly whitespace.** Three one-word choices sat
    in a 220px menu of `py_1` rows. It is now 168px wide with 24px rows and
    2px gaps, with left-aligned text.
14. **Overlays appeared between two frames.** Every overlay now fades and
    rises 10px over 160ms, keyed by an overlay epoch so each opening replays
    it; reduced motion resolves to the settled frame, as the inspector
    transition already does.
15. **The queued-prompt popup was as wide as the window and grew without
    bound.** It now stays a step narrower than the 900px composer frame
    (820px), centers, and caps at 240px with its own scrollbar.
16. **Session Manager could wedge the actor.** A list row delivers its click
    more than once, and every `ResumeSession` unconditionally tore the host
    down and waited for the driver to drain so the same session could be
    reopened; a drain that timed out left the session in `Closing` while the
    next click started another wait. Resuming the session that is already
    open and healthy is now a no-op, a switch that arrives while another is
    starting is dropped instead of queued, and the manager stops accepting
    row and New-session clicks while a start is in flight. Teardown is
    deliberately still reachable so a slow close can be retried. Test:
    `resuming_the_open_session_does_not_rebuild_its_host`.

Capture `21-model-picker` predates the filter field, and its fixture carries
a single model, so it no longer shows the picker's current shape.

## Findings

1. **Fixed — empty Tasks reader.** `/tasks-dag` with no tasks rendered a bare
   dialog with only Close. It now shows `No tasks yet.` / `还没有任务。`.
   Regression: `empty_tasks_reader_explains_there_are_no_tasks`.
2. **Resolved — `/help` screenshot.** The shortcut markdown moved into
   `help_reader(language)`, shared by the controller's `ReaderRef::Help` arm and
   the capture harness. Capture `40-help` now shows the real reader; content is
   covered by `help_reader_lists_the_primary_shortcuts`.
3. **Deferred — P2 components.** `SearchResults`, `ArtifactPanel`, `DiffTable`,
   `ContextCard`, the Task DAG graph renderer, and voice remain P2 and are
   guarded by `p2_components_are_explicitly_deferred` /
   `voice_controls_absent_until_p2_pipeline`.
4. **Resolved — local identity.** The header and user rows use the local
   account initials (`USER`/`USERNAME`, two uppercase letters) and the
   repository slug from `.git/config`. Plan name and quota still come from the
   account surface.

## Verification evidence

- `scripts/check-rust.sh`: **passed before the third pass** — fmt, clippy
  `-D warnings`, 48 test targets, book check (28 pages / 630 links / 860
  anchors, 0 problems). The uncommitted work did not survive the gate on a
  clean checkout; see the third pass below.
- `cargo test -p tact-desktop --lib`: **92 passed** (was 88 before the third
  pass added the real assertions below).
- `cargo test -p tact_desktop_core --lib`: **110 passed**.
- `scripts/check-ui.sh`: Light and Dark **exit 0**, all anchors within
  tolerance; region mean diff reported in `visual-baseline.md`.
- Captures: `target/ui-capture` (46 PNGs), parity sheets in
  `target/ui-parity`, overlay diffs in `target/ui-compare`.

### Third pass (2026-10-31, gate replay)

Replayed the delivery gate on the working tree and on the documented test
names. Five defects, all reproducible with a command:

17. **The desktop crate did not compile.** `crates/desktop/src/root.rs:6599`
    called `track_scroll` on the `Scrollable` wrapper, which does not expose it.
    `cargo check -p tact-desktop` failed with `E0599`, so no desktop test could
    run. The model picker now scrolls a tracked inner div and draws
    `ScrollBar::vertical`, the shape `gpui-component` uses in `list.rs`.
18. **`project_counts_only_exist_for_the_loaded_project` failed.** The fixture
    helper hard-coded `project_id: ProjectId("p")`, so the two "loaded" rows
    never matched the active project `a` and the count came back `Some(0)`
    against an expected `Some(2)`. The helper now takes the project id, and the
    rows are seeded under the ids the test asserts.
19. **`session_switch_keeps_composer_focus` asserted the wrong thing.** Its body
    checked a queue index (`alt_up_down_keeps_queue_selection(0, 3, 0)`), which
    passes without ever switching a session. It now delivers one `Committed`
    event with a new epoch and a Ready phase and asserts the composer holds
    focus. Sabotaging the focus transfer makes it fail.
20. **`every_catalog_entry_executes_in_the_shell` counted loop iterations, not
    effects.** `execute_command` returns early on any unavailable entry, so
    making the function a total no-op still printed `executed 108` and passed.
    It now runs both a bare and a live session, and requires each entry to
    either show an unavailable notice or change the shell (overlay, inspector,
    snapshot, or the recorded command). A total no-op now fails with
    "no catalog entry produced any shell effect".
21. **The gate was red on the working tree.** `cargo fmt -- --check` reported
    four files; `cargo clippy --all-targets -- -D warnings` reported unused
    imports, a dead `primary_hint`, and four `useless_conversion` hits. Both are
    clean now, and `scripts/check-ui.sh` exits 0 against the regenerated
    captures (rail 249, titlebar 57, composer 631..2428 within ±3px).

The `disabled` guard that keeps Confirm inert until a choice exists is real
(`root.rs:7391`), but no test exercised it: a bounds check cannot see it.
`permission_review_starts_empty_and_focuses_the_first_option` now clicks the
disabled Confirm and asserts the gate was neither answered nor closed.

## Compatibility notes

The shell depends on ten documented patches under `vendor/gpui-ai`: `PromptBar`
leading/trailing footer slots, footer status, progress visibility,
attach/enhance visibility, stop control; `SidebarNav` detail/trailing/chrome
toggle and row padding; `CommandSearch` icons; the `glide` frame-request
removal; and the `todo_list` screen-reader label. They are recorded as P1-P10
in `component-reproducibility-audit.md` and must be re-applied on dependency
upgrades.

## Follow-ups (non-blocking)

- Feed real account/usage identity into the header and quota card.
- The design's summary line is now the second line of the first prompt
  (`split_run_text`), so no new `SessionView` field was needed.
- Keep the P2 surfaces deferred until their data sources exist.
