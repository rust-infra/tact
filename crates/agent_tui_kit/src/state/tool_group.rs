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
    let spans: Vec<ToolSpan> = run
        .members
        .iter()
        .filter_map(|&index| blocks.get(index)?.span)
        .collect();
    super::tool_state::unioned_span(&spans)
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
/// The titles are per-call (`"3. 📖 Read  src/app.rs"`), so the summary first
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
pub fn summarize(
    members: &[(ToolPhase, &str)],
    elapsed: Option<Duration>,
    msgs: &crate::i18n::Messages,
) -> FoldSummary {
    let failures = members
        .iter()
        .filter(|(phase, _)| phase.is_failure())
        .count();
    FoldSummary {
        text: tools_summary(members, failures, msgs),
        elapsed,
        failures,
    }
}

/// `"5 tools · 2 failed · Read ×3 · Bash"` — the tool half of a fold line.
///
/// Exposed so an activity region ([`super::activity_region`]) can put its own
/// thought count in front of the same labels instead of growing a second copy
/// of the counting rule.
pub fn tools_summary(
    titles: &[(ToolPhase, &str)],
    failures: usize,
    msgs: &crate::i18n::Messages,
) -> String {
    let titles = titles.iter().map(|(_, title)| *title).collect::<Vec<_>>();
    let mut labels: Vec<String> = Vec::with_capacity(titles.len());
    let mut counts: Vec<usize> = Vec::with_capacity(titles.len());
    for title in &titles {
        let label = tool_label(title);
        match labels.iter().position(|known| known == &label) {
            Some(index) => counts[index] += 1,
            None => {
                labels.push(label);
                counts.push(1);
            }
        }
    }
    let count_tmpl = if titles.len() == 1 {
        msgs.fold_tools_one
    } else {
        msgs.fold_tools
    };
    let mut text = count_tmpl.replacen("{}", &titles.len().to_string(), 1);
    if failures > 0 {
        text.push_str(msgs.fold_sep);
        text.push_str(&msgs.fold_failed.replacen("{}", &failures.to_string(), 1));
    }
    for (label, count) in labels.iter().zip(&counts) {
        text.push_str(msgs.fold_sep);
        if *count > 1 {
            // Both the label and the count live in the template: `×` is a
            // glyph, but which side of the label it sits on is a language
            // decision.
            let marked =
                msgs.fold_repeat
                    .replacen("{}", label, 1)
                    .replacen("{}", &count.to_string(), 1);
            text.push_str(&marked);
        } else {
            text.push_str(label);
        }
    }
    text
}

/// The tool label inside a title row, with the step ordinal and the argument
/// removed.
///
/// [`ToolWidget::title_text`] builds the title as `"<label>  <argument>"`,
/// optionally prefixed with `"<n>. "`. The label itself is a display name such
/// as `"📖 Read"` or `"$ Bash"` and never contains a double space, so the *first*
/// double-space run is the separator. Splitting at the last one stripped the
/// wrong thing whenever the argument contained one — `bash` passes its command
/// through here, and `echo "a  b"` reduced the label to `"$ Bash echo 'a"`.
///
/// Two shapes reach here, because [`ToolWidget::title_text`] has two:
///
/// - `<label>  <argument>` — the `Command`/file/`_` arms separate with a double
///   space.
/// - `<label> · <argument>` — the `Subagent`, `Sleep` and `Task` arms separate
///   with a middle dot. The `Task` arm is the loud one: it passes the whole
///   `# Task · create · subject: …` summary through, so a label split that only
///   knew about double spaces produced a folded row reading
///   `# Task · create · subject: …` four times over.
///
/// The double-space rule stays *first*: a `Task` subject can itself contain
/// `·`, and the dot rule must not fire before the separator the shape actually
/// uses. Both fall back to the whole string when there is no separator, so a
/// title without an argument never prints an empty label.
fn tool_label(title: &str) -> String {
    let without_ordinal = match title.split_once(". ") {
        Some((ordinal, rest)) if ordinal.chars().all(|c| c.is_ascii_digit()) => rest,
        _ => title,
    };
    if let Some((label, _argument)) = without_ordinal.split_once("  ")
        && !label.trim().is_empty()
    {
        return normalize_spaces(label);
    }
    // The middle-dot shape. Split at the *first* ` · `: everything after it is
    // the argument, and a label never contains one.
    if let Some((label, _argument)) = without_ordinal.split_once(" · ")
        && !label.trim().is_empty()
    {
        return normalize_spaces(label);
    }
    normalize_spaces(without_ordinal)
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
    ///
    /// `covers_run` answers "does a mixed activity region stand for this run?".
    /// A run inside a region folds with that region — an independent summary
    /// would paint a second row for a stretch the region already stands for,
    /// and expanding one would leave the other's summary lying across its
    /// members.
    pub fn fold_summary(
        &self,
        phys_idx: usize,
        covers_run: impl Fn(usize) -> bool,
        theme: &crate::theme::Theme,
        msgs: &crate::i18n::Messages,
    ) -> Option<crate::render::cells::tool_run::ToolRunCell> {
        if !self.blocks.iter().any(|block| block.phys_idx == phys_idx) {
            return None;
        }
        let found = runs(&self.blocks);
        let run = found.into_iter().find(|run| {
            run.phys_idx == phys_idx
                && is_folded(&self.blocks, run, self)
                && !covers_run(run.phys_idx)
        })?;
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

    /// The identity of the run whose **first** member sits at `phys_idx`, when
    /// that run is drawn expanded right now.
    ///
    /// Expanding a run replaces its one summary row with its members, so the
    /// row that carried the collapse affordance disappears with it. The first
    /// member's title row takes that position back — it is where the summary
    /// was — so the same click folds the run in both directions.
    ///
    /// `None` for a folded run (its summary row already carries the target) and
    /// for any row that is not a run head.
    pub fn expanded_run_head_at(&self, phys_idx: usize) -> Option<String> {
        let found = runs(&self.blocks);
        let index = run_at_phys(&found, phys_idx)?;
        let run = &found[index];
        if run.phys_idx != phys_idx || is_folded(&self.blocks, run, self) {
            return None;
        }
        run_key(&self.blocks, run).map(str::to_string)
    }

    // ── Activity regions (thinking cards + tool blocks) ──────────────
    //
    // A run of adjacent tool blocks folds through `runs` above. A run that also
    // holds a finished thinking card folds through `activity_region`, because
    // the row it stands for has to count both halves. The two are kept apart on
    // purpose: a tool-only run's summary names each tool, which a region's
    // `N tools` could not.

    /// Every activity region in the log, given its finished thinking cards and
    /// the log's narration runs.
    ///
    /// This is the **only** way in: a commentary-less variant existed and was
    /// deleted, because two ways to derive the regions is exactly how the click
    /// path and the renderer came to disagree about which region a row belongs
    /// to. `thinking` is `ThinkingState::blocks`; the live card is not a
    /// member. `narration` empty reproduces the pre-`fold_commentary` shape.
    ///
    /// The host passes `narration` when `[ui] fold_commentary` is on; an empty slice
    /// gives the pre-switch shape, where narration kept its own rows.
    pub fn activity_regions(
        &self,
        thinking: &crate::state::thinking::ThinkingState,
        commentary: &[crate::state::activity_region::CommentaryRun],
        is_turn_boundary: impl Fn(usize) -> bool,
    ) -> Vec<crate::state::activity_region::ActivityRegion> {
        crate::state::activity_region::regions_with_commentary(
            &thinking.blocks,
            &self.blocks,
            commentary,
            is_turn_boundary,
        )
    }

    /// Whether an activity region can start at `phys_idx`.
    ///
    /// The renderer asks this for every visible row before deriving the whole
    /// region set, so the cheap "nothing starts here" case stays cheap.
    fn region_may_start_at(
        &self,
        thinking: &crate::state::thinking::ThinkingState,
        commentary: &[crate::state::activity_region::CommentaryRun],
        phys_idx: usize,
    ) -> bool {
        self.blocks.iter().any(|block| block.phys_idx == phys_idx)
            || thinking
                .blocks
                .iter()
                .any(|block| block.phys_idx == phys_idx)
            || commentary.iter().any(|run| run.phys_idx == phys_idx)
    }

    /// The folded summary cell for a **mixed** activity region whose first
    /// member sits at `phys_idx`, if that region is folded right now.
    ///
    /// The region twin of [`Self::fold_summary`], with the same cheap-first
    /// shape: the common case is "no region starts here", asked for every
    /// logical row on every frame.
    ///
    /// `is_turn_boundary` comes from the host's log: a region must never span
    /// two turns, and only the host can say where a turn ended.
    pub fn region_fold_summary(
        &self,
        thinking: &crate::state::thinking::ThinkingState,
        commentary: &[crate::state::activity_region::CommentaryRun],
        phys_idx: usize,
        is_turn_boundary: impl Fn(usize) -> bool,
        theme: &crate::theme::Theme,
        msgs: &crate::i18n::Messages,
    ) -> Option<crate::render::cells::activity_region::ActivityRegionCell> {
        // A region can start at a narration row, which is in neither `blocks`
        // slice — so the cheap "nothing starts here" check has to include the
        // commentary runs too, or a narration-headed region never gets drawn.
        if !self.region_may_start_at(thinking, commentary, phys_idx) {
            return None;
        }
        let found = self.activity_regions(thinking, commentary, is_turn_boundary);
        let index = crate::state::activity_region::region_at_phys(&found, phys_idx)?;
        let region = &found[index];
        if region.phys_idx != phys_idx
            || !crate::state::activity_region::is_folded(
                &self.blocks,
                region,
                &self.folded_regions,
                &self.unfolded_regions,
            )
        {
            return None;
        }
        let titles: Vec<(crate::widgets::tool_widget::ToolPhase, String)> = region
            .members
            .iter()
            .filter(|member| member.kind == crate::state::activity_region::RegionMemberKind::Tool)
            .filter_map(|member| self.blocks.get(member.index))
            .map(|block| (block.output.phase, block.output.title_raw.clone()))
            .collect();
        let elapsed =
            crate::state::activity_region::total_span(&thinking.blocks, &self.blocks, region);
        Some(
            crate::render::cells::activity_region::ActivityRegionCell::new(
                region,
                region.thoughts(),
                region.commentary(),
                titles,
                elapsed,
                true,
                theme,
                msgs,
            ),
        )
    }

    /// Flip one region, recording the reader's choice in the set that differs
    /// from the state they are leaving.
    ///
    /// The default (a mixed region folds) never has to be stored: opening a
    /// region lands it in `unfolded_regions`, and folding it back drops out of
    /// that set. `folded_regions` is the mirror case.
    pub fn toggle_region(
        &mut self,
        region: &crate::state::activity_region::ActivityRegion,
    ) -> bool {
        let key = region.key.clone();
        let now_folded = crate::state::activity_region::is_folded(
            &self.blocks,
            region,
            &self.folded_regions,
            &self.unfolded_regions,
        );
        if now_folded {
            self.folded_regions.remove(&key);
            self.unfolded_regions.insert(key);
        } else {
            self.unfolded_regions.remove(&key);
            self.folded_regions.insert(key);
        }
        self.fold_rev = self.fold_rev.wrapping_add(1);
        true
    }

    /// The identity of the region whose **first** member sits at `phys_idx`,
    /// when that region is drawn expanded right now.
    ///
    /// The mirror of [`Self::expanded_run_head_at`] for a region: expanding
    /// replaces the summary row with the members, so the first member's own row
    /// takes the control back.
    ///
    /// `commentary` must be the same runs the fold was built from. Deriving the
    /// regions without them yields a *different* set — a narration-headed region
    /// only exists when the runs are passed — and the control would be missing
    /// exactly where the fold drew a summary for it.
    pub fn expanded_region_head_at(
        &self,
        thinking: &crate::state::thinking::ThinkingState,
        commentary: &[crate::state::activity_region::CommentaryRun],
        is_turn_boundary: impl Fn(usize) -> bool,
        phys_idx: usize,
    ) -> Option<String> {
        if !self.region_may_start_at(thinking, commentary, phys_idx) {
            return None;
        }
        let found = self.activity_regions(thinking, commentary, is_turn_boundary);
        let index = crate::state::activity_region::region_at_phys(&found, phys_idx)?;
        let region = &found[index];
        // A tool-only region is not a region here: the run below owns that row,
        // and handing its key back would leave the run with no control at all.
        // `folds_here` is the region/run split the fold itself uses — not
        // `mixes_thinking_and_tools`, which misses a narration-headed region and
        // would leave it open with no way to close it.
        if !region.folds_here()
            || region.phys_idx != phys_idx
            || crate::state::activity_region::is_folded(
                &self.blocks,
                region,
                &self.folded_regions,
                &self.unfolded_regions,
            )
        {
            return None;
        }
        Some(region.key.clone())
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
            (ToolPhase::Success, "1. 📖 Read  src/main.rs"),
            (ToolPhase::Success, "2. 📖 Read  src/lib.rs"),
            (ToolPhase::Success, "3. 📖 Read  src/app.rs"),
            (ToolPhase::Success, "4. $ Bash  cargo test"),
            (ToolPhase::Success, "5. Grep  fn main"),
        ];
        let summary = summarize(&members, None, &msgs());
        assert_eq!(
            summary.text, "5 tools · 📖 Read ×3 · $ Bash · Grep",
            "repeated tools must collapse to one counted label"
        );
        assert_eq!(summary.failures, 0);
    }

    #[test]
    fn a_failure_is_counted_in_the_summary_not_hidden_by_it() {
        let members = [
            (ToolPhase::Success, "1. 📖 Read  a.rs"),
            (ToolPhase::Failed, "2. $ Bash  cargo test"),
            (ToolPhase::Cancelled, "3. $ Bash  cargo build"),
        ];
        let summary = summarize(&members, None, &msgs());
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
            summarize(&[(ToolPhase::Success, "Bash")], None, &msgs()).text,
            "1 tool · Bash",
            "one call takes the singular form"
        );
        assert_eq!(
            summarize(&[(ToolPhase::Success, "1. Bash")], None, &msgs()).text,
            "1 tool · Bash"
        );
    }

    #[test]
    fn the_tool_half_of_a_fold_line_is_reusable() {
        let members = [
            (ToolPhase::Success, "1. 📖 Read  a.rs"),
            (ToolPhase::Failed, "2. $ Bash  false"),
        ];
        assert_eq!(
            tools_summary(&members, 1, &msgs()),
            "2 tools · 1 failed · 📖 Read · $ Bash"
        );
    }

    /// A command whose own text contains a double space must not leak into the
    /// label.
    ///
    /// The label is separated from the argument by the *first* double space. When
    /// this split at the last one instead, `bash` — which passes its whole
    /// command as the argument — produced a label like `"$ Bash echo 'a"` and the
    /// folded summary printed the command instead of the tool.
    #[test]
    fn an_argument_with_a_double_space_does_not_become_the_label() {
        assert_eq!(tool_label("$ Bash  echo 'a  b' && ls"), "$ Bash");
        assert_eq!(tool_label("1. $ Bash  echo 'a  b' && ls"), "$ Bash");
        assert_eq!(
            tool_label("$ Bash  cd /repo && echo '=== x ===' && grep -n \"pub fn \\\" | head"),
            "$ Bash"
        );
        // The label itself keeps its own single space.
        assert_eq!(tool_label("📖 Read  src/main.rs"), "📖 Read");
    }

    /// The fixtures above are hand-written; this one derives the title the way
    /// the widget does, so a change to the separator cannot pass unnoticed.
    #[test]
    fn a_summary_reads_labels_from_real_widget_titles() {
        use crate::widgets::tool_widget::{ToolPhase as WidgetPhase, ToolWidget};

        let read = ToolWidget::new()
            .with_tool("read_file")
            .with_arg_summary("src/main.rs")
            .with_phase(WidgetPhase::Success)
            .build();
        let bash = ToolWidget::new()
            .with_tool("bash")
            .with_arg_summary("echo 'a  b' && ls")
            .with_phase(WidgetPhase::Success)
            .build();

        let members = [
            (ToolPhase::Success, read.title_raw.as_str()),
            (ToolPhase::Success, bash.title_raw.as_str()),
        ];
        assert_eq!(
            tools_summary(&members, 0, &msgs()),
            "2 tools · 📖 Read · $ Bash",
            "a real bash title must fold to `$ Bash`, not to its command"
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

    /// Four `Task` calls fold into `# Task ×4`, not four long subjects.
    ///
    /// This is the reported symptom end to end: the screenshot's summary row
    /// read `# Task · create · subject: … · # Task · create · subject: …` and
    /// overflowed the panel. The label is what a folded row is *for* — one
    /// glance at what ran — so it must be the tool, never its argument.
    #[test]
    fn task_calls_collapse_into_one_label_instead_of_four_subjects() {
        let msgs = crate::i18n::Messages::by_language(crate::i18n::Language::Chinese);
        let titles = [
            (
                ToolPhase::Success,
                "# Task · create · subject: S5: 示例值与默认值一致",
            ),
            (
                ToolPhase::Success,
                "# Task · create · subject: Std1: book/21 补 fold_commentary",
            ),
            (
                ToolPhase::Success,
                "# Task · create · subject: S1: 固定行只在当前回合",
            ),
        ];
        let summary = tools_summary(&titles, 0, &msgs);

        assert!(
            summary.contains("# Task"),
            "the label must survive the fold: {summary}"
        );
        assert_eq!(
            summary.matches("# Task").count(),
            1,
            "the three calls share one label and a count, not one label each: {summary}"
        );
        assert!(
            !summary.contains("subject:"),
            "a folded row must not carry the argument: {summary}"
        );
    }

    /// The reported bug: a `Task` tool's fold label swallowed the whole row.
    #[test]
    fn a_task_title_that_carries_no_double_space_still_yields_its_label() {
        // `title_text`'s Task arm returns `arg_summary` verbatim, and that
        // summary is `# Task · create · subject: …` — `·`-joined, no double
        // space. Splitting on a double space finds nothing, so the label became
        // the entire string and the folded summary printed four task subjects.
        assert_eq!(
            tool_label("# Task · create · subject: S5: config.example.toml"),
            "# Task"
        );
        assert_eq!(tool_label("📋 Task · update · subject: fix it"), "📋 Task");
        // The shape `ToolWidget` actually emits for a task: the id sits inside
        // the label, so the label keeps it.
        assert_eq!(tool_label("# Task.1 · fix the popup"), "# Task.1");
        assert_eq!(tool_label("1. # Task.7 · ship it"), "# Task.7");
    }

    /// Every `ToolVisualKind`'s title shape must reduce to its label.
    ///
    /// The `Task` bug was one instance of a general hazard: `title_text` has
    /// **two** separators (double space, middle dot) and three kind-specific
    /// arms that bypass the label prefix entirely. A kind whose separator
    /// `tool_label` does not know folds into a row carrying its whole argument.
    /// This walks every kind the formatter can produce, so the next arm added
    /// to `title_text` has to answer this test.
    #[test]
    fn every_title_shape_reduces_to_its_label() {
        // (emitted title, expected label) — one row per `title_text` arm.
        let cases: &[(&str, &str)] = &[
            // Command / file / `_`: `<label>  <argument>`.
            ("$ Bash  cargo test", "$ Bash"),
            ("📖 Read  src/main.rs", "📖 Read"),
            ("✏️ Edit  src/lib.rs", "✏️ Edit"),
            // Subagent: `Subagent · <summary>`.
            ("Subagent · review the diff", "Subagent"),
            // Sleep: `<label> · <duration or id>`.
            ("💤 Sleep · 2.50s", "💤 Sleep"),
            ("⏳ Wait Background · bg-3", "⏳ Wait Background"),
            // Task: the argument *is* the human title.
            ("# Task.1 · fix the popup", "# Task.1"),
            // Ordinal prefix on any of the above.
            ("1. $ Bash  cargo test", "$ Bash"),
            ("2. 📖 Read  src/main.rs", "📖 Read"),
            ("3. Subagent · review", "Subagent"),
            ("4. # Task.7 · ship it", "# Task.7"),
        ];

        for (title, expected) in cases {
            assert_eq!(
                &tool_label(title),
                expected,
                "title {title:?} must fold down to its label"
            );
        }
    }

    /// A title with neither separator keeps whatever it has.
    ///
    /// The fallback matters for the subagent popup, which builds its own title
    /// with no argument at all.
    #[test]
    fn a_title_without_a_separator_is_its_own_label() {
        assert_eq!(tool_label("Subagent"), "Subagent");
        assert_eq!(tool_label("💤 Sleep"), "💤 Sleep");
        assert_eq!(tool_label("# Task.1"), "# Task.1");
    }

    /// End to end: a real `ToolWidget`'s own title, folded to its label.
    ///
    /// [`every_title_shape_reduces_to_its_label`] feeds `tool_label` strings I
    /// wrote by hand. That only proves the splitter agrees with my reading of
    /// `title_text`. This drives the widget itself, so a new arm (or a changed
    /// separator) fails here even if the hand-written cases still pass.
    #[test]
    fn a_real_widgets_title_folds_to_its_label() {
        use crate::widgets::tool_widget::ToolWidget;

        // (tool name, argument summary, expected label)
        let cases: &[(&str, &str, &str)] = &[
            ("bash", "cargo test", "$ Bash"),
            ("read_file", "src/main.rs", "📖 Read"),
            ("edit_file", "src/lib.rs", "✏️ Edit"),
            ("spawn_subagent", "review the diff", "Subagent"),
            ("sleep", "2500", "💤 Sleep"),
            ("task_create", "# Task.1 · fix the popup", "# Task.1"),
        ];

        for (tool, arg, expected) in cases {
            let widget = ToolWidget::new()
                .with_tool(*tool)
                .with_arg_summary(*arg)
                .with_phase(ToolPhase::Success);
            let title = widget.title_text();
            assert_eq!(
                tool_label(&title),
                *expected,
                "tool {tool}: title {title:?} must fold to {expected:?}"
            );
        }
    }
}
