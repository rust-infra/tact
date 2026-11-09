//! Runs of adjacent tool blocks, folded into one summary row.
//!
//! A turn that dispatches several tools renders two rows per block (title and
//! meta), and the meta row of every block in a run says the same thing
//! ("✓ Success · 1.20ms · 3 lines"). A run of adjacent, finished blocks is
//! therefore folded to a single summary row, the way the desktop transcript's
//! `ToolGroupRow` folds its members.
//!
//! Adjacency is derived, never stored: [`runs`] reads `blocks` in insertion
//! order and compares each block's reserved row range with the next. Keeping it
//! a pure function means a block added, resized, or drained cannot leave a
//! stale membership behind.

use crate::state::ToolBlock;
use crate::widgets::tool_widget::ToolPhase;

/// One run of adjacent tool blocks that the log can fold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolRun {
    /// Indices into the `blocks` slice, in display order. Always `>= 2`.
    pub members: Vec<usize>,
    /// Physical log row the summary takes over (the first member's).
    pub phys_idx: usize,
    /// Rows the members occupy when the run is expanded.
    pub expanded_rows: usize,
    /// Rows the summary occupies when the run is folded.
    pub folded_rows: usize,
}

impl ToolRun {
    /// Rows this run saves when folded.
    pub fn saved_rows(&self) -> usize {
        self.expanded_rows.saturating_sub(self.folded_rows)
    }
}

/// Runs of blocks that sit back to back in the log.
///
/// Two blocks are adjacent when the second starts exactly where the first ends.
/// A blank gap row, a thinking card, or any other row between them breaks the
/// run: those rows are the reader's evidence that the calls were separate, and
/// folding across them would attribute one call's rows to another.
pub fn runs(blocks: &[ToolBlock]) -> Vec<ToolRun> {
    let mut found = Vec::new();
    let mut start = 0;
    while start < blocks.len() {
        let first = &blocks[start];
        let mut next_phys = first.phys_idx + first.output.visual_rows(false);
        let mut end = start + 1;
        while let Some(candidate) = blocks.get(end) {
            if candidate.phys_idx != next_phys {
                break;
            }
            next_phys += candidate.output.visual_rows(false);
            end += 1;
        }
        if end - start >= 2 {
            found.push(ToolRun {
                members: (start..end).collect(),
                phys_idx: first.phys_idx,
                expanded_rows: next_phys - first.phys_idx,
                folded_rows: 1,
            });
        }
        start = end;
    }
    found
}

/// What a folded run says about itself.
///
/// The titles are per-call (`"3. 📖  Read  src/app.rs"`), so the summary first
/// drops the step ordinal and the argument, then counts the remaining labels.
/// Repeating `Read ×3` once is the point of folding — three paths on three rows
/// say less than "three reads".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoldSummary {
    /// The whole line, failures included: `"5 tools · Read ×3 · Bash"`.
    pub text: String,
    /// How many members ended without succeeding.
    ///
    /// The caller paints the row in the error color when this is non-zero:
    /// folding must not make a failure quieter than it was when every member
    /// had its own row.
    pub failures: usize,
}

/// Summarize a run by its members' phases and titles.
pub fn summarize(members: &[(ToolPhase, &str)]) -> FoldSummary {
    let failures = members
        .iter()
        .filter(|(phase, _)| phase.is_failure())
        .count();
    FoldSummary {
        text: fold_summary(
            &members.iter().map(|(_, title)| *title).collect::<Vec<_>>(),
            failures,
        ),
        failures,
    }
}

/// `"5 tools · 2 failed · Read ×3 · Bash"` — the failure count comes right
/// after the total so a fold can never hide one behind the tool labels.
fn fold_summary(titles: &[&str], failures: usize) -> String {
    let mut labels: Vec<String> = Vec::with_capacity(titles.len());
    let mut counts: Vec<usize> = Vec::with_capacity(titles.len());
    for title in titles {
        let label = tool_label(title);
        match labels.iter().position(|known| known == &label) {
            Some(index) => counts[index] += 1,
            None => {
                labels.push(label);
                counts.push(1);
            }
        }
    }
    let mut text = format!("{} tools", titles.len());
    if failures > 0 {
        text.push_str(&format!(" · {failures} failed"));
    }
    for (label, count) in labels.iter().zip(&counts) {
        text.push_str(" · ");
        text.push_str(label);
        if *count > 1 {
            text.push_str(&format!(" ×{count}"));
        }
    }
    text
}

/// The tool label inside a title row, with the step ordinal and the argument
/// removed.
///
/// A title is built as `"<ordinal>. <glyph> <Name>  <argument>"` — note the
/// argument is separated by two spaces while the glyph and the name are not, so
/// the argument starts at the **last** double-space run, not the first. A title
/// without an ordinal (the subagent popup builds its own) and a title without an
/// argument both fall back to what is actually there rather than printing an
/// empty label.
fn tool_label(title: &str) -> String {
    let without_ordinal = match title.split_once(". ") {
        Some((ordinal, rest)) if ordinal.chars().all(|c| c.is_ascii_digit()) => rest,
        _ => title,
    };
    match without_ordinal.rsplit_once("  ") {
        Some((label, _argument)) if !label.trim().is_empty() => normalize_spaces(label),
        _ => normalize_spaces(without_ordinal),
    }
}

/// Collapse runs of spaces to one, so `"📖  Read"` and `"$ Bash"` read alike.
fn normalize_spaces(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Whether a run can be folded yet.
///
/// Only a run with a live member keeps its rows. `Success`, `Failed`, and
/// `Cancelled` members all fold: a failure does not vanish into the summary —
/// [`summarize`] counts it and the summary row paints it in the error color, so
/// an error stays visible while costing one row instead of ten. Expanding is
/// how the reader gets the message itself.
pub fn is_foldable(blocks: &[ToolBlock], run: &ToolRun) -> bool {
    run.members.iter().all(|&index| {
        blocks
            .get(index)
            .is_some_and(|block| !block.output.phase.is_live())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::widgets::tool_widget::{ToolLayout, ToolPhase, ToolRenderOutput, tool_visual_rows};

    /// A block whose layout reserves `rows` visual rows and draws no card.
    fn block(tool_id: &str, phys_idx: usize, phase: ToolPhase) -> ToolBlock {
        ToolBlock {
            phys_idx,
            tool_id: tool_id.to_string(),
            output: ToolRenderOutput {
                title_raw: format!("Bash  {tool_id}"),
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
                    detail_collapsed: false,
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
        }
    }

    #[test]
    fn a_summary_counts_repeated_tools_once() {
        let members = [
            (ToolPhase::Success, "1. 📖  Read  src/main.rs"),
            (ToolPhase::Success, "2. 📖  Read  src/lib.rs"),
            (ToolPhase::Success, "3. 📖  Read  src/app.rs"),
            (ToolPhase::Success, "4. $ Bash  cargo test"),
            (ToolPhase::Success, "5. Grep  fn main"),
        ];
        let summary = summarize(&members);
        assert_eq!(
            summary.text, "5 tools · 📖 Read ×3 · $ Bash · Grep",
            "repeated tools must collapse to one counted label"
        );
        assert_eq!(summary.failures, 0);
    }

    #[test]
    fn a_failure_is_counted_in_the_summary_not_hidden_by_it() {
        let members = [
            (ToolPhase::Success, "1. 📖  Read  a.rs"),
            (ToolPhase::Failed, "2. $ Bash  cargo test"),
            (ToolPhase::Cancelled, "3. $ Bash  cargo build"),
        ];
        let summary = summarize(&members);
        assert_eq!(summary.failures, 2, "failed and cancelled both count");
        assert!(
            summary.text.contains("2 failed"),
            "the failure count must be in the line a folded run shows: {}",
            summary.text
        );
    }

    #[test]
    fn a_summary_survives_titles_without_an_ordinal_or_an_argument() {
        assert_eq!(
            summarize(&[(ToolPhase::Success, "Bash")]).text,
            "1 tools · Bash"
        );
        assert_eq!(
            summarize(&[(ToolPhase::Success, "1. Bash")]).text,
            "1 tools · Bash"
        );
    }

    #[test]
    fn adjacent_blocks_form_one_run() {
        let blocks = vec![
            block("a", 1, ToolPhase::Success),
            block("b", 3, ToolPhase::Success),
            block("c", 5, ToolPhase::Success),
        ];
        let found = runs(&blocks);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].members, vec![0, 1, 2]);
        assert_eq!(found[0].phys_idx, 1);
        assert_eq!(found[0].expanded_rows, 6);
        assert_eq!(found[0].saved_rows(), 5);
    }

    #[test]
    fn a_gap_row_breaks_the_run() {
        // `b` starts at 4, not 3: one row sits between them.
        let blocks = vec![
            block("a", 1, ToolPhase::Success),
            block("b", 4, ToolPhase::Success),
        ];
        assert!(runs(&blocks).is_empty());
    }

    #[test]
    fn a_lone_block_is_not_a_run() {
        let blocks = vec![block("a", 1, ToolPhase::Success)];
        assert!(runs(&blocks).is_empty());
    }

    #[test]
    fn a_failed_member_still_folds() {
        let blocks = vec![
            block("a", 1, ToolPhase::Success),
            block("b", 3, ToolPhase::Failed),
        ];
        let found = runs(&blocks);
        assert_eq!(found.len(), 1, "the run is still detected");
        assert!(
            is_foldable(&blocks, &found[0]),
            "a failed block folds like any finished one; the summary counts it"
        );
    }

    #[test]
    fn a_cancelled_member_still_folds() {
        let blocks = vec![
            block("a", 1, ToolPhase::Success),
            block("b", 3, ToolPhase::Cancelled),
        ];
        let found = runs(&blocks);
        assert!(is_foldable(&blocks, &found[0]));
    }

    #[test]
    fn a_live_member_keeps_the_run_unfolded() {
        for phase in [ToolPhase::Running, ToolPhase::Queued] {
            let blocks = vec![block("a", 1, ToolPhase::Success), block("b", 3, phase)];
            let found = runs(&blocks);
            assert_eq!(found.len(), 1);
            assert!(
                !is_foldable(&blocks, &found[0]),
                "{phase:?} must not fold: its rows are still changing"
            );
        }
    }

    #[test]
    fn two_runs_separated_by_a_gap_are_independent() {
        let blocks = vec![
            block("a", 1, ToolPhase::Success),
            block("b", 3, ToolPhase::Success),
            block("c", 8, ToolPhase::Success),
            block("d", 10, ToolPhase::Success),
        ];
        let found = runs(&blocks);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].members, vec![0, 1]);
        assert_eq!(found[1].members, vec![2, 3]);
    }
}
