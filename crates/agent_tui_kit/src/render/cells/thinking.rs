use std::time::Duration;

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::Style,
    text::{Line, Span},
};
use unicode_width::UnicodeWidthStr;

use crate::{
    i18n::Messages,
    render::{
        renderable::Renderable,
        util::{LOG_THINKING_INDENT, truncate_to_width},
    },
    state::{ActiveThinkingBlock, ThinkingBlock},
    theme::Theme,
    widgets::button::{Button, ButtonChrome, ButtonTheme},
};

/// The column the card's text starts in, one right of the cell's own indent.
///
/// That is where assistant prose lands (`LOG_THINKING_INDENT + 1`), so a
/// reasoning line and the reply under it start on the same vertical. The card
/// used to get this column from its left border; without the border the pad
/// carries it.
const CONTENT_PAD: u16 = 1;

/// [`CONTENT_PAD`] as the prose prefix, so the glyphs and the hit rect agree.
const PAD: &str = " ";

/// Rows a **live** thinking card occupies: a header, then one row per body line.
///
/// The live card passes the size of its reasoning tail, so it grows to four.
///
/// A finished card draws no body at all and takes [`COMPLETED_THINKING_ROWS`];
/// it does not go through this function, because a body of zero lines is not a
/// thing the clamp can express.
///
/// The card draws no border and no blank rows of its own. It used to spend five
/// rows on a one-line summary, and the other four were two borders and two blank
/// rows.
///
/// The header keeps a row of its own rather than joining the reasoning. One row
/// would have to hold the title and the readout at once, and the reasoning
/// would have nowhere to go.
pub fn thinking_visual_rows(body_lines: usize) -> usize {
    body_lines.clamp(1, 3) + 1
}

/// Rows a **finished** thinking card occupies: its header, and nothing else.
///
/// The reasoning is behind the `[Open]` button, so a summary row would only be
/// the last line of a paragraph the reader can open in full — one row of log per
/// finished thought is the whole budget the card is meant to spend.
pub const COMPLETED_THINKING_ROWS: usize = 1;

/// How the footer states the reasoning's size.
///
/// A live card shows a fraction because the rows on screen are real: `↕ 2/12
/// lines` says "two of the twelve are drawn". A finished card draws none, so a
/// fraction would be either a lie (`0/12`) or meaningless (`12/12`); it states
/// the total instead, the same way a collapsed tool card does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LineCount {
    Fraction { shown: usize, total: usize },
    Total(usize),
}

pub struct ThinkingCell {
    /// The reasoning rows — `Some` while the card streams, `None` once it is
    /// finished and draws no body at all.
    ///
    /// `Some(vec![])` is not the same state: a live card that has not received
    /// its first line yet still owns a body row (an empty one), and the
    /// placeholder rows in the log are counted for it. `None` is the finished
    /// card, whose only row is the header.
    body: Option<Vec<String>>,
    title: String,
    /// Footer text left of the button — see [`Self::footer`].
    bottom: String,
    /// The footer button's label, when the locale's template ends with it.
    bottom_action: Option<&'static str>,
    button_theme: ButtonTheme,
    fg: ratatui::style::Color,
    bg: ratatui::style::Color,
    accent: ratatui::style::Color,
}

impl ThinkingCell {
    /// The footer the template fills, split into "everything before the button"
    /// and the button's label.
    ///
    /// The action is the template's **tail**, the rule the collapsed tool card's
    /// meta row follows too, so this is a suffix match rather than a search: a
    /// locale that stopped ending with it draws the readout without a button
    /// instead of a button in the wrong place.
    ///
    /// The line count is either a `shown/total` fraction (a live card, where the
    /// number of tail rows on screen is real information) or the total alone (a
    /// finished card, which draws no rows to be "shown"). The finished template
    /// has one `{}` fewer, which is why the two are filled separately rather
    /// than passing a zero: `↕ 0/1 lines` would claim a row count the card does
    /// not have.
    fn footer(
        msgs: &Messages,
        count: LineCount,
        elapsed: Duration,
    ) -> (String, Option<&'static str>) {
        let label = msgs.thinking_card_action;
        let action = ButtonChrome::Brackets.wrap(label);
        let template = match count {
            LineCount::Fraction { .. } => msgs.thinking_card_bottom,
            LineCount::Total(1) => msgs.thinking_card_bottom_done_one,
            LineCount::Total(_) => msgs.thinking_card_bottom_done,
        };
        let mut text = template.replacen("{}", &format_elapsed(elapsed), 1);
        match count {
            LineCount::Fraction { shown, total } => {
                text = text.replacen("{}", &shown.to_string(), 1).replacen(
                    "{}",
                    &total.to_string(),
                    1,
                );
            }
            LineCount::Total(total) => text = text.replacen("{}", &total.to_string(), 1),
        }
        let text = text.replacen("{}", &action, 1);
        match text.strip_suffix(&action) {
            Some(prefix) => (prefix.to_string(), Some(label)),
            None => (text, None),
        }
    }

    pub fn active(
        block: &ActiveThinkingBlock,
        spinner: char,
        theme: &Theme,
        msgs: &Messages,
    ) -> Self {
        let lines = block.display_tail();
        let visible = lines.len().clamp(1, 3);
        let total = block.content.lines().count().max(1);
        let elapsed = block.started_at.elapsed();
        let (bottom, bottom_action) = Self::footer(
            msgs,
            LineCount::Fraction {
                shown: visible,
                total,
            },
            elapsed,
        );
        Self {
            body: Some(lines),
            title: format!(" {spinner}{}", msgs.thinking_card_title_active),
            bottom,
            bottom_action,
            button_theme: ButtonTheme::from_theme(theme),
            fg: theme.thinking_preview_fg(),
            bg: theme.bg,
            accent: theme.thinking_card_border(),
        }
    }

    pub fn completed(block: &ThinkingBlock, theme: &Theme, msgs: &Messages) -> Self {
        let total = block.content.lines().count().max(1);
        let (bottom, bottom_action) = Self::footer(msgs, LineCount::Total(total), block.elapsed);
        Self {
            body: None,
            title: msgs.thinking_card_title_done.to_string(),
            bottom,
            bottom_action,
            button_theme: ButtonTheme::from_theme(theme),
            fg: theme.thinking_preview_fg(),
            bg: theme.bg,
            accent: theme.thinking_card_border(),
        }
    }

    /// The footer as one line: the readout (elapsed, line count) in the accent
    /// color, then the button that opens the full content — drawn by the shared
    /// [`Button`] widget with the same chrome as a collapsed tool card's, so the
    /// two affordances read the same wherever they appear.
    fn bottom_line(&self) -> Line<'static> {
        let style = Style::default().fg(self.accent).bg(self.bg);
        let Some(label) = self.bottom_action else {
            return Line::from(Span::styled(self.bottom.clone(), style));
        };
        let mut spans = vec![Span::styled(self.bottom.clone(), style)];
        spans.extend(
            Button::new(label, self.button_theme)
                .chrome(ButtonChrome::Brackets)
                .line()
                .spans,
        );
        // The trailing space the other footers carry in their template, so a
        // click one column past the button hits nothing.
        spans.push(Span::styled(" ", style));
        Line::from(spans)
    }

    /// The card's first row: the title, then the readout and the button.
    ///
    /// The title leads because it names the state the block is in, and a live
    /// card's reasoning grows downward from it. On the last row the label
    /// drifted down the screen with every new line, and a status that moves is
    /// not a status.
    ///
    /// Keeping the title off the body rows is what lets reasoning keep the
    /// panel's text column: as a prefix it would push every line right.
    fn header_line(&self) -> Line<'static> {
        let title_style = Style::default().fg(self.accent).bg(self.bg);
        let mut spans = vec![
            Span::styled(PAD, title_style),
            Span::styled(self.title.clone(), title_style),
        ];
        spans.extend(self.bottom_line().spans);
        Line::from(spans)
    }

    /// How many rows the reasoning takes: zero for a finished card, which draws
    /// none, and the clamped tail size for a live one.
    fn body_lines(&self) -> usize {
        match &self.body {
            Some(lines) => lines.len().clamp(1, 3),
            None => 0,
        }
    }

    /// The reasoning rows, all at the panel's own text column.
    ///
    /// No title prefix: the title leads on its own row, so a reasoning line sits
    /// in the same column as an assistant reply and the reasoning reads as prose
    /// rather than as a caption beside a glyph. Prefixing the title would push
    /// every line 12 to 16 columns right, a fifth of an 80-column panel.
    ///
    /// A finished card has no body at all — [`Self::body_lines`] is zero and
    /// this returns nothing.
    fn body_rows(&self, width: u16) -> Vec<Line<'static>> {
        let Some(lines) = &self.body else {
            return Vec::new();
        };
        let style = Style::default().fg(self.fg).bg(self.bg);
        let mut lines = lines.clone();
        if lines.is_empty() {
            lines.push(String::new());
        }
        let room = width.saturating_sub(CONTENT_PAD).max(1) as usize;
        lines
            .into_iter()
            .take(self.body_lines())
            .map(|text| {
                Line::from(Span::styled(
                    format!("{PAD}{}", truncate_to_width(&text, room)),
                    style,
                ))
            })
            .collect()
    }

    /// The rows the cell draws, header included: one for a finished card, up to
    /// four for a live one.
    fn total_rows(&self) -> usize {
        self.body_lines() + 1
    }

    /// Where the cell's own rows land for a draw `area` clipped at
    /// `skip_lines`, and how many of them that clip ate.
    ///
    /// Both the draw and the hit rect come from here, so the glyphs a click
    /// must land on cannot drift from the glyphs the frame draws.
    fn visible_geometry(&self, area: Rect, skip_lines: usize) -> Option<(Rect, usize)> {
        let area = crate::render::util::indent_rect(area, LOG_THINKING_INDENT);
        if area.width == 0 || area.height == 0 {
            return None;
        }
        let total_rows = self.total_rows();
        if skip_lines >= total_rows {
            return None;
        }
        let height = area.height.min((total_rows - skip_lines) as u16);
        if height == 0 {
            return None;
        }
        Some((Rect::new(area.x, area.y, area.width, height), skip_lines))
    }

    /// The header's `[󰜼 Open]` button as a screen rect, for the same
    /// `(area, skip_lines)` the renderer draws the cell with.
    ///
    /// This is the card's **only** clickable glyphs: the reasoning rows are text
    /// and open nothing by themselves. `None` when the locale drew no button,
    /// when the clip has scrolled the header off the frame, or when the card is
    /// too narrow to draw the whole button — a half-drawn button is not a
    /// half-target.
    pub fn open_button_rect(&self, area: Rect, skip_lines: usize) -> Option<Rect> {
        let label = self.bottom_action?;
        let (rect, skipped) = self.visible_geometry(area, skip_lines)?;
        // The header is the cell's first row, so a clip past it leaves no
        // button to hit.
        if skipped > 0 {
            return None;
        }
        let y = rect.y;
        let glyphs = ButtonChrome::Brackets.wrap(label);
        let width = UnicodeWidthStr::width(glyphs.as_str()) as u16;
        let x = rect
            .x
            .saturating_add(CONTENT_PAD)
            .saturating_add(UnicodeWidthStr::width(self.title.as_str()) as u16)
            .saturating_add(UnicodeWidthStr::width(self.bottom.as_str()) as u16);
        if width == 0 || x.saturating_add(width) > rect.right() {
            return None;
        }
        Some(Rect::new(x, y, width, 1))
    }
}

impl Renderable for ThinkingCell {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        self.render_partial(area, buf, 0);
    }

    fn height(&self, _width: u16) -> u16 {
        self.total_rows() as u16
    }

    fn render_partial(&self, area: Rect, buf: &mut Buffer, skip_lines: usize) {
        let Some((rect, skipped)) = self.visible_geometry(area, skip_lines) else {
            return;
        };
        buf.set_style(rect, Style::default().bg(self.bg));

        let body = self.body_rows(rect.width);
        for offset in 0..rect.height as usize {
            let row = skipped + offset;
            let y = rect.y + offset as u16;
            if row == 0 {
                let line = self.header_line();
                buf.set_line(rect.x, y, &line, rect.width);
            } else if let Some(line) = body.get(row - 1) {
                buf.set_line(rect.x, y, line, rect.width);
            }
        }
    }
}

fn format_elapsed(duration: Duration) -> String {
    if duration.as_secs() == 0 {
        format!("{}ms", duration.as_millis())
    } else {
        format!("{:.1}s", duration.as_secs_f64())
    }
}

#[cfg(test)]
mod tests {
    use ratatui::{Terminal, backend::TestBackend};

    use super::*;

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

    fn render_text(cell: &ThinkingCell) -> String {
        let backend = TestBackend::new(80, cell.height(80));
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| cell.render(frame.area(), frame.buffer_mut()))
            .expect("draw");
        buffer_text(terminal.backend().buffer())
    }

    fn render_in_viewport(cell: &ThinkingCell, height: u16) -> String {
        let backend = TestBackend::new(80, height);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| cell.render(frame.area(), frame.buffer_mut()))
            .expect("draw");
        buffer_text(terminal.backend().buffer())
    }

    fn render_buffer(cell: &ThinkingCell) -> ratatui::buffer::Buffer {
        let backend = TestBackend::new(80, cell.height(80));
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| cell.render(frame.area(), frame.buffer_mut()))
            .expect("draw");
        terminal.backend().buffer().clone()
    }

    /// `(row, columns)` of the first run of cells spelling `needle` — every
    /// glyph here is one column wide, so a cell window is a fair match.
    fn find_run(buf: &ratatui::buffer::Buffer, needle: &str) -> Option<(u16, Vec<u16>)> {
        let cells: Vec<String> = needle.chars().map(|c| c.to_string()).collect();
        for y in 0..buf.area.height {
            let row: Vec<String> = (0..buf.area.width)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect();
            if let Some(start) = row.windows(cells.len()).position(|w| w == cells) {
                let cols = (start as u16..start as u16 + cells.len() as u16).collect();
                return Some((y, cols));
            }
        }
        None
    }

    fn completed_cell(msgs: &crate::i18n::Messages, theme: &Theme) -> ThinkingCell {
        let block = ThinkingBlock {
            block_id: "test-thinking".into(),
            phys_idx: 0,
            content: "first\nlast".into(),
            summary: "last".into(),
            cached_markdown: Vec::new(),
            elapsed: std::time::Duration::from_millis(1500),
        };
        ThinkingCell::completed(&block, theme, msgs)
    }

    #[test]
    fn active_thinking_cell_stops_growing_after_three_lines() {
        let theme = Theme::from(crate::theme::ThemeName::Dark);
        let msgs = crate::i18n::Messages::by_language(crate::i18n::Language::English);
        let mut three = ActiveThinkingBlock::new(0, std::time::Instant::now());
        three.push_delta("one\ntwo\nthree\n");
        let mut four = ActiveThinkingBlock::new(0, std::time::Instant::now());
        four.push_delta("one\ntwo\nthree\nfour\n");
        let three = ThinkingCell::active(&three, 'x', &theme, &msgs);
        let four = ThinkingCell::active(&four, 'x', &theme, &msgs);

        assert_eq!(three.height(80), four.height(80));
        let text = render_text(&four);
        assert!(text.contains("two") && text.contains("four"), "{text}");
        assert!(!text.contains("one"), "{text}");
        assert!(text.contains("Thinking"), "{text}");
    }

    /// A finished card is a status, not a preview: its reasoning is behind the
    /// `[Open]` button, and the row it used to spend on a summary is gone. The
    /// summary itself stays in `ThinkingBlock` — the popup and the selection
    /// read it — but nothing draws it.
    #[test]
    fn a_finished_thinking_cell_draws_only_its_header() {
        let theme = Theme::from(crate::theme::ThemeName::Dark);
        let msgs = crate::i18n::Messages::by_language(crate::i18n::Language::English);
        let block = ThinkingBlock {
            block_id: "test-thinking".into(),
            phys_idx: 0,
            content: "first\nlast".into(),
            summary: "last".into(),
            cached_markdown: Vec::new(),
            elapsed: std::time::Duration::ZERO,
        };
        let cell = ThinkingCell::completed(&block, &theme, &msgs);
        let text = render_text(&cell);

        assert_eq!(
            cell.height(80),
            COMPLETED_THINKING_ROWS as u16,
            "a finished card is its status row:\n{text}"
        );
        assert!(text.contains("Thought"), "{text}");
        assert!(
            !text.contains("last") && !text.contains("first"),
            "the summary is no longer drawn:\n{text}"
        );
    }

    #[test]
    fn thinking_cell_is_one_card_and_does_not_extend_to_viewport_bottom() {
        let theme = Theme::from(crate::theme::ThemeName::Dark);
        let msgs = crate::i18n::Messages::by_language(crate::i18n::Language::English);
        let mut active = ActiveThinkingBlock::new(0, std::time::Instant::now());
        active.push_delta("reasoning\n");
        let cell = ThinkingCell::active(&active, 'x', &theme, &msgs);

        let text = render_in_viewport(&cell, 12);
        assert_eq!(text.matches("Thinking").count(), 1, "{text}");
        assert!(
            text.lines()
                .nth(cell.height(80) as usize)
                .is_some_and(|line| line.trim().is_empty()),
            "thinking card must stop at its own height, got:\n{text}"
        );
    }

    /// Every row the card occupies carries something: a finished card is its
    /// status row and nothing else. Before, five rows carried one line of
    /// summary and the four around it were two borders and two blank rows; then
    /// two rows carried a summary the reader could open in full anyway.
    #[test]
    fn a_finished_card_spends_no_row_on_chrome() {
        let theme = Theme::from(crate::theme::ThemeName::Dark);
        let msgs = crate::i18n::Messages::by_language(crate::i18n::Language::English);
        let cell = completed_cell(&msgs, &theme);

        assert_eq!(cell.height(80), 1, "a status row, nothing else");
        let text = render_text(&cell);
        let rows: Vec<_> = text.lines().collect();
        assert_eq!(rows.len(), 1, "{text}");
        assert!(
            rows[0].contains("Thought") && rows[0].contains("Open"),
            "the status row carries the title and the button: {text}"
        );
        assert!(
            !rows[0].trim().is_empty(),
            "the one row a finished card spends is not blank: {text}"
        );
    }

    /// A live card shows its whole reasoning tail, each line under the last.
    #[test]
    fn a_live_card_spends_one_row_per_reasoning_line_plus_the_footer() {
        let theme = Theme::from(crate::theme::ThemeName::Dark);
        let msgs = crate::i18n::Messages::by_language(crate::i18n::Language::English);
        let mut active = ActiveThinkingBlock::new(0, std::time::Instant::now());
        active.push_delta("one\ntwo\nthree\n");
        let cell = ThinkingCell::active(&active, 'x', &theme, &msgs);

        assert_eq!(cell.height(80), 4, "a header plus three reasoning lines");
        let text = render_text(&cell);
        for needle in ["one", "two", "three", "Open"] {
            assert!(text.contains(needle), "{needle} missing from:\n{text}");
        }
        // One column of reasoning: `two` starts in the same terminal column
        // `one` does. The header takes the first row, so the reasoning lines
        // are rows 1..; the comparison below accounts for that.
        let area = Rect::new(0, 0, 80, cell.height(80));
        let backend = TestBackend::new(area.width, area.height);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| cell.render(frame.area(), frame.buffer_mut()))
            .expect("draw");
        let buf = terminal.backend().buffer().clone();
        let column_of = |row: u16, needle: &str| {
            (0..buf.area.width)
                .find(|&x| {
                    let drawn: String = (x..buf.area.width.min(x + needle.len() as u16))
                        .map(|c| buf[(c, row)].symbol())
                        .collect();
                    drawn.starts_with(needle)
                })
                .unwrap_or_else(|| panic!("{needle} not drawn on row {row}:\n{text}"))
        };
        assert_eq!(
            column_of(1, "one"),
            column_of(2, "two"),
            "the tail must not staircase:\n{text}"
        );
        assert_eq!(
            column_of(1, "one"),
            column_of(3, "three"),
            "every tail line shares the column:\n{text}"
        );
    }

    /// The footer's click target has to be the glyphs the frame **draws**, at
    /// the columns it draws them on — the rect is computed from the card's own
    /// geometry, and this reads it back off the buffer instead of trusting it.
    #[test]
    fn the_open_button_rect_is_the_drawn_button() {
        let theme = Theme::from(crate::theme::ThemeName::Dark);
        for lang in [
            crate::i18n::Language::English,
            crate::i18n::Language::Chinese,
        ] {
            let msgs = crate::i18n::Messages::by_language(lang);
            let cell = completed_cell(&msgs, &theme);
            let area = Rect::new(0, 0, 80, cell.height(80));
            let buf = render_buffer(&cell);
            let expected = ButtonChrome::Brackets.wrap(msgs.thinking_card_action);

            let rect = cell
                .open_button_rect(area, 0)
                .unwrap_or_else(|| panic!("{lang:?}: a drawn footer carries a button"));
            assert_eq!(rect.height, 1, "{lang:?}: the button is one row");
            // Pinned against the pixels, not against the arithmetic that
            // produced them: the rect starts on the drawn `[` and its last
            // column is the drawn `]`. (Read per cell: a wide label like
            // `打开` spans two cells each, so the row cannot be joined back
            // into the label string.)
            assert_eq!(
                buf[(rect.x, rect.y)].symbol(),
                "[",
                "{lang:?}: the rect starts on the drawn opening bracket"
            );
            assert_eq!(
                buf[(rect.right() - 1, rect.y)].symbol(),
                "]",
                "{lang:?}: the rect ends on the drawn closing bracket"
            );
            assert_eq!(
                rect.width as usize,
                UnicodeWidthStr::width(expected.as_str()),
                "{lang:?}: the rect is as wide as the label it draws"
            );
            // The columns either side are the readout's separator and the space
            // the footer ends with — neither belongs to the target.
            assert_eq!(
                buf[(rect.x - 1, rect.y)].symbol(),
                " ",
                "{lang:?}: the readout before the button stays out of the rect"
            );
            assert_eq!(
                buf[(rect.right(), rect.y)].symbol(),
                " ",
                "{lang:?}: the separator after the button stays out of the rect"
            );
        }
    }

    /// The header is the card's first row, so a clip that scrolls it away
    /// leaves no button; the rect must not point at a row that no longer shows
    /// it. A card too narrow for the whole button is also not a half-target.
    #[test]
    fn a_scrolled_off_header_has_no_button_rect() {
        let theme = Theme::from(crate::theme::ThemeName::Dark);
        let msgs = crate::i18n::Messages::by_language(crate::i18n::Language::English);
        let cell = completed_cell(&msgs, &theme);

        assert!(
            cell.open_button_rect(Rect::new(0, 0, 80, cell.height(80)), 1)
                .is_none(),
            "scrolled past the header there is no button to hit"
        );
        assert!(
            cell.open_button_rect(Rect::new(0, 0, 10, cell.height(10)), 0)
                .is_none(),
            "a card too narrow for the whole button is not a half-target"
        );
    }

    /// Unclipped, the rect lands on the glyphs the frame draws — including when
    /// the cell sits at a vertical offset inside a panel.
    #[test]
    fn the_button_rect_tracks_the_header_row() {
        let theme = Theme::from(crate::theme::ThemeName::Dark);
        let msgs = crate::i18n::Messages::by_language(crate::i18n::Language::English);
        let cell = completed_cell(&msgs, &theme);
        let area = Rect::new(0, 0, 80, cell.height(80));

        let backend = TestBackend::new(area.width, area.height);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| cell.render(frame.area(), frame.buffer_mut()))
            .expect("draw");
        let buf = terminal.backend().buffer().clone();

        let rect = cell
            .open_button_rect(area, 0)
            .expect("a fully visible card keeps its button");
        assert_eq!(rect.y, 0, "the header is the card's first row");
        let drawn: String = (rect.x..rect.right())
            .map(|x| buf[(x, rect.y)].symbol())
            .collect();
        assert_eq!(
            drawn,
            ButtonChrome::Brackets.wrap(msgs.thinking_card_action)
        );
    }

    /// The card names the state it is in: `Thinking` while the spinner turns,
    /// `Thought` once the summary is in. One shared string could only ever be
    /// right for one of the two, which is how a finished card kept reading
    /// "in progress".
    #[test]
    fn the_card_title_names_the_state_the_card_is_in() {
        use crate::i18n::{Language, Messages};

        let theme = Theme::from(crate::theme::ThemeName::Dark);
        for (lang, active_says, done_says) in [
            (Language::English, "Thinking", "Thought"),
            (Language::Chinese, "思考", "已思考"),
        ] {
            let msgs = Messages::by_language(lang);
            let mut active = ActiveThinkingBlock::new(0, std::time::Instant::now());
            active.push_delta("reasoning\n");

            let active_cell = ThinkingCell::active(&active, 'x', &theme, &msgs);
            let done_cell = completed_cell(&msgs, &theme);

            assert_eq!(
                active_cell.title,
                format!(" x{}", msgs.thinking_card_title_active),
                "{lang:?}: the running card is the spinner plus the state"
            );
            assert_eq!(
                done_cell.title, msgs.thinking_card_title_done,
                "{lang:?}: the finished card carries no spinner"
            );
            assert!(
                msgs.thinking_card_title_active.contains(active_says),
                "{lang:?}: a running card says {active_says:?}"
            );
            assert!(
                msgs.thinking_card_title_done.contains(done_says),
                "{lang:?}: a finished card says {done_says:?}"
            );
        }
    }

    /// The footer's action is not more prose: it is the kit's button (brackets
    /// from the chrome, label from the locale) drawn in the button color, with
    /// the readout around it in the border color — the same shape a collapsed
    /// tool card draws.
    ///
    /// The readout states the total, not a fraction: a finished card draws no
    /// reasoning rows, so there is no "shown" number to report.
    #[test]
    fn the_footer_ends_in_the_shared_button() {
        let theme = Theme::from(crate::theme::ThemeName::Dark);
        let en = crate::i18n::Messages::by_language(crate::i18n::Language::English);
        let zh = crate::i18n::Messages::by_language(crate::i18n::Language::Chinese);

        for (msgs, expected) in [(&en, "[󰜼 Open]"), (&zh, "[󰜼 打开]")] {
            let cell = completed_cell(msgs, &theme);
            let line = cell.bottom_line();
            let drawn: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
            assert!(drawn.trim_end().ends_with(expected), "{drawn:?}");
            // The total, and only the total: `↕ 1/2` would claim a row count the
            // card does not draw.
            assert!(
                drawn.contains("⏱ 1.5s | 2 lines") || drawn.contains("⏱ 1.5s | 2 行"),
                "{drawn:?}"
            );
            assert!(
                !drawn.contains('↕'),
                "a finished card states the total, not a fraction: {drawn:?}"
            );
            // Order is a contract too: the elapsed time leads, the line count
            // follows, each label staying on the readout it names.
            let elapsed_at = drawn
                .find('⏱')
                .unwrap_or_else(|| panic!("no elapsed readout: {drawn:?}"));
            let count_at = drawn
                .find("lines")
                .or_else(|| drawn.find('行'))
                .unwrap_or_else(|| panic!("no line count: {drawn:?}"));
            assert!(
                elapsed_at < count_at,
                "the elapsed time must lead the readout: {drawn:?}"
            );
            assert_eq!(
                line.spans[0].style.fg,
                Some(theme.thinking_card_border()),
                "the readout keeps the border color: {drawn:?}"
            );
            assert_eq!(
                line.spans[1].style.fg,
                Some(theme.muted),
                "the button keeps its own color: {drawn:?}"
            );
        }
    }

    /// One reasoning line is the common case, so the footer says `1 line`, not
    /// `1 lines` — the same singular the collapsed tool card carries.
    #[test]
    fn a_one_line_finished_card_states_one_line() {
        let theme = Theme::from(crate::theme::ThemeName::Dark);
        for lang in [
            crate::i18n::Language::English,
            crate::i18n::Language::Chinese,
        ] {
            let msgs = crate::i18n::Messages::by_language(lang);
            let block = ThinkingBlock {
                block_id: "test-thinking".into(),
                phys_idx: 0,
                content: "only line".into(),
                summary: "only line".into(),
                cached_markdown: Vec::new(),
                elapsed: std::time::Duration::from_millis(40),
            };
            let cell = ThinkingCell::completed(&block, &theme, &msgs);
            let drawn: String = cell
                .bottom_line()
                .spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect();
            assert!(
                drawn.contains("| 1 line |") || drawn.contains("| 1 行 |"),
                "{lang:?}: one reasoning line is singular: {drawn:?}"
            );
        }
    }

    /// A live card still reports the fraction: its tail rows are on screen, so
    /// "two of the twelve" is real information. This is the other half of the
    /// contract above — the two states do not share a template.
    #[test]
    fn a_live_card_reports_how_much_of_the_reasoning_is_drawn() {
        let theme = Theme::from(crate::theme::ThemeName::Dark);
        for lang in [
            crate::i18n::Language::English,
            crate::i18n::Language::Chinese,
        ] {
            let msgs = crate::i18n::Messages::by_language(lang);
            let mut active = ActiveThinkingBlock::new(0, std::time::Instant::now());
            active.push_delta("one\ntwo\nthree\nfour\n");
            let cell = ThinkingCell::active(&active, 'x', &theme, &msgs);
            let drawn: String = cell
                .bottom_line()
                .spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect();
            assert!(
                drawn.contains("↕ 3/4"),
                "{lang:?}: a live card reports the drawn tail: {drawn:?}"
            );
        }
    }

    /// The footer reaches the frame as an ordinary drawn line, so pin the drawn
    /// glyphs *and their color* in a real buffer: a title that dropped the
    /// button's spans would still read as text.
    #[test]
    fn the_footer_button_is_drawn_in_the_button_color() {
        let theme = Theme::from(crate::theme::ThemeName::Dark);
        let msgs = crate::i18n::Messages::by_language(crate::i18n::Language::English);
        let cell = completed_cell(&msgs, &theme);

        let buf = render_buffer(&cell);
        let glyphs = ButtonChrome::Brackets.wrap(msgs.thinking_card_action);
        let (row, cols) = find_run(&buf, &glyphs).unwrap_or_else(|| {
            panic!(
                "the footer button {glyphs:?} must be drawn, got:\n{}",
                buffer_text(&buf)
            )
        });
        for x in &cols {
            assert_eq!(
                buf[(*x, row)].fg,
                theme.muted,
                "column {x} of the button carries the button color"
            );
            assert_eq!(buf[(*x, row)].bg, theme.bg, "painted on the card surface");
        }
        // One column left of it is the separator the readout ends with, drawn
        // in the border color — the button is a patch inside the sentence.
        assert_eq!(buf[(cols[0] - 1, row)].fg, theme.thinking_card_border());
    }

    /// The footer's tail is a contract, not an accident: both locales end the
    /// template with the action, so the split is a suffix match and neither can
    /// silently lose its button.
    #[test]
    fn every_locale_ends_its_footer_with_the_action() {
        let theme = Theme::from(crate::theme::ThemeName::Dark);
        for lang in [
            crate::i18n::Language::English,
            crate::i18n::Language::Chinese,
        ] {
            let msgs = crate::i18n::Messages::by_language(lang);
            let cell = completed_cell(&msgs, &theme);
            assert_eq!(
                cell.bottom_action,
                Some(msgs.thinking_card_action),
                "{lang:?}: the template must end with {}",
                msgs.thinking_card_action
            );
        }
    }
}
