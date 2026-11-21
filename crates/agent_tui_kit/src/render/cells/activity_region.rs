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
    render::renderable::Renderable,
    state::activity_region::{self, ActivityRegion},
    theme::Theme,
    widgets::tool_widget::{ToolPhase, format_duration_us},
};

/// A mixed activity region drawn as one summary row.
pub struct ActivityRegionCell {
    /// How many thinking cards the region holds.
    thoughts: usize,
    /// How many narration runs the region swallowed. Read by `summary_line`.
    notes: usize,
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
    error: ratatui::style::Color,
    muted: ratatui::style::Color,
    /// The summary's colour: the error hue when a member failed, the accent
    /// otherwise. Stored so the fold gutter's `⏵` matches the text it
    /// introduces.
    summary_color: ratatui::style::Color,
    /// The region's stable identity, so a click can name what it toggles.
    identity: String,
    /// The locale the summary line is composed with (`Messages` is `Copy`).
    msgs: Messages,
    /// Columns the region's **first member's row** is indented by.
    ///
    /// The summary stands in for that row, so it has to be drawn at that row's
    /// indent or the whole line shifts sideways when the fold flips. It is not a
    /// constant: a region headed by a thinking card is indented 2, one headed by
    /// a tool block 8, one headed by narration 3.
    indent: u16,
}

impl ActivityRegionCell {
    /// Build the cell for `region`.
    ///
    /// `titles` are the tool members' phases and title rows, in the same order
    /// as the region's tool members; `thoughts` is the card count;
    /// `elapsed` is the region's wall clock
    /// ([`activity_region::total_span`]).
    ///
    /// `thoughts` and `notes` are the two non-tool halves of the summary; the
    /// tool half comes from `region` plus `titles`. The argument count is one
    /// over the lint's limit and bundling them would only hide the same count.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        region: &ActivityRegion,
        thoughts: usize,
        notes: usize,
        titles: Vec<(ToolPhase, String)>,
        elapsed: Option<std::time::Duration>,
        folded: bool,
        theme: &Theme,
        msgs: &Messages,
    ) -> Self {
        // A failed member keeps the error hue, so the fold can never make an
        // error quieter than the calls it hides.
        let summary_color = if titles.iter().any(|(phase, _)| phase.is_failure()) {
            theme.error
        } else {
            theme.accent
        };
        Self {
            thoughts,
            notes,
            members: titles,
            expanded_rows: region.expanded_rows,
            folded_rows: region.folded_rows,
            folded,
            elapsed,
            fg: theme.fg,
            bg: theme.bg,
            error: theme.error,
            muted: theme.muted,
            summary_color,
            identity: region.key.clone(),
            msgs: *msgs,
            indent: region
                .members
                .first()
                .map(|member| match member.kind {
                    activity_region::RegionMemberKind::Thinking => {
                        crate::render::util::LOG_THINKING_INDENT
                    }
                    activity_region::RegionMemberKind::Tool => {
                        crate::render::util::LOG_TOOL_BLOCK_INDENT
                    }
                    // Narration is assistant prose: one column past a card.
                    activity_region::RegionMemberKind::Commentary => {
                        crate::render::util::LOG_THINKING_INDENT + 1
                    }
                })
                .unwrap_or(crate::render::util::LOG_THINKING_INDENT),
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
        let summary = activity_region::summarize_with_commentary(
            self.thoughts,
            self.notes,
            &titles,
            self.elapsed,
            &self.msgs,
        );
        // A failure keeps its colour after folding: `⚠` plus the error hue is
        // what tells the reader one of these calls went wrong without making
        // them count the labels.
        // No glyph in the text: the row's fold gutter paints it, two columns
        // left of the text (`FOLD_GUTTER_COLS`, via `fold_glyph_x`). The text
        // itself never moves between the folded and open states.
        let color = self.summary_color;
        let mut spans = vec![Span::styled(
            summary.text,
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        )];
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
        let area = crate::render::util::indent_rect(area, self.indent);
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

        // Filling the tail is a *second* pass on purpose: `Paragraph` styles
        // the cells its text reaches, and the summary is usually shorter than
        // the row. Painting first does not help — the paragraph's own cells
        // overwrite it and the rest keeps the old frame's style. This is the
        // same reason `render_live_stats_band` repaints its row's background.
        // It runs from the gutter (where the fold glyph sits), not the text
        // area, so the indent columns are covered too.
        // `set_style` intersects with the buffer, so a row that ends at the
        // panel edge (where `area.x + area.width` is one past the last column)
        // cannot index out of bounds. Colouring cell by cell by hand did.
        let painted = Rect::new(
            crate::render::util::fold_glyph_x(area.x),
            area.y,
            area.width + self.indent,
            1,
        );
        buf.set_style(painted, Style::default().fg(self.fg).bg(self.bg));
        // The glyph and the summary text carry their own foreground; only the
        // background is being asserted here, so re-apply it without touching
        // any cell that already has ink of its own.
        for x in painted.left()..painted.right().min(buf.area.right()) {
            if x < buf.area.left() {
                continue;
            }
            let cell = &mut buf[(x, painted.y)];
            cell.bg = self.bg;
        }
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

    fn render_terminal(cell: &ActivityRegionCell) -> Terminal<TestBackend> {
        let backend = TestBackend::new(80, cell.height(80));
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| cell.render(frame.area(), frame.buffer_mut()))
            .expect("draw");
        terminal
    }

    fn render_text(cell: &ActivityRegionCell) -> String {
        buffer_text(render_terminal(cell).backend().buffer())
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
            (ToolPhase::Success, "1. 📖 Read  src/a.rs".to_string()),
            (ToolPhase::Success, "2. 📖 Read  src/b.rs".to_string()),
        ];
        ActivityRegionCell::new(
            &region,
            1,
            0,
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

    /// Render invariant (AGENTS.md): a TUI cell paints its **full** area.
    ///
    /// The folded row is one row wide and the summary is usually shorter than
    /// the panel. `Paragraph` only styles the cells its text reaches, so the
    /// row repaints its own background afterwards — without that step the
    /// indent column and the column just past the last glyph keep the previous
    /// frame's style, which reads as a band punched through the row.
    ///
    /// A wide glyph's **continuation** cell is skipped: `Buffer` writes its
    /// style on the leading cell only, so the second column of `📖` reads back
    /// `Reset` even on a fully painted row. Counting it would make this
    /// assertion fail on correct code.
    #[test]
    fn a_folded_region_paints_its_whole_row_with_the_theme_background() {
        let cell = mixed(true);
        let theme = Theme::from(crate::theme::ThemeName::Dark);
        let terminal = render_terminal(&cell);
        let buffer = terminal.backend().buffer();

        assert_eq!(buffer.area.height, 1, "a folded region is one row");

        let holes: Vec<u16> = (0..buffer.area.width)
            .filter(|&x| {
                // Skip the trailing column of a double-width glyph.
                let after_wide = x > 0
                    && buffer[(x - 1, 0)]
                        .symbol()
                        .chars()
                        .any(|c| unicode_width::UnicodeWidthChar::width(c) == Some(2));
                !after_wide && buffer[(x, 0)].bg == ratatui::style::Color::Reset
            })
            .collect();
        assert!(
            holes.is_empty(),
            "the folded row leaves holes at {holes:?}; its background must run \
             from the fold gutter to the last column: {}",
            buffer_text(buffer)
        );

        for x in 0..buffer.area.width {
            if buffer[(x, 0)].bg != ratatui::style::Color::Reset {
                assert_eq!(
                    buffer[(x, 0)].bg,
                    theme.bg,
                    "folded row cell at x={x} must carry theme.bg"
                );
            }
        }
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
            0,
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
