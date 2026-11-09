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

/// Rows a thinking card occupies: one per body line, plus the footer.
///
/// A finished card passes one body line (its summary) and takes two rows. A
/// live card passes the size of its reasoning tail, so it grows to four.
///
/// The card draws no border and no blank rows of its own. Before, five rows
/// carried one line of summary; the four rows around it were chrome, and four
/// of every five rows a thinking card spent showed nothing. The footer stays a
/// separate row rather than joining the summary, because one row cannot hold
/// the summary, the elapsed time, the line count and the open button at any
/// terminal width a reader actually uses: measured, the button is clipped below
/// 120 columns.
/// The column the card's text starts in, one right of the cell's own indent.
///
/// That is where assistant prose lands (`LOG_THINKING_INDENT + 1`), so a
/// reasoning line and the reply under it start on the same vertical. The card
/// used to get this column from its left border; without the border the pad
/// carries it.
const CONTENT_PAD: u16 = 1;

/// [`CONTENT_PAD`] as the prose prefix, so the glyphs and the hit rect agree.
const PAD: &str = " ";

pub fn thinking_visual_rows(body_lines: usize) -> usize {
    body_lines.clamp(1, 3) + 1
}

pub struct ThinkingCell {
    lines: Vec<String>,
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
    fn footer(
        msgs: &Messages,
        shown: usize,
        total: usize,
        elapsed: Duration,
    ) -> (String, Option<&'static str>) {
        let label = msgs.thinking_card_action;
        let action = ButtonChrome::Brackets.wrap(label);
        // Filled in template order: elapsed first, then the line count.
        let text = msgs
            .thinking_card_bottom
            .replacen("{}", &format_elapsed(elapsed), 1)
            .replacen("{}", &shown.to_string(), 1)
            .replacen("{}", &total.to_string(), 1)
            .replacen("{}", &action, 1);
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
        let (bottom, bottom_action) = Self::footer(msgs, visible, total, elapsed);
        Self {
            lines,
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
        let (bottom, bottom_action) = Self::footer(msgs, 1, total, block.elapsed);
        Self {
            lines: vec![block.summary.clone()],
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

    /// The card's last row: the title, then the readout and the button.
    ///
    /// The title lives here rather than above the reasoning so every reasoning
    /// line keeps the panel's text column. It also keeps the card's identity on
    /// the row that already carries its facts.
    fn footer_line(&self) -> Line<'static> {
        let title_style = Style::default().fg(self.accent).bg(self.bg);
        let mut spans = vec![
            Span::styled(PAD, title_style),
            Span::styled(self.title.clone(), title_style),
        ];
        spans.extend(self.bottom_line().spans);
        Line::from(spans)
    }

    fn body_lines(&self) -> usize {
        self.lines.len().clamp(1, 3)
    }

    /// The reasoning rows, all at the panel's own text column.
    ///
    /// No title prefix: the title moves to the footer, so a summary sits in the
    /// same column as an assistant reply and the reasoning reads as prose rather
    /// than as a caption beside a glyph. Prefixing the title would push every
    /// line 14 columns right, which is a sixth of an 80-column panel.
    fn body_rows(&self, width: u16) -> Vec<Line<'static>> {
        let style = Style::default().fg(self.fg).bg(self.bg);
        let mut lines = self.lines.clone();
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
        let total_rows = self.body_lines() + 1;
        if skip_lines >= total_rows {
            return None;
        }
        let height = area.height.min((total_rows - skip_lines) as u16);
        if height == 0 {
            return None;
        }
        Some((Rect::new(area.x, area.y, area.width, height), skip_lines))
    }

    /// The footer's `[󰜼 Open]` button as a screen rect, for the same
    /// `(area, skip_lines)` the renderer draws the cell with.
    ///
    /// This is the card's **only** clickable glyphs: the summary row is text and
    /// opens nothing by itself. `None` when the locale drew no button, when the
    /// clip leaves the footer off the frame, or when the card is too narrow to
    /// draw the whole button — a half-drawn button is not a half-target.
    pub fn footer_button_rect(&self, area: Rect, skip_lines: usize) -> Option<Rect> {
        let label = self.bottom_action?;
        let (rect, skipped) = self.visible_geometry(area, skip_lines)?;
        // The footer is the cell's last row; the clip may have eaten it.
        let footer_row = self.body_lines();
        if skipped > footer_row {
            return None;
        }
        let y = rect.y + (footer_row - skipped) as u16;
        if y >= rect.bottom() {
            return None;
        }
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
        thinking_visual_rows(self.body_lines()) as u16
    }

    fn render_partial(&self, area: Rect, buf: &mut Buffer, skip_lines: usize) {
        let Some((rect, skipped)) = self.visible_geometry(area, skip_lines) else {
            return;
        };
        buf.set_style(rect, Style::default().bg(self.bg));

        let body = self.body_rows(rect.width);
        let footer_row = self.body_lines();
        for offset in 0..rect.height as usize {
            let row = skipped + offset;
            let y = rect.y + offset as u16;
            if row < footer_row {
                buf.set_line(rect.x, y, &body[row], rect.width);
            } else if row == footer_row {
                let line = self.footer_line();
                buf.set_line(rect.x, y, &line, rect.width);
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

    #[test]
    fn completed_thinking_cell_renders_only_its_summary() {
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
        let text = render_text(&ThinkingCell::completed(&block, &theme, &msgs));
        assert!(text.contains("last"), "{text}");
        assert!(!text.contains("first"), "{text}");
        assert!(text.contains("Thought"), "{text}");
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
    /// summary and its footer, nothing else. Before, five rows carried one line
    /// of summary and the four around it were a border and two blank rows.
    #[test]
    fn a_finished_card_spends_no_row_on_chrome() {
        let theme = Theme::from(crate::theme::ThemeName::Dark);
        let msgs = crate::i18n::Messages::by_language(crate::i18n::Language::English);
        let cell = completed_cell(&msgs, &theme);

        assert_eq!(cell.height(80), 2, "a summary plus its footer");
        let text = render_text(&cell);
        let rows: Vec<_> = text.lines().collect();
        assert_eq!(rows.len(), 2, "{text}");
        assert!(rows[0].contains("last"), "the summary leads: {text}");
        assert!(
            rows[1].contains("Thought") && rows[1].contains("Open"),
            "the title and the button share the footer: {text}"
        );
        for (n, row) in rows.iter().enumerate() {
            assert!(
                !row.trim().is_empty(),
                "row {n} of a thinking card is blank: {text}"
            );
        }
    }

    /// A live card shows its whole reasoning tail, each line under the last.
    #[test]
    fn a_live_card_spends_one_row_per_reasoning_line_plus_the_footer() {
        let theme = Theme::from(crate::theme::ThemeName::Dark);
        let msgs = crate::i18n::Messages::by_language(crate::i18n::Language::English);
        let mut active = ActiveThinkingBlock::new(0, std::time::Instant::now());
        active.push_delta("one\ntwo\nthree\n");
        let cell = ThinkingCell::active(&active, 'x', &theme, &msgs);

        assert_eq!(cell.height(80), 4, "three reasoning lines plus the footer");
        let text = render_text(&cell);
        for needle in ["one", "two", "three", "Open"] {
            assert!(text.contains(needle), "{needle} missing from:\n{text}");
        }
        // One column of reasoning: `two` starts in the same terminal column
        // `one` does. Read the column off the buffer rather than off a joined
        // string: the header glyph is one character but two cells, so a
        // character index and a cell index disagree exactly where this matters.
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
            column_of(0, "one"),
            column_of(1, "two"),
            "the tail must not staircase:\n{text}"
        );
        assert_eq!(
            column_of(0, "one"),
            column_of(2, "three"),
            "every tail line shares the column:\n{text}"
        );
    }

    /// The footer's click target has to be the glyphs the frame **draws**, at
    /// the columns it draws them on — the rect is computed from the card's own
    /// geometry, and this reads it back off the buffer instead of trusting it.
    #[test]
    fn the_footer_button_rect_is_the_drawn_button() {
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
                .footer_button_rect(area, 0)
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

    /// A card whose footer row is clipped off the frame draws no button, so it
    /// has no target — the rect must not point at rows that are not there.
    #[test]
    fn a_clipped_footer_has_no_button_rect() {
        let theme = Theme::from(crate::theme::ThemeName::Dark);
        let msgs = crate::i18n::Messages::by_language(crate::i18n::Language::English);
        let cell = completed_cell(&msgs, &theme);

        assert!(
            cell.footer_button_rect(Rect::new(0, 0, 80, 1), 0).is_none(),
            "a card whose last row is clipped off draws no button"
        );
        assert!(
            cell.footer_button_rect(Rect::new(0, 0, 10, cell.height(10)), 0)
                .is_none(),
            "a card too narrow for the whole button is not a half-target"
        );
    }

    /// Scrolled up by one row the summary is gone and the footer moves into its
    /// place, so the rect has to follow it.
    #[test]
    fn the_button_rect_follows_a_scrolled_card() {
        let theme = Theme::from(crate::theme::ThemeName::Dark);
        let msgs = crate::i18n::Messages::by_language(crate::i18n::Language::English);
        let cell = completed_cell(&msgs, &theme);
        let area = Rect::new(0, 0, 80, cell.height(80));

        let backend = TestBackend::new(area.width, area.height);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| cell.render_partial(frame.area(), frame.buffer_mut(), 1))
            .expect("draw");
        let buf = terminal.backend().buffer().clone();

        let rect = cell
            .footer_button_rect(area, 1)
            .expect("a fully visible card keeps its button");
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
            assert!(
                drawn.contains("↕ 1/2 lines") || drawn.contains("↕ 1/2 行"),
                "{drawn:?}"
            );
            // Order is a contract too: the elapsed time leads, the line count
            // follows, each label staying on the readout it names.
            let elapsed_at = drawn
                .find('⏱')
                .unwrap_or_else(|| panic!("no elapsed readout: {drawn:?}"));
            let count_at = drawn
                .find('↕')
                .unwrap_or_else(|| panic!("no line count: {drawn:?}"));
            assert!(
                elapsed_at < count_at && drawn.contains("⏱ 1.5s | ↕ 1/2"),
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

    /// The footer reaches the frame as a `title_bottom` line, so pin the drawn
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
