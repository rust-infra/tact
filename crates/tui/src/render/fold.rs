//! The fold decisions the log renderer needs, captured before it borrows
//! `log_scroll` mutably.
//!
//! The renderer cannot read `ToolState` while holding `&mut log_scroll`, and
//! cloning the blocks (each carries a `ToolRenderOutput`) every frame is too
//! expensive. So only the *decisions* are copied out: which runs are folded,
//! which rows that hides, and the generation that keys the caches.

use agent_tui_kit::state::tool_group::{self, ToolRun};
use agent_tui_kit::state::{ToolBlock, ToolState};

/// Which physical log rows a folded run hides.
#[derive(Debug, Default)]
pub(crate) struct FoldLayout {
    folds: Vec<Fold>,
    /// `ToolState::fold_rev` this was built from.
    pub(crate) rev: u64,
}

#[derive(Debug)]
struct Fold {
    run: ToolRun,
    folded: bool,
}

impl FoldLayout {
    /// Capture every run of `blocks` and whether it is folded right now.
    pub(crate) fn capture(blocks: &[ToolBlock], state: &ToolState) -> Self {
        let folds = tool_group::runs(blocks)
            .into_iter()
            .map(|run| Fold {
                folded: tool_group::is_folded(blocks, &run, state),
                run,
            })
            .collect();
        Self {
            folds,
            rev: state.fold_rev,
        }
    }

    /// A run starts at `phys_idx` and is currently drawn as one summary row.
    pub(crate) fn folded_run_at(&self, phys_idx: usize) -> Option<&ToolRun> {
        self.folds
            .iter()
            .find(|fold| fold.folded && fold.run.phys_idx == phys_idx)
            .map(|fold| &fold.run)
    }

    /// Is this physical row hidden by a folded run?
    ///
    /// The run's first row carries the summary, so only the members after it
    /// disappear.
    pub(crate) fn hides(&self, phys_idx: usize) -> bool {
        self.folds.iter().any(|fold| {
            fold.folded
                && phys_idx > fold.run.phys_idx
                && phys_idx < fold.run.phys_idx + fold.run.expanded_rows
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_tui_kit::widgets::tool_widget::{
        ToolLayout, ToolPhase, ToolRenderOutput, tool_visual_rows,
    };

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
            span: None,
        }
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
        let layout = FoldLayout::capture(&blocks, &ToolState::default());
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
        let layout = FoldLayout::capture(&blocks, &state);
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
        let layout = FoldLayout::capture(&blocks, &ToolState::default());
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
        let layout = FoldLayout::capture(&blocks, &ToolState::default());
        assert!(layout.folded_run_at(1).is_some());
        assert!(layout.folded_run_at(9).is_some());
        assert!(layout.hides(3), "first run's member");
        assert!(!layout.hides(6), "the gap row is not part of either run");
        assert!(layout.hides(11), "second run's member");
    }
}
