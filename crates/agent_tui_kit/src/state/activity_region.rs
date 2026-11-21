//! Thinking cards and tool blocks as **one** foldable region of work.
//!
//! The log used to have two independent fold units: a run of adjacent tool
//! blocks ([`super::tool_group`]) and a finished thinking card. A turn that
//! thought, then ran three commands, then thought again therefore cost two
//! card rows plus three tool rows even though every one of those rows
//! described the same stretch of work. The desktop transcript has always
//! folded them together (`ProcessGroup` in `crates/desktop/src/ui/process.rs`);
//! this is the log-side equivalent.
//!
//! Shape of the rule:
//!
//! - A *region* is a maximal run of adjacent thinking cards and tool blocks.
//! - Adjacency is derived from the occupied row ranges, exactly like
//!   [`super::tool_group::runs`]: the region ends at the first gap row, at any
//!   other block kind, or at the end of the log.
//! - A region of one member is not a fold candidate — there is nothing to get
//!   out of the way.
//! - A region that is *only* tool blocks keeps folding through
//!   [`super::tool_group`], so a plain tool run's summary (per-tool labels,
//!   failure count, union wall clock) is unchanged.
//! - A region that mixes at least one card with at least one call folds through
//!   this module, and its summary counts both halves:
//!   `2 thoughts · 3 tools · 1 failed · Read ×2 · Bash`.
//!
//! Membership is derived, never stored, for the same reason
//! [`super::tool_group`] derives it: a block inserted, resized or drained
//! cannot leave a stale member behind. Only the reader's explicit choice is
//! stored, keyed on the region's stable identity (its first member's
//! `block_id` or `tool_id`, or a narration run's generated key).
//!
//! Members are **indices**, not block copies: a region is rebuilt for every
//! logical row on every frame, and cloning a [`ToolBlock`] carries a whole
//! `ToolRenderOutput` (previews, detail text, a cached popup body).

use std::{collections::HashMap, time::Duration};

use super::thinking::ThinkingBlock;
use super::tool_group;
use super::tool_state::ToolBlock;

/// Which source slice a member's index refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegionMemberKind {
    /// `ThinkingState::blocks` — a finished card.
    Thinking,
    /// `ToolState::blocks` — a finished call.
    Tool,
    /// A run of adjacent `LogItemKind::AssistantCommentary` rows — assistant
    /// text the provider labelled as mid-turn narration.
    ///
    /// Indexed into the caller's commentary-run slice, not into the log: the
    /// fold rules already take member rows by index, and a run of prose rows is
    /// one member however many rows it wraps into.
    Commentary,
}

/// One member of an activity region.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegionMember {
    pub kind: RegionMemberKind,
    /// Index into the member kind's slice.
    pub index: usize,
    /// Physical row the member's first placeholder row owns.
    pub phys_idx: usize,
    /// Rows the member occupies when the region is expanded.
    pub rows: usize,
}

/// One region of adjacent thinking cards and tool blocks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivityRegion {
    /// The members, in display order.
    pub members: Vec<RegionMember>,
    /// Physical row the collapsed summary takes over (the first member's).
    pub phys_idx: usize,
    /// Rows the members occupy when the region is expanded.
    pub expanded_rows: usize,
    /// Rows the summary occupies when the region is folded (always 1).
    pub folded_rows: usize,
    /// The region's stable identity: its first member's `block_id`,
    /// `tool_id`, or narration-run key.
    ///
    /// Computed once while the source slices are in hand, because a `phys_idx`
    /// is rewritten by every insert above it and a fold keyed on one would
    /// silently jump to another region.
    pub key: String,
}

impl ActivityRegion {
    /// Whether `phys_idx` is one of the rows this region owns when expanded.
    #[must_use]
    pub fn contains(&self, phys_idx: usize) -> bool {
        phys_idx >= self.phys_idx && phys_idx < self.phys_idx + self.expanded_rows
    }

    /// How many thinking cards the region holds.
    #[must_use]
    pub fn thoughts(&self) -> usize {
        self.members
            .iter()
            .filter(|member| member.kind == RegionMemberKind::Thinking)
            .count()
    }

    /// How many tool blocks the region holds.
    #[must_use]
    pub fn tools(&self) -> usize {
        self.members
            .iter()
            .filter(|member| member.kind == RegionMemberKind::Tool)
            .count()
    }

    /// How many narration runs the region holds.
    #[must_use]
    pub fn commentary(&self) -> usize {
        self.members
            .iter()
            .filter(|member| member.kind == RegionMemberKind::Commentary)
            .count()
    }

    /// Whether the region holds both a card and a call.
    ///
    /// Only a mixed region folds here; a region of tools only is left to
    /// [`super::tool_group`], whose summary names each tool.
    #[must_use]
    pub fn mixes_thinking_and_tools(&self) -> bool {
        self.thoughts() > 0 && self.tools() > 0
    }

    /// Whether this region folds through [`super::activity_region`] at all.
    ///
    /// True when it holds a card and a call, or when it holds narration beside
    /// either — the two shapes the desktop transcript folds and the log did
    /// not. A region of calls alone stays with [`super::tool_group`], whose
    /// summary names each call.
    #[must_use]
    pub fn folds_here(&self) -> bool {
        self.mixes_thinking_and_tools()
            || (self.commentary() > 0 && (self.tools() > 0 || self.thoughts() > 0))
    }
}

/// One run of adjacent narration rows (`LogItemKind::AssistantCommentary`).
///
/// A run, not one member per row: a paragraph wraps to five log rows, and
/// counting rows would say `5 paragraphs` for one. The host builds these from
/// the log, because the kit does not own the log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommentaryRun {
    /// Physical row of the run's first row.
    pub phys_idx: usize,
    /// How many log rows the run covers. They are contiguous by construction.
    pub rows: usize,
    /// First non-blank row's text, used to derive [`Self::key`].
    ///
    /// The log has no id per paragraph, so the host passes the run's first
    /// row's text: it survives inserts above the run, where a `phys_idx`
    /// would not.
    pub identity: String,
    /// Unique fold key for a run that heads a region.
    ///
    /// Text alone is not enough: two turns can open with the same sentence,
    /// and a code-only commentary message has no text at all. The key combines
    /// the text with its occurrence count, so repeated narration never folds
    /// two regions together.
    pub key: String,
}

/// Build the activity regions of one log.
///
/// `thinking` is `ThinkingState::blocks` — finished cards only. The live card is
/// not a member and its rows break a region, which is what keeps a summary from
/// shrinking around a card whose height still changes.
///
/// The result is ordered by `phys_idx` and its members are disjoint.
#[must_use]
pub fn regions(thinking: &[ThinkingBlock], tools: &[ToolBlock]) -> Vec<ActivityRegion> {
    regions_bounded(thinking, tools, |_| false)
}

/// [`regions`] with an explicit turn boundary.
///
/// `is_turn_boundary` is asked about a physical row that sits between two
/// would-be members. The log is the only place a turn boundary exists
/// (`add_task_end_separator` and the user-message gap), and those rows already
/// break adjacency — this hook exists so a host that logs a turn end *without*
/// a row (a future out-of-band update, say) cannot silently fuse two turns into
/// one summary. Returning `true` ends the region at that row.
#[must_use]
pub fn regions_bounded(
    thinking: &[ThinkingBlock],
    tools: &[ToolBlock],
    is_turn_boundary: impl Fn(usize) -> bool,
) -> Vec<ActivityRegion> {
    regions_with_commentary(thinking, tools, &[], is_turn_boundary)
}

/// Group the log's rows into narration runs.
///
/// `is_owned` answers "does a card or a tool block own this row". Those rows are
/// blank too, and they are **not** narration: absorbing one would make a run
/// overlap a member and the region builder would drop the whole stretch.
///
/// Two callers need this — the host (for event handling) and the renderer (for
/// its layout caches) — and a second copy drifts on the one rule that matters:
/// how a run's `identity` and `key` are built. The identity is the first
/// **non-blank** row's text. A row's own text is what survives an insert above
/// it, but a narration run commonly opens with the blank gap that preceded its
/// paragraph, and keyed on `""` every such region would share one fold key —
/// folding one would fold them all. The key adds an occurrence counter so
/// repeated non-empty text stays distinct too.
#[must_use]
pub fn commentary_runs(
    items: &[crate::state::LogItem],
    is_owned: impl Fn(usize) -> bool,
) -> Vec<CommentaryRun> {
    let mut runs: Vec<CommentaryRun> = Vec::new();
    let mut current: Option<CommentaryRun> = None;
    for (phys, item) in items.iter().enumerate() {
        match item.kind {
            crate::state::LogItemKind::AssistantCommentary => match current.as_mut() {
                Some(run) => {
                    run.rows += 1;
                    if run.identity.is_empty() && !item.raw.trim().is_empty() {
                        run.identity = item.raw.clone();
                    }
                }
                None => {
                    current = Some(CommentaryRun {
                        phys_idx: phys,
                        rows: 1,
                        identity: if item.raw.trim().is_empty() {
                            String::new()
                        } else {
                            item.raw.clone()
                        },
                        key: String::new(),
                    });
                }
            },
            _ if item.raw.is_empty() && !is_owned(phys) => {
                if let Some(run) = current.as_mut() {
                    run.rows += 1;
                }
            }
            _ => {
                if let Some(run) = current.take() {
                    runs.push(run);
                }
            }
        }
    }
    if let Some(run) = current {
        runs.push(run);
    }
    let mut occurrences: HashMap<String, usize> = HashMap::new();
    for run in &mut runs {
        let occurrence = occurrences.entry(run.identity.clone()).or_insert(0);
        *occurrence += 1;
        run.key = format!("commentary\u{1f}{}\u{1f}{occurrence}", run.identity);
    }
    runs
}

/// [`regions_bounded`] with the log's narration runs included.
///
/// Narration is the half the desktop transcript folds and the log did not: the
/// "let me check that" text a model writes *before* it calls a tool. Only runs
/// the host already marked `AssistantCommentary` are included, which is what
/// keeps a `FinalAnswer` — and text no provider labelled — out of the fold.
#[must_use]
pub fn regions_with_commentary(
    thinking: &[ThinkingBlock],
    tools: &[ToolBlock],
    commentary: &[CommentaryRun],
    is_turn_boundary: impl Fn(usize) -> bool,
) -> Vec<ActivityRegion> {
    let mut members: Vec<(RegionMember, String)> = thinking
        .iter()
        .enumerate()
        .map(|(index, block)| {
            (
                RegionMember {
                    kind: RegionMemberKind::Thinking,
                    index,
                    phys_idx: block.phys_idx,
                    rows: crate::render::cells::thinking::COMPLETED_THINKING_ROWS,
                },
                block.block_id.clone(),
            )
        })
        .chain(tools.iter().enumerate().map(|(index, block)| {
            (
                RegionMember {
                    kind: RegionMemberKind::Tool,
                    index,
                    phys_idx: block.phys_idx,
                    rows: block.output.visual_rows(false),
                },
                block.tool_id.clone(),
            )
        }))
        .chain(commentary.iter().enumerate().map(|(index, run)| {
            (
                RegionMember {
                    kind: RegionMemberKind::Commentary,
                    index,
                    phys_idx: run.phys_idx,
                    rows: run.rows,
                },
                run.key.clone(),
            )
        }))
        .collect();
    members.sort_by_key(|(member, _)| member.phys_idx);

    let mut found = Vec::new();
    let mut start = 0;
    while start < members.len() {
        let (first, _) = &members[start];
        let mut next_phys = first.phys_idx + first.rows;
        let mut end = start + 1;
        while let Some((candidate, _)) = members.get(end) {
            if candidate.phys_idx != next_phys {
                break;
            }
            // A turn boundary anywhere between the two members ends the region
            // even when the rows touch: two turns must never share a summary.
            // The scan covers the previous member's last row through the
            // candidate's first, because a host that logs a turn end without a
            // row of its own can name any row in that gap.
            let gap_start = next_phys.saturating_sub(1);
            let gap_end = candidate.phys_idx + 1;
            if (gap_start..gap_end).any(&is_turn_boundary) {
                break;
            }
            next_phys += candidate.rows;
            end += 1;
        }
        if end - start >= 2 {
            found.push(ActivityRegion {
                members: members[start..end]
                    .iter()
                    .map(|(member, _)| *member)
                    .collect(),
                phys_idx: first.phys_idx,
                expanded_rows: next_phys - first.phys_idx,
                folded_rows: 1,
                key: members[start].1.clone(),
            });
        }
        start = end;
    }
    found
}

/// Total wall clock a region occupied, or `None` when nothing in it was timed.
///
/// The **union** of every member's span — thoughts and calls alike — for the
/// same reason [`super::tool_group::total_span`] unions a run's calls: a
/// thought and the call it explains share one stretch of wall clock, and calls
/// in a turn run concurrently by wave. Adding durations would report a thought
/// that overlapped a call twice, and would report four parallel one-second
/// calls as four seconds.
///
/// A member with no span (rebuilt from history) contributes nothing rather
/// than a fabricated zero. When no member has one, the answer is `None` — a
/// total that guessed would be a number the reader could not trust.
#[must_use]
pub fn total_span(
    thinking: &[ThinkingBlock],
    tools: &[ToolBlock],
    region: &ActivityRegion,
) -> Option<Duration> {
    let spans: Vec<super::tool_state::ToolSpan> = region
        .members
        .iter()
        .filter_map(|member| match member.kind {
            RegionMemberKind::Thinking => thinking.get(member.index)?.span,
            RegionMemberKind::Tool => tools.get(member.index)?.span,
            // Narration rows carry no clock of their own; the text arrived
            // inside the stream that the span around it already covers.
            RegionMemberKind::Commentary => None,
        })
        .collect();
    super::tool_state::unioned_span(&spans)
}

/// Whether a region can be folded yet.
///
/// One member is never a region worth folding; a live tool keeps every row it
/// owns, because its rows are still changing.
#[must_use]
pub fn is_foldable(tools: &[ToolBlock], region: &ActivityRegion) -> bool {
    region.members.len() >= 2
        && region.members.iter().all(|member| match member.kind {
            // A finished card is in `blocks`, so it is not live by definition.
            RegionMemberKind::Thinking => true,
            RegionMemberKind::Tool => tools
                .get(member.index)
                .is_some_and(|block| !block.output.phase.is_live()),
            // Narration rows are finished text: nothing in them can still be
            // changing, so a run never keeps a region open on its own.
            RegionMemberKind::Commentary => true,
        })
}

/// Whether the region is currently drawn as one summary row.
///
/// Inputs, in order: a region that cannot fold is always open; a reader's
/// explicit choice wins; otherwise a foldable **mixed** region folds.
///
/// A region of tools only is never folded here — [`super::tool_group::is_folded`]
/// owns that case, so the tool summary keeps its per-tool labels.
#[must_use]
pub fn is_folded(
    tools: &[ToolBlock],
    region: &ActivityRegion,
    folded: &std::collections::BTreeSet<String>,
    unfolded: &std::collections::BTreeSet<String>,
) -> bool {
    if !is_foldable(tools, region) || !region.folds_here() {
        return false;
    }
    if folded.contains(&region.key) {
        return true;
    }
    if unfolded.contains(&region.key) {
        return false;
    }
    true
}

/// The region containing `phys_idx`, if any.
#[must_use]
pub fn region_at_phys(regions: &[ActivityRegion], phys_idx: usize) -> Option<usize> {
    regions.iter().position(|region| region.contains(phys_idx))
}

/// The region whose stable identity is `key`.
#[must_use]
pub fn region_by_key(regions: &[ActivityRegion], key: &str) -> Option<usize> {
    regions.iter().position(|region| region.key == key)
}

/// What a folded region says about itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegionSummary {
    /// The whole line, cards and calls included:
    /// `"2 thoughts · 3 tools · 1 failed · Read ×2 · Bash"`.
    pub text: String,
    /// Wall clock of the whole region, carried beside `text` so the line stays
    /// a pure function of the titles.
    pub elapsed: Option<Duration>,
    /// How many calls ended without succeeding. The caller paints the summary
    /// in the error color when this is non-zero.
    pub failures: usize,
}

/// Summarize a region from its tool members' titles and phases plus a thought
/// count.
#[must_use]
pub fn summarize(
    thoughts: usize,
    titles: &[(crate::widgets::tool_widget::ToolPhase, &str)],
    elapsed: Option<Duration>,
    msgs: &crate::i18n::Messages,
) -> RegionSummary {
    summarize_with_commentary(thoughts, 0, titles, elapsed, msgs)
}

/// [`summarize`] with the region's narration runs counted.
///
/// A folded region that swallowed narration has to say so: the reader is
/// looking at one row where a paragraph used to be, and `1 note` is what tells
/// them the model said something rather than only calling tools.
#[must_use]
pub fn summarize_with_commentary(
    thoughts: usize,
    notes: usize,
    titles: &[(crate::widgets::tool_widget::ToolPhase, &str)],
    elapsed: Option<Duration>,
    msgs: &crate::i18n::Messages,
) -> RegionSummary {
    let failures = titles
        .iter()
        .filter(|(phase, _)| phase.is_failure())
        .count();
    let mut text = String::new();
    let push_part = |text: &mut String, part: String| {
        if !text.is_empty() {
            text.push_str(msgs.fold_sep);
        }
        text.push_str(&part);
    };
    if thoughts > 0 {
        let tmpl = if thoughts == 1 {
            msgs.fold_thoughts_one
        } else {
            msgs.fold_thoughts
        };
        push_part(&mut text, tmpl.replacen("{}", &thoughts.to_string(), 1));
    }
    if notes > 0 {
        let tmpl = if notes == 1 {
            msgs.fold_notes_one
        } else {
            msgs.fold_notes
        };
        push_part(&mut text, tmpl.replacen("{}", &notes.to_string(), 1));
    }
    if !titles.is_empty() {
        // The tool half owns the failure count and the per-tool labels, so a
        // mixed region cannot state them twice or in another order than a
        // tool-only run does.
        let part = tool_group::tools_summary(titles, failures, msgs);
        push_part(&mut text, part);
    }
    RegionSummary {
        text,
        elapsed,
        failures,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::widgets::tool_widget::{ToolLayout, ToolPhase, ToolRenderOutput};

    /// The English locale, so a summary's wording is pinned in one place.
    fn msgs() -> crate::i18n::Messages {
        crate::i18n::Messages::by_language(crate::i18n::Language::English)
    }
    use std::time::Instant;

    fn thinking(block_id: &str, phys_idx: usize) -> ThinkingBlock {
        ThinkingBlock::new(
            block_id.to_string(),
            phys_idx,
            "reasoned".into(),
            "reasoned".into(),
            Vec::new(),
            Duration::from_millis(400),
        )
    }

    fn tool(tool_id: &str, phys_idx: usize, phase: ToolPhase) -> ToolBlock {
        ToolBlock {
            phys_idx,
            tool_id: tool_id.to_string(),
            output: ToolRenderOutput {
                title_raw: format!("$ Bash  {tool_id}"),
                phase,
                permission_label: None,
                error_message: None,
                duration_us: Some(1_200),
                size_bytes: None,
                tool_name: "bash".to_string(),
                use_diff_gutter: false,
                arg_summary: tool_id.to_string(),
                arg_full: String::new(),
                layout: ToolLayout {
                    preview_lines: 0,
                    has_detail_card: false,
                    detail_collapsed: true,
                },
                live_detail: false,
                detail_preview: Vec::new(),
                detail_total_lines: 0,
                detail_full: None,
                subagent_id: None,
                subagent_model: None,
                subagent_tokens: None,
                subagent_context_window: None,
                task_id: None,
                visual_kind: tact_protocol::ToolVisualKind::Command,
            },
            span: Some(crate::state::ToolSpan {
                started_at: Instant::now(),
                finished_at: Instant::now(),
            }),
        }
    }

    fn no_overrides() -> (
        std::collections::BTreeSet<String>,
        std::collections::BTreeSet<String>,
    ) {
        Default::default()
    }

    #[test]
    fn a_card_and_the_call_after_it_are_one_region() {
        // The card occupies row 1 and one row, so the call starts at 2.
        let cards = [thinking("t1", 1)];
        let calls = [tool("a", 2, ToolPhase::Success)];
        let found = regions(&cards, &calls);
        assert_eq!(found.len(), 1, "adjacent card and call form one region");
        assert_eq!(found[0].thoughts(), 1);
        assert_eq!(found[0].tools(), 1);
        assert!(found[0].mixes_thinking_and_tools());
        assert_eq!(found[0].key, "t1", "the first member names the region");
        assert_eq!(found[0].expanded_rows, 3, "one card row plus two tool rows");
    }

    #[test]
    fn a_gap_row_breaks_the_region() {
        let cards = [thinking("t1", 1)];
        // Row 2 is a blank separator, so the call at 3 is its own region.
        let calls = [tool("a", 3, ToolPhase::Success)];
        assert!(regions(&cards, &calls).is_empty());
    }

    #[test]
    fn a_lone_card_is_not_a_region() {
        assert!(regions(&[thinking("t1", 1)], &[]).is_empty());
    }

    #[test]
    fn a_live_tool_makes_the_region_unfoldable() {
        let cards = [thinking("t1", 1)];
        let calls = [tool("a", 2, ToolPhase::Running)];
        let found = regions(&cards, &calls);
        assert_eq!(found.len(), 1);
        assert!(!is_foldable(&calls, &found[0]));
        let (folded, unfolded) = no_overrides();
        assert!(!is_folded(&calls, &found[0], &folded, &unfolded));
    }

    #[test]
    fn a_tools_only_region_leaves_folding_to_the_tool_group() {
        let calls = [
            tool("a", 1, ToolPhase::Success),
            tool("b", 3, ToolPhase::Success),
        ];
        let found = regions(&[], &calls);
        assert_eq!(found.len(), 1);
        assert!(is_foldable(&calls, &found[0]));
        let (folded, unfolded) = no_overrides();
        assert!(
            !is_folded(&calls, &found[0], &folded, &unfolded),
            "a tool-only region is folded by tool_group, not here"
        );
    }

    #[test]
    fn a_mixed_region_folds_by_default_and_stays_open_when_told() {
        let cards = [thinking("t1", 1)];
        let calls = [tool("a", 2, ToolPhase::Success)];
        let found = regions(&cards, &calls);
        let (folded, unfolded) = no_overrides();
        assert!(is_folded(&calls, &found[0], &folded, &unfolded));

        let mut unfolded = unfolded;
        unfolded.insert("t1".to_string());
        assert!(
            !is_folded(&calls, &found[0], &folded, &unfolded),
            "a reader who opened the region keeps it open"
        );
    }

    #[test]
    fn the_summary_counts_both_halves_and_puts_failures_before_the_labels() {
        let titles = [
            (ToolPhase::Success, "1. 📖 Read  a.rs"),
            (ToolPhase::Failed, "2. $ Bash  false"),
        ];
        let summary = summarize(2, &titles, Some(Duration::from_millis(900)), &msgs());
        assert_eq!(
            summary.text,
            "2 thoughts · 2 tools · 1 failed · 📖 Read · $ Bash"
        );
        assert_eq!(summary.failures, 1);
    }

    /// Every piece of the summary line goes through `Messages`, so `/lang`
    /// rewrites it. The glyphs that carry meaning (`×`) stay, the words do not.
    #[test]
    fn the_summary_follows_the_locale() {
        let titles = [
            (ToolPhase::Success, "1. 📖 Read  a.rs"),
            (ToolPhase::Failed, "2. $ Bash  false"),
            (ToolPhase::Success, "3. 📖 Read  b.rs"),
        ];
        let english = summarize(
            2,
            &titles,
            None,
            &crate::i18n::Messages::by_language(crate::i18n::Language::English),
        );
        assert_eq!(
            english.text,
            "2 thoughts · 3 tools · 1 failed · 📖 Read ×2 · $ Bash"
        );

        let chinese = summarize(
            2,
            &titles,
            None,
            &crate::i18n::Messages::by_language(crate::i18n::Language::Chinese),
        );
        assert!(
            chinese.text.contains("2 段思考"),
            "the thought count is localized: {}",
            chinese.text
        );
        assert!(
            chinese.text.contains("3 个工具"),
            "the tool count is localized: {}",
            chinese.text
        );
        assert!(
            chinese.text.contains("1 个失败"),
            "the failure count is localized: {}",
            chinese.text
        );
        assert!(
            chinese.text.contains("📖 Read ×2"),
            "the tool label and repeat glyph stay as they are: {}",
            chinese.text
        );
    }

    fn span(base: Instant, start_ms: u64, len_ms: u64) -> crate::state::ToolSpan {
        let started_at = base + Duration::from_millis(start_ms);
        crate::state::ToolSpan {
            started_at,
            finished_at: started_at + Duration::from_millis(len_ms),
        }
    }

    /// A thought and a call that overlapped took the time of one, not two: the
    /// card's duration must not be added on top of the call's span.
    #[test]
    fn a_thought_and_an_overlapping_call_count_once() {
        let base = Instant::now();
        let mut card = thinking("t1", 1);
        card.span = Some(span(base, 0, 1_000));
        let mut call = tool("a", 2, ToolPhase::Success);
        call.span = Some(span(base, 500, 500));
        let cards = [card];
        let calls = [call];
        let found = regions(&cards, &calls);
        assert_eq!(
            total_span(&cards, &calls, &found[0]),
            Some(Duration::from_millis(1_000)),
            "the union covers 0..1000, not 1000+500"
        );
    }

    /// Sequential members add up.
    #[test]
    fn a_thought_then_a_call_add_up() {
        let base = Instant::now();
        let mut card = thinking("t1", 1);
        card.span = Some(span(base, 0, 300));
        let mut call = tool("a", 2, ToolPhase::Success);
        call.span = Some(span(base, 300, 700));
        let cards = [card];
        let calls = [call];
        let found = regions(&cards, &calls);
        assert_eq!(
            total_span(&cards, &calls, &found[0]),
            Some(Duration::from_millis(1_000))
        );
    }

    /// A card with no span contributes nothing rather than a fabricated zero —
    /// the call's clock still answers the question.
    #[test]
    fn a_spanless_thought_does_not_shrink_a_measured_total() {
        let base = Instant::now();
        let card = thinking("t1", 1); // no span, only `elapsed`
        let mut call = tool("a", 2, ToolPhase::Success);
        call.span = Some(span(base, 0, 500));
        let cards = [card];
        let calls = [call];
        let found = regions(&cards, &calls);
        assert_eq!(
            total_span(&cards, &calls, &found[0]),
            Some(Duration::from_millis(500)),
            "a card rebuilt from history contributes no clock"
        );
    }

    /// Nothing timed at all reports nothing, not zero.
    #[test]
    fn a_region_with_no_clock_reports_none() {
        let mut card = thinking("t1", 1);
        card.span = None;
        let mut call = tool("a", 2, ToolPhase::Success);
        call.span = None;
        let cards = [card];
        let calls = [call];
        let found = regions(&cards, &calls);
        assert_eq!(total_span(&cards, &calls, &found[0]), None);
    }

    #[test]
    fn a_region_is_found_by_any_of_its_rows() {
        let cards = [thinking("t1", 1)];
        let calls = [tool("a", 2, ToolPhase::Success)];
        let found = regions(&cards, &calls);
        // The card owns row 1, the call owns rows 2 and 3.
        for phys in [1, 2, 3] {
            assert_eq!(region_at_phys(&found, phys), Some(0), "row {phys}");
        }
        assert_eq!(region_at_phys(&found, 0), None, "a row before the region");
        assert_eq!(region_at_phys(&found, 4), None, "a row after the region");
    }

    /// Two turns' activities must never share one summary, even when the rows
    /// happen to touch. The turn end is supplied by the host's log; without it
    /// adjacency alone would fuse them.
    #[test]
    fn a_turn_boundary_ends_the_region_even_between_adjacent_rows() {
        let cards = [thinking("card-a", 1), thinking("card-b", 4)];
        let calls = [
            tool("call-a", 2, ToolPhase::Success),
            tool("call-b", 5, ToolPhase::Success),
        ];
        // Row 3 is where turn 1 ended; no member owns it.
        let found = regions_bounded(&cards, &calls, |phys| phys == 3);
        assert_eq!(
            found.len(),
            2,
            "the boundary must split them: {:?}",
            found
                .iter()
                .map(|r| (r.thoughts(), r.tools()))
                .collect::<Vec<_>>()
        );
        assert_eq!(found[0].thoughts(), 1);
        assert_eq!(found[0].tools(), 1);
        assert_eq!(found[1].thoughts(), 1);
        assert_eq!(found[1].tools(), 1);
    }

    /// The complement: with no boundary, the same rows are one region, which is
    /// what pins the guarantee to the boundary rather than to spacing.
    #[test]
    fn without_a_boundary_the_same_rows_are_one_region() {
        let cards = [thinking("card-a", 1), thinking("card-b", 4)];
        let calls = [
            tool("call-a", 2, ToolPhase::Success),
            tool("call-b", 5, ToolPhase::Success),
        ];
        let found = regions_bounded(&cards, &calls, |_| false);
        assert_eq!(found.len(), 1, "adjacency alone merges them");
        assert_eq!(found[0].thoughts(), 2);
        assert_eq!(found[0].tools(), 2);
    }
}
