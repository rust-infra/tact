//! One folded run of adjacent tool blocks.
//!
//! A run of finished, adjacent tools collapses to a single summary row, the way
//! the desktop transcript's `ToolGroupRow` folds its members. Expanding the run
//! reports the members' own heights, so each member still draws through its own
//! [`ToolCell`] — this cell only decides *how many rows the run occupies* and
//! paints the summary row that stands in for them.
//!
//! The cell never mutates the log. A run is a view over blocks that already own
//! placeholder rows; folding changes what is painted at those rows, so nothing
//! has to be drained, re-indexed, or re-anchored when the reader expands or
//! folds again.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Widget as _},
};

use crate::{
    i18n::Messages,
    render::{renderable::Renderable, util::LOG_TOOL_BLOCK_INDENT},
    state::tool_group::{self, ToolRun},
    theme::Theme,
    widgets::tool_widget::format_duration_us,
};

/// A run of adjacent tool blocks rendered as one summary row, or as its
/// members when expanded.
pub struct ToolRunCell {
    /// Each member's phase and title, in display order.
    members: Vec<(crate::widgets::tool_widget::ToolPhase, String)>,
    /// Rows the run occupies when every member draws itself.
    expanded_rows: usize,
    folded: bool,
    /// Rows the summary itself occupies (always 1 when folded).
    folded_rows: usize,
    /// Wall clock of the whole run, as [`tool_group::total_span`] measured it.
    /// `None` when no member carried a clock — a fabricated zero would read as
    /// "instantaneous".
    elapsed: Option<std::time::Duration>,
    fg: ratatui::style::Color,
    bg: ratatui::style::Color,
    error: ratatui::style::Color,
    muted: ratatui::style::Color,
    /// The summary's colour: the error hue when a member failed, the accent
    /// otherwise. Stored rather than derived at paint time because the fold
    /// gutter's `⏵` has to be the same colour as the text it introduces.
    summary_color: ratatui::style::Color,
    /// The run's stable identity, so a click on the summary row can name what
    /// it is toggling.
    identity: String,
    /// The locale the summary line is composed with.
    ///
    /// `Messages` is `Copy` (every field is `&'static str`), so the cell owns
    /// its copy rather than borrowing the frame's — the cell outlives the
    /// borrow that built it.
    msgs: Messages,
}

/// Whether any member ended badly, mirrored from the summary's own count so the
/// gutter colour and the text colour cannot disagree.
fn members_have_failure(members: &[(crate::widgets::tool_widget::ToolPhase, String)]) -> bool {
    members.iter().any(|(phase, _)| phase.is_failure())
}

impl ToolRunCell {
    /// Build the cell for `run`, whose members are already rendered.
    ///
    /// `members` are the members' phases and title rows: the phases decide
    /// whether the summary reports failures, the titles say *what* ran.
    /// `elapsed` is the run's wall clock. `folded` comes from the run's current
    /// expansion state.
    pub fn new(
        run: &ToolRun,
        members: Vec<(crate::widgets::tool_widget::ToolPhase, String)>,
        elapsed: Option<std::time::Duration>,
        folded: bool,
        theme: &Theme,
        msgs: &Messages,
    ) -> Self {
        let summary_color = if members_have_failure(&members) {
            theme.error
        } else {
            theme.accent
        };
        Self {
            members,
            expanded_rows: run.expanded_rows,
            folded,
            folded_rows: run.folded_rows,
            elapsed,
            fg: theme.fg,
            bg: theme.bg,
            error: theme.error,
            muted: theme.muted,
            summary_color,
            identity: String::new(),
            msgs: *msgs,
        }
    }

    /// Attach the run identity the click path reports back.
    pub fn with_identity(mut self, identity: impl Into<String>) -> Self {
        self.identity = identity.into();
        self
    }

    /// The run this summary stands for.
    pub fn identity(&self) -> &str {
        &self.identity
    }

    /// The folded summary line: an expander glyph, the count, then the tools.
    ///
    /// Each member's own title is reduced to its tool label and counted, so
    /// three reads of three paths read as "Read ×3" — the paths are what the
    /// reader expands the run to see.
    fn summary_line(&self) -> Line<'static> {
        let members: Vec<(crate::widgets::tool_widget::ToolPhase, &str)> = self
            .members
            .iter()
            .map(|(phase, title)| (*phase, title.as_str()))
            .collect();
        let summary = tool_group::summarize(&members, self.elapsed, &self.msgs);
        // A failure keeps its color after folding: `⚠` plus the error hue is
        // what tells the reader that one of these calls went wrong, without
        // making them count the tool labels.
        // No glyph in the text: the row's fold gutter paints it, so the
        // summary text starts at [`FOLD_TEXT_OFFSET`] in both fold states.
        let color = self.summary_color;
        // The total comes last, after the tool labels: it answers "how long did
        // this cost me", which a reader asks after "what ran".
        let mut spans = vec![Span::styled(
            summary.text,
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        )];
        if let Some(elapsed) = self.elapsed {
            spans.push(Span::styled(
                format!(
                    " · {}",
                    format_duration_us(elapsed.as_micros().try_into().unwrap_or(u64::MAX))
                ),
                Style::default().fg(self.muted),
            ));
        }
        Line::from(spans)
    }
}

impl Renderable for ToolRunCell {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        self.render_partial(area, buf, 0);
    }

    fn height(&self, _width: u16) -> u16 {
        if self.folded {
            self.folded_rows as u16
        } else {
            self.expanded_rows as u16
        }
    }

    fn render_partial(&self, area: Rect, buf: &mut Buffer, skip_lines: usize) {
        let area = crate::render::util::indent_rect(area, LOG_TOOL_BLOCK_INDENT);
        if area.height == 0 || area.width == 0 {
            return;
        }
        if !self.folded {
            // An expanded run draws through its members' own cells; this cell
            // only reserved the rows, so there is nothing to paint.
            return;
        }
        if skip_lines > 0 {
            return;
        }
        let line = self.summary_line();
        // The glyph goes in the gutter two columns left of the text, and the text
        // itself does not move: that is exactly where an expanded head draws its
        // `⏷`, so flipping the fold leaves the control under the pointer.
        // `area` is already indented by `self.indent`, so stepping
        // `FOLD_GUTTER_COLS` back lands in the gutter left of the text.
        let x = area.x.saturating_sub(crate::render::util::FOLD_GUTTER_COLS);
        let width = crate::render::util::FOLD_GUTTER_COLS.min(area.width);
        if width > 0 {
            // A failure keeps its `⚠`: the glyph is what tells the reader one of
            // these calls went wrong without making them count the labels, and
            // it has to survive the fold. The expander yields to it.
            let glyph = if self.summary_color == self.error {
                "⚠"
            } else {
                "⏵"
            };
            buf.set_string(
                x,
                area.y,
                glyph,
                Style::default().fg(self.summary_color).bg(self.bg),
            );
        }
        Paragraph::new(vec![line])
            .style(Style::default().fg(self.fg).bg(self.bg))
            .render(Rect::new(area.x, area.y, area.width, 1), buf);
    }
}
