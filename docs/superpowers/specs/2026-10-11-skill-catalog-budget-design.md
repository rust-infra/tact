# A bounded skill catalog — capped descriptions, a context budget, and a per-skill switch

Status: **implemented** (2026-10-11), uncommitted; `./scripts/check-rust.sh` green.
The measurements are from the author's machine, taken 2026-10-11.

## Problem

`# Available skills` is the only part of the system prompt that grows with how much software the
user has installed, and nothing bounds it. Measured on the author's machine — 79 skills from
`~/.agents/skills` (79) and `~/.tact/skills` (2):

| | |
|---|---|
| catalog size | **22,605 chars ≈ 5.6k tokens** |
| per request | unconditional — it is above `=== DYNAMIC_BOUNDARY ===` |
| longest single description | **966 chars** (`cut-the-curve`) |
| average description | 286 chars |

Four separate gaps, all in `crates/tact/src/skill/mod.rs`:

1. **No cap on a description.** `describe_available()` prints whatever the author wrote. The Agent
   Skills spec caps a description at 1024 chars and Pi enforces it; Tact enforces nothing.
2. **No ceiling on the list.** 79 skills is a fact about this machine, not a bound. Codex is the only
   implementation surveyed that caps the list at all (2% of the context window, or 8,000 characters
   when the window is unknown — shorten descriptions first, then omit skills with a warning).
3. **A skill cannot reference its own files.** `load_skill` returns the body wrapped in `<skill>` and
   nothing else. `SkillManifest` had a `path` field removed on 2026-10-04 for having no consumer
   ([Ch 02](../../../book/02_chapter_skill_zh.html) §3) — the consumer is exactly this: a skill that
   ships `references/formats.md` has no way to tell the model where it lives. Pi and Codex both solve
   it by putting the path in the catalog.
4. **No per-skill switch.** A skill that must never be auto-matched can only be removed by deleting
   the file. `skill_body_auto_inject` is the only knob and it points the other way.

## Design

### A catalog line is a routing hint, not documentation

A new `describe_available_bounded(budget_chars, description_cap)` renders the prompt's catalog. It
caps every description at `description_cap` (default **200**, `0` = uncapped), cutting on a word
boundary and appending `…`. The full text is never lost — it is what `load_skill` returns, what
`/skill list` renders, and what the `/skill ` completion shows.

`describe_available()` keeps its current, uncapped meaning: it is the TUI's `skills_description`
snapshot, not a prompt input. Making the bounded form a separate method keeps the two audiences from
sharing a signature that only one of them needs.

Measured against the same 79 skills:

| cap | catalog | ≈ tokens | entries trimmed |
|---|---|---|---|
| none (today) | 22,605 | 5.6k | — |
| 512 | 20,124 | 5.0k | 13 |
| 320 | 16,591 | 4.1k | 27 |
| **200 (default)** | **12,672** | **3.2k** | **37** |
| 160 | 11,078 | 2.8k | 44 |

200 is the default because it holds a "what it does and when to use it" sentence pair — the spec's own
description guidance — while halving the cost. `0` exists for a five-skill machine that wants the
whole text.

### A budget, because 79 is not a ceiling

```
budget_chars = model_context_window × 2% × 4      // four bytes per token, the house estimate
budget_chars = 8_000                              // when model_context_window == 0
```

Four bytes per token is the estimate the `mcp get` cost line already uses
([2026-10-01-mcp-tool-declaration-cost-design.md](2026-10-01-mcp-tool-declaration-cost-design.md)),
and the fallback mirrors Codex's.

Over budget, entries are dropped from the **tail of the name-sorted list** and replaced by one
trailer line naming the dropped skills, itself capped at 40 names plus `…`. The keep-loop reserves
the trailer's length, so the trailer cannot push the catalog back over. Names are kept because
without them `load_skill` is unusable for the dropped set — the trailer is the difference between
"some skills are hidden" and "some skills are unreachable".

Name order is deliberate and is the weakest part of the design. Ranking by usefulness would need the
root a skill came from, which `SkillManifest` does not carry and which this change does not add. A
deterministic, stated order beats a hidden heuristic.

**Honest scope note.** On the author's machine `model_context_window = 1000000`, so the budget is
80,000 chars and **never fires** — there the cap is the whole saving. The budget exists to protect
128k and 200k windows, where a capped catalog (12,672) still exceeds the ceiling at 128k (10,240 —
about six entries dropped) but fits at 200k (16,000).

The budget applies to `describe_available_bounded()` **only**. `describe_available_with_body()` is
what `skill_body_auto_inject` selects, and that flag is an explicit opt-in to "everything, always";
capping it would silently break the promise it makes. It is unchanged apart from dropping disabled
skills.

### The path arrives with the body, not before it

Codex and Pi both put the skill's path in the catalog because their loader *is* "read the file at that
path". Tact has a tool, so it can hand the directory over at the moment the model can act on it —
for **zero always-on cost**:

- `SkillDocument` and `SkillEntry` gain `dir`.
- The wrapper becomes `<skill name="demo" dir="/abs/path/to/demo">` in **both** producers:
  `SkillDocument::Display` (the `load_skill` result) and `format_skill_agent_task`
  (`crates/tui/src/handlers/skills.rs`, the slash path). One test pins that they agree.
- The system prompt's skill-loading policy paragraph gains one sentence: bundled files resolve
  relative to `dir`.

An interned root table over 79 skills would cost ≈2 KB on every request to deliver information the
model needs once, later. This is the one place Tact should deliberately diverge from both.

Mechanical fallout: `SkillEntry { … }` is constructed in ~25 places across `crates/tui`, tests
included. The compiler finds every one.

### A per-skill switch that hides, never disables

```toml
[agent]
skill_disable = ["cut-the-curve", "plugin:brainstorming"]
```

Keyed on the **registry name** — what the model sees, what `/skill` takes, what `load_skill` takes.
Not on the path: a plugin skill's path contains the plugin **revision**
(`~/.tact/plugins/cache/<marketplace>/<plugin>/<revision>/skills/`), so a path-keyed entry breaks on
every plugin update. Codex keys on path; that is the one part of its design that does not transfer.

A disabled skill is dropped from `describe_available()` and from `describe_available_with_body()`,
but **stays invocable**: `/skill <name>` still runs it and `load_skill` still resolves it. The switch
governs automatic discovery, not capability, so a typo cannot lock anyone out.

`SkillRegistry` therefore carries `disabled: HashSet<String>` and filters at **render** time, not at
load time — `/skill list` still shows the skill, marked, because a row that silently vanishes is
indistinguishable from a broken install. The `/skill ` completion still offers disabled skills too:
they are reachable by name and hiding them from the completion would only make typing them harder.

### `/skill disable <name>` / `/skill enable <name>` write the key

The two verbs are declared in `SKILL_SUBCOMMANDS`, so `/skill ` completes them and
`every_declared_subcommand_has_a_handler` holds them to a handler. The handler:

1. resolves the name against the TUI's list — an unknown name reports itself and stops, rather than
   falling into the usage hint (the popup already taught the syntax, and this error fires mid-command);
2. treats a repeat as a no-op with a message, so `/skill disable demo` twice does not rewrite the file;
3. flips the **shared** registry, so the next turn's `# Available skills` changes with no reload;
4. rebuilds the TUI list from that same registry, so `/skill list` marks the row in the same breath
   instead of at the next `/skill reload`;
5. writes the **whole** list to `[agent].skill_disable` through `toml_edit`, keeping the file's
   comments, and reports the outcome the way `/hook-output` does — saved / this session only / write
   failed.

Writing the whole set rather than a delta is not a style choice: the writer replaces the key, so a
partial list would silently re-enable every skill left out of it.

The verb's value is **completed, not typed**: `slash_candidates()` records when the walk lands on a
value-taking subcommand, and `skill_verb_candidates()` offers the half that verb can act on —
`disable` lists what is in the catalog, `enable` what is out of it. A row for a skill already in the
state being asked for would only ever answer "already hidden", so it is not offered. The space after
`disable` does not close the popup, because `slash_candidates()` is non-empty, and Enter on a row
**runs** it rather than completing the path — which is the point of listing them. This is the only
place in the completion tree where a *value* is completable; `/mcp auth <server>` still closes on the
separating space. An unrecognised verb returns nothing rather than guessing which half to offer.

An empty set is written as `skill_disable = []` rather than removing the key — the file is the record
of what the catalog excludes, and a key that disappears when the last skill is re-enabled makes the
mechanism look like it was never there.

Still no CLI flag, like `skill_dirs`.

## Non-goals

- **No retrieval, no `list_skills`.** Ranked injection against the user's message is the next tier and
  is not designed here. Nothing in this change indexes skills.
- **No budget knob.** The ceiling is derived from the window. The two knobs are the cap and the deny
  list; escaping the budget is not supported.
- **No usefulness ranking for omission.** Alphabetical tail, named in the trailer.
- **No per-skill description cap in frontmatter.** One global cap; a skill cannot opt out of it.
- **No `path` back in `SkillManifest`.** The directory rides the document, and never enters the
  catalog.
- **No change to `describe_available_with_body()`** beyond dropping disabled skills, and none to the
  `load_skill` error path.
- **No `skill_enable` allow list.** `/skill enable <name>` clears one deny entry; an allow list
  ("only these may be auto-discovered") would need a notion of "otherwise disabled" that the registry
  does not have.

## Verification

- `crates/tact/src/skill/mod.rs`:
  `a_long_description_is_capped_on_a_word_boundary` (no mid-word cut, `…` appended),
  `a_cap_of_zero_leaves_descriptions_whole`,
  `the_catalog_drops_the_alphabetical_tail_when_over_budget`,
  `the_trailer_names_the_dropped_skills_and_stops_after_forty`,
  `the_budget_falls_back_to_eight_thousand_chars_without_a_window`,
  `a_catalog_that_fits_is_not_touched` (the budget must not fire on a 1M window),
  `describe_available_with_body_is_not_capped`,
  `a_disabled_skill_leaves_the_catalog_but_still_loads`,
  `the_skill_block_carries_the_skill_directory`.- `crates/tui/src/handlers/skills.rs`: `format_skill_agent_task_carries_the_skill_directory`.
- `crates/tui/src/handlers/mod.rs`: `the_skill_list_marks_a_disabled_skill`,
  `skill_disable_then_enable_round_trips` (the row is marked without a reload),
  `disabling_twice_reports_the_state_instead_of_rewriting`,
  `disabling_an_unknown_skill_names_it` (it must not land on the usage hint).
- `crates/tui/src/widgets/state/slash_command.rs`:
  `the_disable_verb_offers_the_skills_that_are_in_the_catalog`,
  `the_enable_verb_offers_only_what_is_out_of_the_catalog`,
  `the_disable_verb_filters_by_the_token_being_typed`,
  `the_enable_verb_offers_nothing_when_the_catalog_is_whole`.
- `crates/tui/src/handlers/insert.rs`: `slash_popup_offers_the_skills_a_verb_can_act_on` (the popup
  survives the separating space, and Enter runs rather than completes).
- `crates/tact/src/config/persist.rs`: `writes_the_skill_disable_list_keeping_the_rest_of_the_agent_table`,
  `an_empty_disable_list_is_written_not_removed`.
- `crates/tact/src/config/resolve.rs`: `resolve_skill_description_cap_defaults_to_200`,
  `resolve_skill_disable_from_toml`.
- Docs: [Ch 02](../../../book/02_chapter_skill_zh.html) §1 (the per-turn table), §5 (`describe_available`
  signature), §7 (the `/skill` table and the new verbs), §10 (three rows close),
  [Ch 21](../../../book/21_chapter_config_zh.html) (the two keys),
  [Ch 26](../../../book/26_chapter_issue_zh.html) entry.

## Related

- [2026-10-01-mcp-tool-declaration-cost-design.md](2026-10-01-mcp-tool-declaration-cost-design.md) —
  the same problem for tool declarations, and the reason the estimate is four bytes per token. It
  chose visibility over auto-trimming; this change caps presentation and budgets the list, which is
  a different call, so the difference is worth stating: a tool schema is structure the model needs to
  *call* the tool, while a catalog description is a routing hint that `load_skill` can always
  supersede.
- [2026-10-01-tui-skill-command-and-completion-design.md](2026-10-01-tui-skill-command-and-completion-design.md) —
  `/skill list` and the `/skill ` completion, the two user-facing surfaces this change marks but does
  not reshape.
