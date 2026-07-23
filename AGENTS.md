# Agent guidelines (Tact)

Conventions for AI agents working in this repository. Prefer small, focused diffs; do not commit unless asked.

## Documentation sync — when to update

Update docs **in the same change** (or immediately after) when behavior or public contracts change. Do not leave book / design docs lagging behind code.

| Trigger | Sync these |
|---------|------------|
| Agent loop / compaction / recovery behavior changes | `book/05_chapter_compact.md` **and** `book/05_chapter_compact_zh.md`; skim `ARCHITECTURE.md` §6 and `docs/compaction.md` if the overview drifts |
| Config / CLI flags rename or semantics change | `book/` chapter that documents them, `tact.example.toml`, relevant `docs/superpowers/specs/` or plans |
| TUI bottom-bar / token / cache display changes | `docs/token_usage_schema.md` (TUI display notes) and any book section that describes the bar |
| New multi-step feature from brainstorming | Write `docs/superpowers/specs/YYYY-MM-DD-<topic>-design.md` after design approval; add `docs/superpowers/plans/YYYY-MM-DD-<topic>.md` before or with implementation |
| Store / session persistence contracts change | `book/01_chapter_store*.md`, `docs/token_usage_schema.md` if usage tables change |
| Shipped optimization or bug fix with user/API-visible behavior change | Append a newest-first entry to `book/26_chapter_issue.md` **and** `book/26_chapter_issue_zh.md` (same section id / heading hierarchy). Link the PR, design spec/plan if any, and related subsystem chapters. Do **not** replace subsystem chapters — Ch 26 is the changelog; Ch 5/7/… remain the how-it-works docs |

### Issue log entry requirements (`book/26_chapter_issue*`)

When the sync table requires a Ch 26 entry, include at least:

- Date (`YYYY-MM-DD`), type (`optimization` / `bugfix` / `removal` / `docs`), optional PR URL
- Symptom / motivation before the change
- Final decision and observable post-change behavior
- Code / spec / related chapter pointers

Skip Ch 26 for pure refactors, test-only changes, and comment/typo-only edits (same as “When *not* required” below).

### Bilingual book chapters

Paired files `book/NN_chapter_*.md` and `book/NN_chapter_*_zh.md` must stay **structurally aligned**:

- Same section numbering and heading hierarchy
- Same mermaid / tables updated on both sides when the described behavior changes
- Prefer updating both in one commit when the change is behavioral

If only wording polish is needed on one language, that is fine; do not leave one language describing an obsolete algorithm.

### When *not* required

- Pure refactors with no user-visible or API-visible behavior change
- Test-only changes
- Typo fixes confined to code comments

## Compaction (quick pointer)

Current design: Codex-style rebuild — recent real user messages + `SUMMARY_PREFIX` handoff; entry path compacts **before** pushing `user_turn_message` and reserves incoming-turn size in `should_auto_compact`. Spec: `docs/superpowers/specs/2026-07-18-codex-style-compact-design.md`. Legacy single-summary path: `Agent::compact_history_legacy`.
