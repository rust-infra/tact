# `/skill` — one command per idea, and a slash surface that completes subcommands

Status: **shipped** (`89400ca3`, 2026-10-01), on top of the single `SlashCommand` enum from
`df4a974b`. Retroactive record, written 2026-10-02 from the shipped code, that commit's message, and
the Ch 26 entry.

## Problem

Three turns of one thread:

1. The slash surface could offer a command but **not its subcommands** — you had to know the syntax
   and type it out.
2. `/skills` and `/skill-reload` were the only pair that was not "a command with subcommands".
   `/mcp`, `/plugin` and `/hooks` all are.
3. **Every installed skill was a first-level command.** A marketplace with dozens of them buried
   `/mcp` and `/compact`, and each install shifted the whole list — so muscle memory broke on an
   action unrelated to the list you were reading.

## Design

### Subcommands are a declared tree

`SlashCommand::subcommands() -> &'static [Subcommand]` (`crates/tui/src/widgets/state/slash.rs`):

| field | meaning |
|---|---|
| `name` | the token |
| `hint` | the syntax shown to the right of the row (`<server>`, `--all \| --source <label>`) |
| `children` | nested subcommands |
| `takes_value` | whether more input is expected after it |

Unimplemented subcommands are **not declared at all** — which is why `/plugin install` is absent: the
TUI handler rejects it. The declaration is therefore a claim about what *runs*, and
`every_declared_subcommand_has_a_handler` holds it to that: it walks the declaration, dispatches a
sample input per leaf (a value-taking one gets `sample`), and fails if any of them reaches a usage
hint. A completable-but-unrunnable subcommand is a test failure, not a user's dead end.

### One function decides what is offered

`App::slash_candidates() -> Vec<Candidate>` replaces `matched_commands`, and decides from how far the
input has got:

| input state | offered |
|---|---|
| the name is still being typed | the built-ins |
| the name is complete and a space follows | that command's subcommands |
| the input is an argument | nothing — the popup closes rather than showing a "no match" box over an argument |

`Candidate { path, detail, is_skill, incomplete }`. `path` is the whole command path, echoed back
verbatim so completing a nested subcommand replaces the whole span rather than just its last token.

Keys:

- **Tab** completes the whole path (`/plugin ma` → `/plugin marketplace ` → `/plugin marketplace list `).
- **Enter** completes candidates that expect more input and runs finished ones.
- **Space** only closes the popup when nothing follows it.
- The palette's Enter on `/skill` opens the same popup, so the hand-off keeps completing.

### Skills become children of `/skill`

- `skill_candidates()` turns the registry into `/skill`'s dynamic children: `/skill ` lists `list`,
  `reload`, then every skill with its description. The name filters like any other token.
- `/skill <name> [args]` runs one, sharing `invoke_skill` with the `/{name}` form that stays
  supported — `$ARGUMENTS` handling, the `<skill>` wrapper and the submission have one
  implementation; only the echoed log line differs.
- A skill named like a declared subcommand is **dropped from the list** (`/skill list` cannot be two
  indistinguishable rows) and remains reachable directly.
- `/skills` and `/skill-reload` fold into `/skill list` / `/skill reload`; `needs_args()` gains
  `Skill`, so Enter completes `/skill ` instead of firing it, and a bare `/skill` or an unknown
  subcommand leaves the usage hint in the input box — exactly like `/mcp`.

### Highlighting

`split_skill_slash` highlights `/skill demo` as well as `/demo`. The kit's `SKILL_COMMAND` is
asserted against `SlashCommand::Skill` from the host, so a rename cannot silently un-highlight it.

## Non-goals

- **No new invocation semantics.** `/{name}` keeps working; `/skill <name>` is the gathered spelling
  of the same thing.
- **No declaring a subcommand without a handler.** The declaration is what the guard test reads.
- **No skills at the first level.** That is the whole point of the move.

## Verification

- `crates/tui/src/widgets/state/slash_command.rs` (candidate selection):
  `a_trailing_space_offers_the_subcommands_of_a_builtin`, `a_partial_subcommand_is_the_only_match`,
  `subcommands_nest`, `a_subcommand_row_carries_its_syntax_and_whether_more_is_expected`,
  `nothing_is_offered_once_the_input_is_an_argument`, `a_command_without_subcommands_stops_at_the_command`,
  `still_typing_the_name_matches_commands`, `the_first_level_is_builtins_only`,
  `skills_are_offered_under_the_skill_command`.
- `crates/tui/src/handlers/insert.rs` (keys):
  `typing_a_space_keeps_the_popup_while_subcommands_remain`, `tab_walks_down_nested_subcommands`,
  `enter_on_a_subcommand_that_needs_a_value_only_completes`, `enter_on_a_complete_subcommand_runs_it`,
  `slash_popup_tab_only_autocompletes_a_skill`.
- `crates/tui/src/handlers/mod.rs` (dispatch):
  `every_declared_subcommand_has_a_handler` — the guard that keeps the declaration honest —
  plus `skill_command_lists_skills_and_clears_the_input`, `skill_command_reload_reports_the_rescan`,
  `bare_skill_command_offers_the_usage_and_keeps_the_input`.
- `crates/tui/src/handlers/palette.rs`:
  `palette_enter_on_a_command_with_subcommands_opens_its_completion`,
  `palette_enter_on_a_command_without_subcommands_stays_quiet`,
  `enter_on_an_arg_taking_command_preserves_prior_input_via_undo`.
- `crates/tui/src/handlers/skills.rs`: `skill_args_from_the_skill_command_strips_the_whole_prefix`,
  `the_skill_command_runs_a_skill_with_args`, `an_unknown_skill_name_gets_the_usage`.
- Rendering: `crates/tui/src/render/popup_scene_tests.rs` —
  `full_frame_slash_popup_completes_subcommands_with_their_syntax`;
  `crates/tui/src/render/slash_style.rs` — `the_kit_gather_command_is_the_hosts_skill_command`,
  `the_skill_command_form_highlights_the_skill_name_too`;
  `crates/agent_tui_kit/src/render/slash_style.rs` — `split_skill_slash_covers_the_gathered_form`.
- Docs: [Ch 23](../../../book/23_chapter_tui_zh.md), [Ch 26](../../../book/26_chapter_issue_zh.md)
  2026-10-01 entries.
