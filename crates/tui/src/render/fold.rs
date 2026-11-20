//! The fold decisions the log renderer needs, captured before it borrows
//! `log_scroll` mutably.
//!
//! The renderer cannot read `ToolState` while holding `&mut log_scroll`, and
//! cloning the blocks (each carries a `ToolRenderOutput`) every frame is too
//! expensive. So only the *decisions* are copied out: which runs are folded,
//! which regions are folded, which rows that hides, and the generation that
//! keys the caches.
//!
//! Two kinds of fold live here, and they are deliberately not merged:
//!
//! - a **tool run** ([`agent_tui_kit::state::tool_group`]) — adjacent calls
//!   only, summarized by per-tool labels;
//! - an **activity region** ([`agent_tui_kit::state::activity_region`]) — calls
//!   and finished thinking cards, summarized by `N thoughts · M tools · …`.
//!
//! A region that holds cards folds as a region; a tool-only span folds as a
//! run. The rules for which is which live in the two state modules, not here.

use agent_tui_kit::state::activity_region::{self, ActivityRegion};
use agent_tui_kit::state::thinking::ThinkingState;
use agent_tui_kit::state::tool_group::{self, ToolRun};
use agent_tui_kit::state::{ToolBlock, ToolState};

/// Which physical log rows a folded run or region hides.
#[derive(Debug, Default)]
pub(crate) struct FoldLayout {
    folds: Vec<Fold>,
    regions: Vec<Fold>,
    /// `ToolState::fold_rev` this was built from.
    pub(crate) rev: u64,
}

#[derive(Debug)]
struct Fold {
    phys_idx: usize,
    expanded_rows: usize,
    folded_rows: usize,
    folded: bool,
    /// `Some` for a tool-only run, `None` for a mixed region. Only the tests
    /// read them; the render path needs the geometry above and nothing else.
    #[cfg_attr(not(test), allow(dead_code))]
    run: Option<ToolRun>,
    #[cfg_attr(not(test), allow(dead_code))]
    region: Option<ActivityRegion>,
}

impl Fold {
    fn hides(&self, phys_idx: usize) -> bool {
        self.folded && phys_idx > self.phys_idx && phys_idx < self.phys_idx + self.expanded_rows
    }
}

impl FoldLayout {
    /// Capture every run and region of `blocks` and whether it is folded right
    /// now.
    ///
    /// `is_turn_boundary` is the log's own answer to "does this row end a
    /// turn" — see `App::is_turn_boundary_row`. It is passed rather than read
    /// because the fold is captured from disjoint borrows that cannot touch the
    /// log, and because a region must never span two turns.
    pub(crate) fn capture(
        blocks: &[ToolBlock],
        thinking: &ThinkingState,
        state: &ToolState,
        commentary: &[activity_region::CommentaryRun],
        is_turn_boundary: impl Fn(usize) -> bool,
    ) -> Self {
        // A run that sits **inside** a mixed region folds with that region: an
        // independent run summary would paint a second row for a stretch the
        // region already stands for, and the two row counts would disagree.
        let regions_found = activity_region::regions_with_commentary(
            &thinking.blocks,
            blocks,
            commentary,
            &is_turn_boundary,
        );
        let covered = |phys: usize| {
            regions_found
                .iter()
                .any(|region| region.folds_here() && region.contains(phys))
        };
        let folds = tool_group::runs(blocks)
            .into_iter()
            .filter(|run| !covered(run.phys_idx))
            .map(|run| Fold {
                phys_idx: run.phys_idx,
                expanded_rows: run.expanded_rows,
                folded_rows: run.folded_rows,
                folded: tool_group::is_folded(blocks, &run, state),
                run: Some(run),
                region: None,
            })
            .collect();
        let regions: Vec<Fold> = regions_found
            .into_iter()
            .map(|region| Fold {
                phys_idx: region.phys_idx,
                expanded_rows: region.expanded_rows,
                folded_rows: region.folded_rows,
                folded: activity_region::is_folded(
                    blocks,
                    &region,
                    &state.folded_regions,
                    &state.unfolded_regions,
                ),
                run: None,
                region: Some(region),
            })
            .collect();

        Self {
            folds,
            regions,
            rev: state.fold_rev,
        }
    }

    /// A run starts at `phys_idx` and is currently drawn as one summary row.
    ///
    /// Test-only: the render path asks [`Self::folded_rows_at`], which covers a
    /// run and a region in one question.
    #[cfg(test)]
    pub(crate) fn folded_run_at(&self, phys_idx: usize) -> Option<&ToolRun> {
        self.folds
            .iter()
            .find(|fold| fold.folded && fold.phys_idx == phys_idx)
            .and_then(|fold| fold.run.as_ref())
    }

    /// A mixed activity region starts at `phys_idx` and is currently drawn as
    /// one summary row.
    ///
    /// Test-only, for the same reason as [`Self::folded_run_at`].
    #[cfg(test)]
    pub(crate) fn folded_region_at(&self, phys_idx: usize) -> Option<&ActivityRegion> {
        self.regions
            .iter()
            .find(|fold| fold.folded && fold.phys_idx == phys_idx)
            .and_then(|fold| fold.region.as_ref())
    }

    /// Is this physical row hidden by a folded run or region?
    ///
    /// The fold's first row carries the summary, so only the members after it
    /// disappear.
    pub(crate) fn hides(&self, phys_idx: usize) -> bool {
        self.folds.iter().any(|fold| fold.hides(phys_idx))
            || self.regions.iter().any(|fold| fold.hides(phys_idx))
    }

    /// Rows the fold starting at `phys_idx` reserves, or `None` when nothing
    /// starts there.
    pub(crate) fn folded_rows_at(&self, phys_idx: usize) -> Option<usize> {
        self.folds
            .iter()
            .chain(self.regions.iter())
            .find(|fold| fold.folded && fold.phys_idx == phys_idx)
            .map(|fold| fold.folded_rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_tui_kit::state::thinking::ThinkingBlock;
    use agent_tui_kit::widgets::tool_widget::{ToolLayout, ToolPhase, ToolRenderOutput};
    use std::time::Duration;

    fn block(tool_id: &str, phys_idx: usize, phase: ToolPhase) -> ToolBlock {
        ToolBlock {
            phys_idx,
            tool_id: tool_id.to_string(),
            output: ToolRenderOutput {
                title_raw: format!("$ Bash  {tool_id}"),
                phase,
                permission_label: None,
                error_message: None,
                duration_us: None,
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
            span: None,
        }
    }

    fn card(block_id: &str, phys_idx: usize) -> ThinkingBlock {
        ThinkingBlock::new(
            block_id.to_string(),
            phys_idx,
            "reasoned".into(),
            "reasoned".into(),
            Vec::new(),
            Duration::from_millis(200),
        )
    }

    fn three_successful() -> Vec<ToolBlock> {
        vec![
            block("a", 1, ToolPhase::Success),
            block("b", 3, ToolPhase::Success),
            block("c", 5, ToolPhase::Success),
        ]
    }

    #[test]
    fn a_settled_run_starts_folded_and_hides_its_members() {
        let blocks = three_successful();
        let layout = FoldLayout::capture(
            &blocks,
            &ThinkingState::default(),
            &ToolState::default(),
            &[],
            |_| false,
        );
        assert!(layout.folded_run_at(1).is_some(), "the run starts folded");
        assert!(!layout.hides(1), "the summary row itself is visible");
        assert!(layout.hides(3), "member b is hidden");
        assert!(layout.hides(5), "member c is hidden");
    }

    #[test]
    fn expanding_shows_every_member_again() {
        let blocks = three_successful();
        let mut state = ToolState {
            blocks: blocks.clone(),
            ..ToolState::default()
        };
        assert!(state.toggle_run_at(1));
        let layout =
            FoldLayout::capture(&blocks, &ThinkingState::default(), &state, &[], |_| false);
        assert!(layout.folded_run_at(1).is_none());
        for phys in [1, 3, 5] {
            assert!(!layout.hides(phys), "{phys} must be visible again");
        }
    }

    #[test]
    fn a_live_run_never_folds() {
        let blocks = vec![
            block("a", 1, ToolPhase::Success),
            block("b", 3, ToolPhase::Running),
        ];
        let layout = FoldLayout::capture(
            &blocks,
            &ThinkingState::default(),
            &ToolState::default(),
            &[],
            |_| false,
        );
        assert!(layout.folded_run_at(1).is_none());
        assert!(!layout.hides(3));
    }

    #[test]
    fn a_gap_between_runs_keeps_them_apart() {
        let blocks = vec![
            block("a", 1, ToolPhase::Success),
            block("b", 3, ToolPhase::Success),
            block("c", 9, ToolPhase::Success),
            block("d", 11, ToolPhase::Success),
        ];
        let layout = FoldLayout::capture(
            &blocks,
            &ThinkingState::default(),
            &ToolState::default(),
            &[],
            |_| false,
        );
        assert!(layout.folded_run_at(1).is_some());
        assert!(layout.folded_run_at(9).is_some());
        assert!(layout.hides(3), "first run's member");
        assert!(!layout.hides(6), "the gap row is not part of either run");
        assert!(layout.hides(11), "second run's member");
    }

    #[test]
    fn a_card_and_the_calls_after_it_fold_as_one_region() {
        // Card at row 1 (one row), calls at 2 and 4.
        let blocks = vec![
            block("a", 2, ToolPhase::Success),
            block("b", 4, ToolPhase::Success),
        ];
        let thinking = ThinkingState {
            blocks: vec![card("t1", 1)],
            ..Default::default()
        };
        let layout = FoldLayout::capture(&blocks, &thinking, &ToolState::default(), &[], |_| false);
        let region = layout
            .folded_region_at(1)
            .expect("a mixed region folds by default");
        assert_eq!(region.thoughts(), 1);
        assert_eq!(region.tools(), 2);
        assert_eq!(
            layout.folded_run_at(1),
            None,
            "the region wins: a tool-only run does not start at the card"
        );
        assert!(!layout.hides(1), "the summary row stays visible");
        assert!(layout.hides(2), "the first call is behind the summary");
        assert!(layout.hides(4), "the second call is too");
    }

    #[test]
    fn a_tool_only_run_still_folds_through_the_tool_group() {
        let blocks = three_successful();
        let thinking = ThinkingState::default();
        let layout = FoldLayout::capture(&blocks, &thinking, &ToolState::default(), &[], |_| false);
        assert!(layout.folded_region_at(1).is_none());
        assert!(layout.folded_run_at(1).is_some());
    }

    #[test]
    fn a_folded_region_reserves_one_row_where_it_starts() {
        let blocks = vec![block("a", 2, ToolPhase::Success)];
        let thinking = ThinkingState {
            blocks: vec![card("t1", 1)],
            ..Default::default()
        };
        let layout = FoldLayout::capture(&blocks, &thinking, &ToolState::default(), &[], |_| false);
        assert_eq!(
            layout.folded_rows_at(1),
            Some(1),
            "the card's own row becomes the summary row"
        );
    }
}
