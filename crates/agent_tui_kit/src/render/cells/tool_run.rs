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
    accent: ratatui::style::Color,
    error: ratatui::style::Color,
    muted: ratatui::style::Color,
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
        _msgs: &Messages,
    ) -> Self {
        Self {
            members,
            expanded_rows: run.expanded_rows,
            folded,
            folded_rows: run.folded_rows,
            elapsed,
            fg: theme.fg,
            bg: theme.bg,
            accent: theme.accent,
            error: theme.error,
            muted: theme.muted,
        }
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
        let summary = tool_group::summarize(&members, self.elapsed);
        // A failure keeps its color after folding: `⚠` plus the error hue is
        // what tells the reader that one of these calls went wrong, without
        // making them count the tool labels.
        let (glyph, color) = if summary.failures > 0 {
            ("⚠ ", self.error)
        } else {
            ("⏵ ", self.accent)
        };
        // The total comes last, after the tool labels: it answers "how long did
        // this cost me", which a reader asks after "what ran".
        let mut spans = vec![
            Span::styled(glyph, Style::default().fg(color)),
            Span::styled(
                summary.text,
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ),
        ];
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
        Paragraph::new(vec![self.summary_line()])
            .style(Style::default().fg(self.fg).bg(self.bg))
            .render(Rect::new(area.x, area.y, area.width, 1), buf);
    }
}
