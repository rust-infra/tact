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
//! `block_id` or `tool_id`).
//!
//! Members are **indices**, not block copies: a region is rebuilt for every
//! logical row on every frame, and cloning a [`ToolBlock`] carries a whole
//! `ToolRenderOutput` (previews, detail text, a cached popup body).

use std::time::Duration;

use super::thinking::ThinkingBlock;
use super::tool_group::{self, ToolRun};
use super::tool_state::ToolBlock;

/// Which source slice a member's index refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegionMemberKind {
    /// `ThinkingState::blocks` — a finished card.
    Thinking,
    /// `ToolState::blocks` — a finished call.
    Tool,
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
    /// The region's stable identity: its first member's `block_id` or
    /// `tool_id`.
    ///
    /// Computed once while the source slices are in hand, because a `phys_idx`
    /// is rewritten by every insert above it and a fold keyed on one would
    /// silently jump to another region.
    pub key: String,
}

impl ActivityRegion {
    /// Rows this region saves when folded.
    #[must_use]
    pub fn saved_rows(&self) -> usize {
        self.expanded_rows.saturating_sub(self.folded_rows)
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

    /// Whether the region holds both a card and a call.
    ///
    /// Only a mixed region folds here; a region of tools only is left to
    /// [`super::tool_group`], whose summary names each tool.
    #[must_use]
    pub fn mixes_thinking_and_tools(&self) -> bool {
        self.thoughts() > 0 && self.tools() > 0
    }
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
    let mut members: Vec<(RegionMember, &str)> = thinking
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
                block.block_id.as_str(),
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
                block.tool_id.as_str(),
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
                key: members[start].1.to_string(),
            });
        }
        start = end;
    }
    found
}

/// Total wall clock a region occupied, or `None` when nothing in it was timed.
///
/// A thinking card carries an elapsed duration, tool blocks carry spans. When
/// both exist the spans decide, because a thought and the call it explains
/// shared one stretch of wall clock and adding the card's duration to the span
/// union would count part of it twice. A region of cards only has no spans, so
/// its cards' durations are the only measurement there is.
#[must_use]
pub fn total_span(
    thinking: &[ThinkingBlock],
    tools: &[ToolBlock],
    region: &ActivityRegion,
) -> Option<Duration> {
    let tool_blocks: Vec<ToolBlock> = region
        .members
        .iter()
        .filter(|member| member.kind == RegionMemberKind::Tool)
        .filter_map(|member| tools.get(member.index).cloned())
        .collect();
    if tool_blocks.is_empty() {
        let thoughts: Duration = region
            .members
            .iter()
            .filter(|member| member.kind == RegionMemberKind::Thinking)
            .filter_map(|member| thinking.get(member.index))
            .map(|block| block.elapsed)
            .sum();
        return (!thoughts.is_zero()).then_some(thoughts);
    }
    // `tool_group::total_span` wants indices into its own slice. Build the run
    // over exactly the tool members so a card between two calls cannot make
    // them look non-adjacent.
    let run = ToolRun {
        members: (0..tool_blocks.len()).collect(),
        phys_idx: 0,
        expanded_rows: 0,
        folded_rows: 0,
    };
    tool_group::total_span(&tool_blocks, &run)
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
    if !is_foldable(tools, region) || !region.mixes_thinking_and_tools() {
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
    regions.iter().position(|region| {
        phys_idx >= region.phys_idx && phys_idx < region.phys_idx + region.expanded_rows
    })
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
    let failures = titles
        .iter()
        .filter(|(phase, _)| phase.is_failure())
        .count();
    let mut text = String::new();
    if thoughts > 0 {
        let tmpl = if thoughts == 1 {
            msgs.fold_thoughts_one
        } else {
            msgs.fold_thoughts
        };
        text.push_str(&tmpl.replacen("{}", &thoughts.to_string(), 1));
    }
    if !titles.is_empty() {
        if !text.is_empty() {
            text.push_str(msgs.fold_sep);
        }
        // The tool half owns the failure count and the per-tool labels, so a
        // mixed region cannot state them twice or in another order than a
        // tool-only run does.
        text.push_str(&tool_group::tools_summary(titles, failures, msgs));
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
    use crate::widgets::tool_widget::{ToolLayout, ToolPhase, ToolRenderOutput, tool_visual_rows};

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
                    visual_rows: tool_visual_rows(false, 0, 0, false),
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
            (ToolPhase::Success, "1. 📖  Read  a.rs"),
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
            (ToolPhase::Success, "1. 📖  Read  a.rs"),
            (ToolPhase::Failed, "2. $ Bash  false"),
            (ToolPhase::Success, "3. 📖  Read  b.rs"),
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
