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

use std::time::Duration;

use crate::state::{ToolBlock, ToolSpan, ToolState};
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

/// Total wall clock a run occupied, or `None` when no member was timed.
///
/// Calls in one turn run concurrently, so this is the **union** of the members'
/// spans rather than the sum of their durations: four parallel 1s reads take
/// 1s, not 4s. Summing would overstate every parallel wave, which is the common
/// case for a run worth folding.
///
/// A member with no span (rebuilt from history) contributes nothing; if no
/// member has one the total is `None`, because a fabricated zero would read as
/// "instantaneous".
pub fn total_span(blocks: &[ToolBlock], run: &ToolRun) -> Option<Duration> {
    let mut spans: Vec<ToolSpan> = run
        .members
        .iter()
        .filter_map(|&index| blocks.get(index)?.span)
        .collect();
    if spans.is_empty() {
        return None;
    }
    spans.sort_by_key(|span| span.started_at);

    let mut total = Duration::ZERO;
    let mut current_start = spans[0].started_at;
    let mut current_end = spans[0].finished_at;
    for span in &spans[1..] {
        if span.started_at <= current_end {
            // Overlaps (or touches) the interval being accumulated.
            current_end = current_end.max(span.finished_at);
        } else {
            total += current_end.saturating_duration_since(current_start);
            current_start = span.started_at;
            current_end = span.finished_at;
        }
    }
    total += current_end.saturating_duration_since(current_start);
    Some(total)
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
    /// Wall clock of the whole run, appended by the caller that can measure it.
    /// Kept out of `text` so the line stays a pure function of the titles.
    pub elapsed: Option<Duration>,
    /// How many members ended without succeeding.
    ///
    /// The caller paints the row in the error color when this is non-zero:
    /// folding must not make a failure quieter than it was when every member
    /// had its own row.
    pub failures: usize,
}

/// Summarize a run by its members' phases and titles.
///
/// `elapsed` is the run's wall clock ([`total_span`]); it is carried beside the
/// line rather than folded into it, so a caller with no clock still gets the
/// same text.
pub fn summarize(members: &[(ToolPhase, &str)], elapsed: Option<Duration>) -> FoldSummary {
    let failures = members
        .iter()
        .filter(|(phase, _)| phase.is_failure())
        .count();
    FoldSummary {
        text: fold_summary(
            &members.iter().map(|(_, title)| *title).collect::<Vec<_>>(),
            failures,
        ),
        elapsed,
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

/// Whether this run is currently drawn as one summary row.
///
/// Three inputs, in order:
/// 1. A run that cannot fold yet is always open.
/// 2. A reader's explicit choice wins over the default, which is what keeps an
///    expanded run from snapping shut when the next tool settles.
/// 3. Otherwise a foldable run starts folded.
///
/// Step 3 is why a run nobody has touched is already folded: the point of the
/// feature is to get a turn's tool noise out of the way, and a reader who wants
/// the detail has a click and a key to say so.
pub fn is_folded(blocks: &[ToolBlock], run: &ToolRun, state: &ToolState) -> bool {
    is_folded_by(blocks, run, &state.folded, &state.unfolded)
}

/// [`is_folded`] over the two sets alone, so callers that already hold the
/// fields split can ask without a whole `ToolState`.
pub fn is_folded_by(
    blocks: &[ToolBlock],
    run: &ToolRun,
    folded: &std::collections::BTreeSet<String>,
    unfolded: &std::collections::BTreeSet<String>,
) -> bool {
    if !is_foldable(blocks, run) {
        return false;
    }
    let Some(key) = run_key(blocks, run) else {
        return false;
    };
    if folded.contains(key) {
        return true;
    }
    if unfolded.contains(key) {
        return false;
    }
    true
}

/// The stable identity of a run: its first member's tool id.
///
/// Used as the fold-state key. `None` when the run is empty, which callers treat
/// as "not foldable" rather than inventing a key.
pub fn run_key<'a>(blocks: &'a [ToolBlock], run: &ToolRun) -> Option<&'a str> {
    let index = *run.members.first()?;
    blocks.get(index).map(|block| block.tool_id.as_str())
}

/// Flip one run's fold state, recording it as the reader's explicit choice.
///
/// Records an override in the set that *differs* from the new state, so the
/// default (folded) never has to be stored: an unfolded run stores itself in
/// `unfolded`, a folded one drops out of `unfolded` and lands in `folded` only
/// when the default would have kept it open.
pub fn toggle(
    blocks: &[ToolBlock],
    run: &ToolRun,
    folded: &mut std::collections::BTreeSet<String>,
    unfolded: &mut std::collections::BTreeSet<String>,
    fold_rev: &mut u64,
) {
    let Some(key) = run_key(blocks, run).map(str::to_string) else {
        return;
    };
    let now_folded = is_folded_by(blocks, run, folded, unfolded);
    if now_folded {
        folded.remove(&key);
        unfolded.insert(key);
    } else {
        unfolded.remove(&key);
        folded.insert(key);
    }
    *fold_rev = fold_rev.wrapping_add(1);
}

/// The run containing `phys_idx`, if any.
pub fn run_at_phys(runs: &[ToolRun], phys_idx: usize) -> Option<usize> {
    runs.iter()
        .position(|run| phys_idx >= run.phys_idx && phys_idx < run.phys_idx + run.expanded_rows)
}

impl ToolState {
    /// The folded summary cell for the run starting at `phys_idx`, if that run
    /// is folded right now.
    ///
    /// Lives here, beside the fold rules, so the pure log renderer can ask
    /// without a second copy of them. The desktop-side cache build asks the same
    /// [`is_folded`].
    /// The common case is "this row is not a folded run", which has to be
    /// answered for **every** logical row on every frame. Checking the cheap
    /// case first keeps a fold from turning the render loop quadratic: `runs`
    /// walks every block, and the log redraws each line.
    pub fn fold_summary(
        &self,
        phys_idx: usize,
        theme: &crate::theme::Theme,
        msgs: &crate::i18n::Messages,
    ) -> Option<crate::render::cells::tool_run::ToolRunCell> {
        if !self.blocks.iter().any(|block| block.phys_idx == phys_idx) {
            return None;
        }
        let found = runs(&self.blocks);
        let run = found
            .into_iter()
            .find(|run| run.phys_idx == phys_idx && is_folded(&self.blocks, run, self))?;
        let key = run_key(&self.blocks, &run)?.to_string();
        let members = run
            .members
            .iter()
            .filter_map(|&index| self.blocks.get(index))
            .map(|block| (block.output.phase, block.output.title_raw.clone()))
            .collect();
        let elapsed = total_span(&self.blocks, &run);
        Some(
            crate::render::cells::tool_run::ToolRunCell::new(
                &run, members, elapsed, true, theme, msgs,
            )
            .with_identity(key),
        )
    }

    /// Flip the run that owns `phys_idx`, if any.
    pub fn toggle_run_at(&mut self, phys_idx: usize) -> bool {
        let found = runs(&self.blocks);
        let Some(index) = run_at_phys(&found, phys_idx) else {
            return false;
        };
        // Field-split: `toggle` needs `blocks` and the fold sets at once, and
        // both live on `self`.
        let Self {
            blocks,
            folded,
            unfolded,
            fold_rev,
            ..
        } = self;
        toggle(blocks, &found[index], folded, unfolded, fold_rev);
        true
    }

    /// Flip the run whose stable identity is `run_id`.
    pub fn toggle_run_by_id(&mut self, run_id: &str) -> bool {
        let found = runs(&self.blocks);
        let Some(run) = found
            .into_iter()
            .find(|run| run_key(&self.blocks, run) == Some(run_id))
        else {
            return false;
        };
        let Self {
            blocks,
            folded,
            unfolded,
            fold_rev,
            ..
        } = self;
        toggle(blocks, &run, folded, unfolded, fold_rev);
        true
    }

    /// The row bounds of the run whose identity is `run_id`, plus the rows it
    /// hides while folded.
    ///
    /// Used to compensate the scroll position after a fold: the caller needs to
    /// know how much disappeared and whether it all sat above the viewport.
    pub fn run_bounds_by_id(&self, run_id: &str) -> Option<(usize, usize, usize)> {
        let found = runs(&self.blocks);
        let run = found
            .into_iter()
            .find(|run| run_key(&self.blocks, run) == Some(run_id))?;
        if !is_folded(&self.blocks, &run, self) {
            return None;
        }
        Some((
            run.phys_idx,
            run.phys_idx + run.expanded_rows,
            run.saved_rows(),
        ))
    }

    /// The bounds of a run that is currently **unfolded**, plus the rows it
    /// would hide if folded.
    ///
    /// The mirror of [`Self::run_bounds_by_id`]: a caller about to fold needs
    /// the geometry before the toggle, and the fold state it will leave.
    pub fn run_bounds_if_unfolded(&self, run_id: &str) -> Option<(usize, usize, usize)> {
        let found = runs(&self.blocks);
        let run = found
            .into_iter()
            .find(|run| run_key(&self.blocks, run) == Some(run_id))?;
        if is_folded(&self.blocks, &run, self) || !is_foldable(&self.blocks, &run) {
            return None;
        }
        Some((
            run.phys_idx,
            run.phys_idx + run.expanded_rows,
            run.saved_rows(),
        ))
    }

    /// The stable identity of the run owning `phys_idx`, if any.
    pub fn run_identity_at(&self, phys_idx: usize) -> Option<String> {
        let found = runs(&self.blocks);
        let index = run_at_phys(&found, phys_idx)?;
        run_key(&self.blocks, &found[index]).map(str::to_string)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::widgets::tool_widget::{ToolLayout, ToolPhase, ToolRenderOutput, tool_visual_rows};
    use std::time::Instant;

    /// A block whose layout reserves `rows` visual rows and draws no card.
    fn block(tool_id: &str, phys_idx: usize, phase: ToolPhase) -> ToolBlock {
        block_at(tool_id, phys_idx, phase, None)
    }

    fn block_at(
        tool_id: &str,
        phys_idx: usize,
        phase: ToolPhase,
        span: Option<ToolSpan>,
    ) -> ToolBlock {
        ToolBlock {
            span,
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
        let summary = summarize(&members, None);
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
        let summary = summarize(&members, None);
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
            summarize(&[(ToolPhase::Success, "Bash")], None).text,
            "1 tools · Bash"
        );
        assert_eq!(
            summarize(&[(ToolPhase::Success, "1. Bash")], None).text,
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

    /// A span starting `start_ms` after `base` and running for `len_ms`.
    fn span(base: Instant, start_ms: u64, len_ms: u64) -> ToolSpan {
        let started_at = base + Duration::from_millis(start_ms);
        ToolSpan {
            started_at,
            finished_at: started_at + Duration::from_millis(len_ms),
        }
    }

    #[test]
    fn parallel_calls_report_the_union_not_the_sum() {
        let base = Instant::now();
        // Four 1s calls that all start together: 1s of wall clock, not 4s.
        let blocks = vec![
            block_at("a", 1, ToolPhase::Success, Some(span(base, 0, 1000))),
            block_at("b", 3, ToolPhase::Success, Some(span(base, 0, 1000))),
            block_at("c", 5, ToolPhase::Success, Some(span(base, 0, 1000))),
            block_at("d", 7, ToolPhase::Success, Some(span(base, 0, 1000))),
        ];
        let run = &runs(&blocks)[0];
        assert_eq!(total_span(&blocks, run), Some(Duration::from_millis(1000)));
    }

    #[test]
    fn sequential_calls_add_up() {
        let base = Instant::now();
        let blocks = vec![
            block_at("a", 1, ToolPhase::Success, Some(span(base, 0, 300))),
            block_at("b", 3, ToolPhase::Success, Some(span(base, 300, 200))),
            block_at("c", 5, ToolPhase::Success, Some(span(base, 500, 500))),
        ];
        let run = &runs(&blocks)[0];
        assert_eq!(total_span(&blocks, run), Some(Duration::from_millis(1000)));
    }

    #[test]
    fn a_partial_overlap_counts_the_overlap_once() {
        let base = Instant::now();
        let blocks = vec![
            block_at("a", 1, ToolPhase::Success, Some(span(base, 0, 600))),
            block_at("b", 3, ToolPhase::Success, Some(span(base, 400, 600))),
        ];
        let run = &runs(&blocks)[0];
        // 0..1000, not 600+600=1200.
        assert_eq!(total_span(&blocks, run), Some(Duration::from_millis(1000)));
    }

    #[test]
    fn interleaved_spans_merge_into_their_covering_interval() {
        let base = Instant::now();
        let blocks = vec![
            block_at("a", 1, ToolPhase::Success, Some(span(base, 0, 300))),
            block_at("b", 3, ToolPhase::Success, Some(span(base, 100, 800))),
            block_at("c", 5, ToolPhase::Success, Some(span(base, 200, 300))),
        ];
        let run = &runs(&blocks)[0];
        // The middle span covers the other two: 900ms, not 1400.
        assert_eq!(total_span(&blocks, run), Some(Duration::from_millis(900)));
    }

    #[test]
    fn no_timed_member_reports_no_total() {
        let blocks = vec![
            block("a", 1, ToolPhase::Success),
            block("b", 3, ToolPhase::Success),
        ];
        let run = &runs(&blocks)[0];
        assert_eq!(
            total_span(&blocks, run),
            None,
            "a run with no clock must report nothing rather than zero"
        );
    }

    #[test]
    fn untimed_members_do_not_shrink_a_measured_total() {
        let base = Instant::now();
        let blocks = vec![
            block_at("a", 1, ToolPhase::Success, Some(span(base, 0, 500))),
            block("b", 3, ToolPhase::Success),
        ];
        let run = &runs(&blocks)[0];
        assert_eq!(total_span(&blocks, run), Some(Duration::from_millis(500)));
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
