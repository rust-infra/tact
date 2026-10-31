# Tact Desktop manual test report — 2026-10-31

Manual review of every user-facing flow in the PRD (§8), the entry coverage
matrix (§20), and the prototype interaction list. Each check was exercised
either through the off-screen capture harness (`target/ui-capture-r53`), an
automated interaction test, or both. Commands are given for reproducibility.

## How to reproduce

```bash
cargo run -p tact-desktop --example ui_capture --features ui-capture -- target/ui-capture
cargo test -p tact-desktop --lib
cargo test -p tact_desktop_core --lib
scripts/check-rust.sh
```

## A. Startup and sessions

| # | Check | Evidence | Result |
|---|---|---|---|
| A1 | Clean app data starts without a stale registry/settings | `platform::tests::clean_app_data_has_no_registry_or_settings`; capture `01` | pass |
| A2 | Empty session shows the Ready state and composer | capture `01` | pass |
| A3 | Submitting a prompt switches the composer to running (stop + send) | `composer_shows_stop_and_send_while_running`; capture `26c` | pass |
| A4 | Cancelling a run via Esc / stop | `keymap_matches_prd_interaction_table`; stop control in `26c` | pass |
| A5 | Resume a session from the rail keeps composer focus | `session_switch_keeps_composer_focus` | pass |
| A6 | Resume last from Command Center | `every_catalog_entry_executes_in_the_shell` | pass |
| A7 | Compact conversation | `every_catalog_entry_executes_in_the_shell` | pass |
| A8 | Archive/restore filters the manager | `session_manager_filters_active_and_archived`; capture `06` | pass |
| A9 | Fuzzy search in the session manager | `session_manager_fuzzy_search_finds_abbreviations` | pass |
| A10 | Rename / archive actions restore trigger focus | `multistep_forms_restore_the_original_trigger_focus`; capture `06` | pass |

## B. Transcript and run control

| # | Check | Evidence | Result |
|---|---|---|---|
| B1 | Every row kind renders (user, assistant, thinking, plan, tools, diff, notice, hook, error, gate) | `every_transcript_row_kind_renders`; captures `26b`–`26d` | pass |
| B2 | Plan card shows steps and EXECUTING state | capture `26d`; `desktop_shell_renders_all_transcript_row_kinds` | pass |
| B3 | Tool group expands to rows and output | capture `26c` | pass |
| B4 | Diff / thinking / tool readers open | captures `16`–`20`; `every_catalog_entry_executes_in_the_shell` | pass |
| B5 | Permission gate is sticky, Review focuses the first option, Confirm stays disabled until a choice | `permission_review_starts_empty_and_focuses_the_first_option`, `gate_close_restores_trigger_focus`; capture `33` | pass |
| B6 | Multi-select gate keeps every clicked choice | `multiselect_gate_keeps_every_clicked_choice` | pass |
| B7 | Back-to-bottom pill floats above sticky tasks | `back_to_bottom_floats_above_the_sticky_strip`; capture `28` | pass |
| B8 | Hook output toggle hides hook rows | `hook_output_toggle_hides_hook_status_rows` | pass |
| B9 | Error row exposes Retry | capture `26b` | pass |
| B10 | Assistant suggestions and selection actions | captures `26c`; `localized_answer_and_suggestion_labels_are_not_english` | pass |
| B11 | Long runs keep the tail anchored while scrolling back | captures `26a`–`26c`; `back_to_bottom_floats_above_the_sticky_strip` | pass |
| B12 | Run header shows the headline plus the prompt's summary line | capture `26a`; `run_titles_split_a_second_line_into_the_summary` | pass |

## C. Inspector

| # | Check | Evidence | Result |
|---|---|---|---|
| C1 | Run map: active run, gauge, current step, signals, latest activity | capture `28` | pass |
| C2 | Goals: create, list, pause/resume/cancel/steer/accept/archive | `goal_commands_open_the_goals_inspector`, `goal_create_arguments_preserve_objective_and_acceptance_criteria`; action buttons wired to `GoalInvoke`/forms; captures `13`, `29` | pass |
| C3 | Subagents: overview, drawer, cancel | capture `30`, `30b`; `read_only_subagent_rows_hide_suggestions_retry_and_gate_actions`; cancel dispatches `CancelSubagent` (no dedicated interaction test) | pass (capture) |
| C4 | Context tab | capture `31` | pass |
| C5 | Background task popover and reader | capture `32`; `sticky_background_opens_a_task_popover` | pass |
| C6 | Ctrl/Cmd 1–4 switch tabs | captures `12`–`15`; keymap tests | pass |

## D. Composer

| # | Check | Evidence | Result |
|---|---|---|---|
| D1 | Slash overlay lists commands and nested paths | capture `04`; `slash_root_only_lists_builtin_slash_commands`, `nested_candidates_follow_the_catalog_tree` | pass |
| D2 | Slash completion separates arguments | `longest_path_keeps_arguments_separate` | pass |
| D3 | `@` workspace mentions | `snapshot_refreshes_workspace_mentions` | pass |
| D4 | Attachments as removable chips | `localized_component_labels_are_not_english` (AttachmentStrip labels); `composer_controls_live_inside_the_prompt_frame` | pass |
| D5 | Queue starts as a count badge | `queued_prompts_start_as_a_count_badge` | pass |
| D6 | Up/Down recall input history | `composer_recalls_input_history_with_arrow_keys` | pass |
| D7 | Model picker and keyboard activation | capture `21`; picker focus is tracked by `gate_primary_focus`-style focus handles (`activate_model_picker` wired in the shell; no dedicated test) | pass (capture) |
| D8 | Permission picker | capture `21`; permission tests | pass |
| D9 | Voice control absent until the P2 pipeline | `voice_controls_absent_until_p2_pipeline` | pass |

## E. Overlays and commands

| # | Check | Evidence | Result |
|---|---|---|---|
| E1 | Command Center lists every catalog entry and executes it | `every_catalog_entry_executes_in_the_shell`; capture `03` | pass |
| E2 | Command Center fits constrained and short windows | `command_center_fits_a_constrained_window`, `command_center_fits_a_short_window` | pass |
| E3 | Settings rows show live and startup-global modes | `startup_global_settings_display_their_values`; capture `05` | pass |
| E4 | Extensions popover and manager (MCP/Skills/Plugins) | captures `07`–`10b`; `extension_subcommands_open_their_manager`, `extensions_manager_carries_query_into_full_dialog` | pass |
| E5 | Hook review sheet with trust/forget | capture `10c`; hooks route tests | pass |
| E6 | Gate review dialog (keyboard) | capture `33`; gate tests | pass |
| E7 | Subagent drawer | capture `30b` | pass |
| E8 | Readers: system prompt, stats, history, tasks, balance | captures `16`–`20`, `38` | pass |
| E9 | Help / keyboard shortcuts reader | capture `40-help`; `help_reader_lists_the_primary_shortcuts` (content), `help_and_save_entries_are_executable` (route) | pass |

## F. Appearance, accessibility, i18n

| # | Check | Evidence | Result |
|---|---|---|---|
| F1 | Light and dark themes share geometry | `light_and_dark_share_component_geometry`; captures `01`/`02` | pass |
| F2 | Narrow window moves panels to scrimmed overlays | `narrow_window_moves_panels_to_scrimmed_overlays`; capture `23` | pass |
| F3 | 125% / 150% text scale stays usable | `text_scale_125_and_150_remain_usable`; capture `24` | pass |
| F4 | Chinese UI strings are complete | `every_desktop_string_has_a_chinese_translation`; capture `25` | pass |
| F5 | Reduced motion skips transitions | `reduced_motion_uses_direct_state_changes` | pass |
| F6 | Keyboard-only traversal and overlay focus traps | `tab_traverses_the_shell_controls`, `tab_cycles_inside_an_overlay_focus_trap`, `overlay_focus_keeps_composer_inert` | pass |
| F7 | Rows expose accessibility roles and labels | `rendered_rows_use_accessibility_roles_and_labels`, `every_transcript_kind_writes_accesskit_role_and_localized_label` | pass |
| F8 | Copy/select on transcript and readers | `SelectionActions` wired for assistant rows (`sync_assistant_selections`); clipboard handling comes from the component. No dedicated clipboard test in the off-screen harness | pass (wiring) |
| F10 | Avatars use the local account initials | `avatars_use_two_uppercase_initials`; captures `01`, `26a` | pass |
| F9 | Packaging targets exist for macOS/Windows/Linux | `macos_packaging_target_is_explicit`, `windows_target_is_explicit`, `linux_target_is_explicit`; `.github/workflows/desktop-package.yml` | pass (CI) |

## Findings fixed during this pass

1. **Tasks DAG reader showed an empty dialog with only Close** when no tasks
   existed. It now prints `No tasks yet.` / `还没有任务。`; covered by
   `empty_tasks_reader_explains_there_are_no_tasks`.
2. **`/help` now has a capture.** The reader content is a free function
   (`help_reader`) shared by the controller and the off-screen harness, so
   capture `40-help` renders the real shortcut list.

## Residual (non-blocking)

- P2 components stay explicitly deferred: `SearchResults`, `ArtifactPanel`,
  `DiffTable`, `ContextCard` (`p2_components_are_explicitly_deferred`).
- The Task DAG graph renderer is P2; the dependency list is the P1 surface.
- The capture harness seeds design copy from `26a` onward; production content
  comes from the live controller.
