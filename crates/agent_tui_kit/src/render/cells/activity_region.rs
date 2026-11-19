//! One folded activity region: thinking cards and tool calls on one row.
//!
//! The sibling of [`super::tool_run::ToolRunCell`], for a region that mixes a
//! finished thinking card with at least one tool block. A tool-only run keeps
//! its own cell: its summary names each tool, while a mixed region's line is
//! `2 thoughts · 3 tools · Read ×2 · Bash` and says "how much of this turn was
//! work" rather than "which calls ran".
//!
//! The cell never mutates the log. Folding only decides what is painted at the
//! rows the members already own; nothing is drained, re-indexed, or re-anchored
//! when the reader expands or folds again.

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
    state::activity_region::{self, ActivityRegion},
    theme::Theme,
    widgets::tool_widget::{ToolPhase, format_duration_us},
};

/// A mixed activity region drawn as one summary row.
pub struct ActivityRegionCell {
    /// How many thinking cards the region holds.
    thoughts: usize,
    /// Each tool member's phase and title, in display order.
    members: Vec<(ToolPhase, String)>,
    /// Rows the region occupies when every member draws itself.
    expanded_rows: usize,
    folded_rows: usize,
    folded: bool,
    /// Wall clock of the whole region, whenever anything in it was timed.
    elapsed: Option<std::time::Duration>,
    fg: ratatui::style::Color,
    bg: ratatui::style::Color,
    accent: ratatui::style::Color,
    error: ratatui::style::Color,
    muted: ratatui::style::Color,
    /// The region's stable identity, so a click can name what it toggles.
    identity: String,
    /// The locale the summary line is composed with (`Messages` is `Copy`).
    msgs: Messages,
}

impl ActivityRegionCell {
    /// Build the cell for `region`.
    ///
    /// `titles` are the tool members' phases and title rows, in the same order
    /// as the region's tool members; `thoughts` is the card count;
    /// `elapsed` is the region's wall clock
    /// ([`activity_region::total_span`]).
    pub fn new(
        region: &ActivityRegion,
        thoughts: usize,
        titles: Vec<(ToolPhase, String)>,
        elapsed: Option<std::time::Duration>,
        folded: bool,
        theme: &Theme,
        msgs: &Messages,
    ) -> Self {
        Self {
            thoughts,
            members: titles,
            expanded_rows: region.expanded_rows,
            folded_rows: region.folded_rows,
            folded,
            elapsed,
            fg: theme.fg,
            bg: theme.bg,
            accent: theme.accent,
            error: theme.error,
            muted: theme.muted,
            identity: region.key.clone(),
            msgs: *msgs,
        }
    }

    /// The region this summary stands for.
    pub fn identity(&self) -> &str {
        &self.identity
    }

    /// The summary line: an expander glyph, the thought count, then the calls.
    fn summary_line(&self) -> Line<'static> {
        let titles: Vec<(ToolPhase, &str)> = self
            .members
            .iter()
            .map(|(phase, title)| (*phase, title.as_str()))
            .collect();
        let summary = activity_region::summarize(self.thoughts, &titles, self.elapsed, &self.msgs);
        // A failure keeps its colour after folding: `⚠` plus the error hue is
        // what tells the reader one of these calls went wrong without making
        // them count the labels.
        let (glyph, color) = if summary.failures > 0 {
            ("⚠ ", self.error)
        } else {
            ("⏵ ", self.accent)
        };
        let mut spans = vec![
            Span::styled(glyph, Style::default().fg(color)),
            Span::styled(
                summary.text,
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ),
        ];
        // The total comes last, after the counts: a reader asks "how long did
        // this cost me" after "how much ran".
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

impl Renderable for ActivityRegionCell {
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
            // An expanded region draws through its members' own cells; this cell
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::cells::thinking::COMPLETED_THINKING_ROWS;
    use ratatui::{Terminal, backend::TestBackend};

    fn buffer_text(buf: &ratatui::buffer::Buffer) -> String {
        let mut out = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                out.push_str(buf[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    fn render_text(cell: &ActivityRegionCell) -> String {
        let backend = TestBackend::new(80, cell.height(80));
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| cell.render(frame.area(), frame.buffer_mut()))
            .expect("draw");
        buffer_text(terminal.backend().buffer())
    }

    fn mixed(folded: bool) -> ActivityRegionCell {
        let region = ActivityRegion {
            members: Vec::new(),
            phys_idx: 1,
            expanded_rows: COMPLETED_THINKING_ROWS + 4,
            folded_rows: 1,
            key: "t1".to_string(),
        };
        let titles = vec![
            (ToolPhase::Success, "1. 📖  Read  src/a.rs".to_string()),
            (ToolPhase::Success, "2. 📖  Read  src/b.rs".to_string()),
        ];
        ActivityRegionCell::new(
            &region,
            1,
            titles,
            Some(std::time::Duration::from_millis(1_200)),
            folded,
            &Theme::from(crate::theme::ThemeName::Dark),
            &Messages::by_language(crate::i18n::Language::English),
        )
    }

    #[test]
    fn a_folded_region_is_one_row_that_counts_both_halves() {
        let cell = mixed(true);
        assert_eq!(cell.height(80), 1);
        let text = render_text(&cell);
        assert!(text.contains("1 thought"), "{text}");
        assert!(text.contains("2 tools"), "{text}");
        assert!(text.contains("Read ×2"), "{text}");
        assert!(text.contains("1.20s"), "{text}");
    }

    #[test]
    fn an_expanded_region_reserves_its_members_rows_and_paints_nothing() {
        let cell = mixed(false);
        assert_eq!(cell.height(80), (COMPLETED_THINKING_ROWS + 4) as u16);
        let text = render_text(&cell);
        assert!(
            !text.contains("thought"),
            "the members paint, not this cell: {text}"
        );
    }

    #[test]
    fn a_failure_keeps_the_error_glyph_after_folding() {
        let region = ActivityRegion {
            members: Vec::new(),
            phys_idx: 1,
            expanded_rows: COMPLETED_THINKING_ROWS + 2,
            folded_rows: 1,
            key: "t1".to_string(),
        };
        let cell = ActivityRegionCell::new(
            &region,
            1,
            vec![(ToolPhase::Failed, "1. $ Bash  false".to_string())],
            None,
            true,
            &Theme::from(crate::theme::ThemeName::Dark),
            &Messages::by_language(crate::i18n::Language::English),
        );
        let text = render_text(&cell);
        assert!(text.contains('⚠'), "{text}");
        assert!(text.contains("1 failed"), "{text}");
    }
}
