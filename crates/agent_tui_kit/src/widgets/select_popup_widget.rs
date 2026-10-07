use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::Widget,
};
use unicode_width::UnicodeWidthStr;

use crate::{
    render::util::wrap_line,
    state::SelectPopup,
    theme::Theme,
    widgets::list_popup::{ListPopup, ListRow, SelectionStyle, window_offset},
};

/// Layout result for the select popup.
pub struct SelectPopupLayout {
    /// Outer popup rect (borders included).
    pub popup_area: Rect,
    /// Prompt lines (already truncated to fit the popup).
    pub prompt_lines: Vec<Line<'static>>,
    /// Number of option rows visible inside the popup.
    pub visible: usize,
    /// Scroll offset into the filtered options (index of the first visible one).
    pub offset: usize,
}

/// Compute the select popup geometry for a given state and available area.
///
/// The popup height is capped by `area`. The visible option window is derived
/// from the focused index so the selected row is always on screen — once the
/// option list overflows the popup, the window scrolls to keep the selection
/// in view (the shared rule in [`window_offset`], as in every other list
/// popup).
///
/// `footer_width` is the display width of the bottom-border navigation hint;
/// the popup is widened to fit it so the hint is never clipped.
pub fn select_popup_layout(
    state: &SelectPopup,
    area: Rect,
    fg_color: Color,
    footer_width: u16,
) -> SelectPopupLayout {
    let max_w = area.width.saturating_sub(4).max(1);
    // The filter line is a header row of its own, always present for a
    // filterable popup.
    let query_rows = u16::from(state.filterable());

    // ~50% of screen width; still at least fit options / a readable minimum.
    const MIN_WIDTH: u16 = 36;
    let prefix_w = if state.multi { 8usize } else { 4usize };
    let content_w = state
        .options
        .iter()
        .map(|o| UnicodeWidthStr::width(o.as_str()).saturating_add(prefix_w))
        .max()
        .unwrap_or(20)
        .saturating_add(4) as u16;
    let half = ((area.width as f32) * 0.5) as u16;
    let footer_w = footer_width.saturating_add(4);
    let popup_width = half
        .max(content_w)
        .max(footer_w.min(max_w))
        .max(MIN_WIDTH.min(max_w))
        .min(max_w);

    let inner_w = popup_width.saturating_sub(2).max(1) as usize;
    let prompt_style = Style::default().fg(fg_color);
    let mut prompt_lines = wrap_line(
        &Line::from(Span::styled(state.prompt.clone(), prompt_style)),
        inner_w,
    );

    let max_popup_h = area.height.saturating_sub(2).max(1);
    // borders(2) + separator(1) + filter line + at least 1 list row; the
    // navigation hint lives in the bottom border (`title_bottom`), so it
    // consumes no content height.
    let header_fixed = 2 + 1 + query_rows;
    // The prompt is the only elastic part — the filter line and one list row
    // are not — so on a popup too short for both the prompt is dropped rather
    // than pushing the focused row off the bottom.
    let max_prompt_rows = max_popup_h.saturating_sub(header_fixed + 1) as usize;
    if prompt_lines.len() > max_prompt_rows {
        prompt_lines.truncate(max_prompt_rows);
        if let Some(last) = prompt_lines.last_mut() {
            *last = Line::from(Span::styled(
                format!(
                    "{}…",
                    last.spans
                        .iter()
                        .map(|s| s.content.as_ref())
                        .collect::<String>()
                        .chars()
                        .take(inner_w.saturating_sub(1))
                        .collect::<String>()
                ),
                prompt_style,
            ));
        }
    }
    let prompt_rows = prompt_lines.len() as u16;

    // The option window must agree with the popup height: if the list were
    // sized to the full option count, the bottom rows would be clipped and
    // the selected row could land outside the visible content.
    let max_list_rows = max_popup_h
        .saturating_sub(header_fixed + prompt_rows)
        .max(1) as usize;
    // The window counts *visible* options: a filter that matches three of
    // forty rows shows three.
    let visible_count = state.filtered_indices().len().max(1);
    let visible = visible_count.min(max_list_rows);
    let offset = window_offset(visible_count, state.selected, visible);

    let popup_height = (prompt_rows + query_rows + 1 + visible as u16 + 2).min(max_popup_h);
    let popup_area =
        crate::render::popups::centered_list_popup_area(area, popup_width, popup_height);

    SelectPopupLayout {
        popup_area,
        prompt_lines,
        visible,
        offset,
    }
}

/// Selection popup widget: displays prompt and option list centered, supports keyboard/mouse selection.
///
/// The list itself — window, focused-row band, empty hint — is the shared
/// [`ListPopup`]. This type only supplies the prompt header and the option rows.
pub struct SelectPopupWidget<'a> {
    state: &'a SelectPopup,
    theme: &'a Theme,
    /// Hint text when there are no options.
    empty_text: &'static str,
    /// Selected item prefix arrow.
    arrow: &'static str,
    /// Grey text inside the empty filter line.
    filter_placeholder: &'static str,
    /// Navigation hint rendered in the bottom border (e.g. `↑↓/j/k`).
    footer: Option<Line<'static>>,
}

impl<'a> SelectPopupWidget<'a> {
    pub fn new(
        state: &'a SelectPopup,
        theme: &'a Theme,
        empty_text: &'static str,
        arrow: &'static str,
    ) -> Self {
        SelectPopupWidget {
            state,
            theme,
            empty_text,
            arrow,
            filter_placeholder: "",
            footer: None,
        }
    }

    /// Text drawn inside the filter line while it is empty.
    pub fn with_filter_placeholder(mut self, placeholder: &'static str) -> Self {
        self.filter_placeholder = placeholder;
        self
    }

    /// Set the navigation hint rendered in the bottom border (styled spans).
    pub fn with_footer(mut self, footer: Line<'static>) -> Self {
        self.footer = Some(footer);
        self
    }

    /// Display width of the bottom-border navigation hint (0 when absent).
    fn footer_width(&self) -> u16 {
        self.footer.as_ref().map(|f| f.width() as u16).unwrap_or(0)
    }

    /// Outer popup rect for the current state/area (used by the app layer to
    /// route mouse-wheel scrolls to the popup).
    pub fn popup_area(&self, area: Rect) -> Rect {
        select_popup_layout(self.state, area, self.theme.fg, self.footer_width()).popup_area
    }

    /// The filter line: a search icon, the query (or its grey placeholder while
    /// empty) and a caret block at the insertion point.
    ///
    /// The line is the popup's only text field, and it is always the focus, so
    /// the caret is always drawn — without it the line read as one more row of
    /// the list and the `>` that used to be there said nothing about typing.
    fn filter_line(&self) -> Line<'static> {
        let theme = self.theme;
        // A space with the text colour behind it: the terminal-cursor look, and
        // it needs no background of its own (the popup already paints one).
        let caret = Span::styled(" ", Style::default().bg(theme.fg).fg(theme.bg));
        let mut spans = vec![Span::styled(
            "\u{1f50d} ",
            Style::default().fg(theme.accent),
        )];
        if self.state.query.is_empty() {
            spans.push(caret);
            if !self.filter_placeholder.is_empty() {
                spans.push(Span::styled(
                    self.filter_placeholder,
                    Style::default().fg(theme.muted),
                ));
            }
        } else {
            spans.push(Span::styled(
                self.state.query.clone(),
                Style::default().fg(theme.fg),
            ));
            spans.push(caret);
        }
        Line::from(spans)
    }

    /// The option rows the filter leaves visible, with the focus marker applied.
    fn rows(&self) -> Vec<ListRow<'static>> {
        self.state
            .filtered_indices()
            .into_iter()
            .map(|i| {
                let opt = &self.state.options[i];
                let cursor = if i == self.state.selected {
                    self.arrow
                } else {
                    "  "
                };
                let text = if self.state.multi {
                    let mark = if self.state.checked.get(i).copied().unwrap_or(false) {
                        "[x]"
                    } else {
                        "[ ]"
                    };
                    format!("{cursor}{mark} {opt}")
                } else {
                    format!("{cursor}{opt}")
                };
                // No focus-dependent color here: `SelectionStyle::Highlight`
                // owns the focused row's foreground, so every row is built as
                // if it were unfocused.
                ListRow::item(vec![Span::styled(text, Style::default().fg(self.theme.fg))])
            })
            .collect()
    }
}

impl Widget for SelectPopupWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let SelectPopupLayout {
            popup_area,
            prompt_lines,
            ..
        } = select_popup_layout(self.state, area, self.theme.fg, self.footer_width());
        let rows = self.rows();
        // `selected` is an index into `options`; the window and the highlight
        // need its position among the *visible* rows.
        let selected_row = self
            .state
            .filtered_indices()
            .iter()
            .position(|&i| i == self.state.selected)
            .unwrap_or(0);
        let title = if self.state.multi {
            " Multi-select "
        } else {
            " Select "
        };

        let mut header = prompt_lines;
        if self.state.filterable() {
            header.push(self.filter_line());
        }

        let mut popup = ListPopup::new(self.theme, &rows, popup_area.width, popup_area.height)
            .title(title)
            .selected_row(selected_row)
            .empty_text(self.empty_text)
            .selection(SelectionStyle::Highlight)
            .bg(Some(self.theme.bottom_bar_bg))
            .header(header);
        if let Some(footer) = self.footer {
            popup = popup.footer(footer);
        }
        popup.render(area, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::ThemeName;

    fn state_with(n: usize, selected: usize) -> SelectPopup {
        let mut popup = SelectPopup::default();
        popup.list.options = (0..n).map(|i| format!("opt-{i:02}")).collect();
        popup.list.selected = selected;
        popup
    }

    fn layout(state: &SelectPopup, area: Rect, footer_width: u16) -> SelectPopupLayout {
        select_popup_layout(state, area, Theme::from(ThemeName::Dark).fg, footer_width)
    }

    /// Flatten a buffer into one string per row.
    fn rows_text(buf: &Buffer) -> Vec<String> {
        (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect()
            })
            .collect()
    }

    /// Render a local (filterable) pick with one option.
    fn render_filterable(query: &str) -> (Buffer, Theme, Rect) {
        let theme = Theme::from(ThemeName::Dark);
        let mut state = SelectPopup::default();
        state.set_local("Select model".into(), vec!["opt-00".into()], 0, true);
        state.query = query.to_string();
        let area = Rect::new(0, 0, 60, 20);
        let mut buf = Buffer::empty(area);
        SelectPopupWidget::new(&state, &theme, "No options", "▶ ")
            .with_filter_placeholder("type to filter")
            .render(area, &mut buf);
        let inner = crate::render::popups::popup_inner(
            select_popup_layout(&state, area, theme.fg, 0).popup_area,
        );
        (buf, theme, inner)
    }

    #[test]
    fn an_empty_filter_line_shows_the_icon_the_placeholder_and_a_caret() {
        let (buf, theme, inner) = render_filterable("");
        // Header: prompt on the first row, the filter line on the second.
        let line = &rows_text(&buf)[inner.y as usize + 1];

        assert!(
            line.contains('\u{1f50d}'),
            "the filter line needs a search icon: {line:?}"
        );
        assert!(
            line.contains("type to filter"),
            "an empty filter line needs its placeholder: {line:?}"
        );
        // The caret sits at the insertion point — position 0, right after the
        // icon — and is a solid block in the text colour.
        let caret_x = inner.x + 3;
        assert_eq!(buf[(caret_x, inner.y + 1)].bg, theme.fg, "caret block");
        assert_eq!(buf[(caret_x, inner.y + 1)].fg, theme.bg);
    }

    #[test]
    fn typing_replaces_the_placeholder_and_moves_the_caret() {
        let (buf, theme, inner) = render_filterable("kim");
        let line = &rows_text(&buf)[inner.y as usize + 1];

        assert!(line.contains("kim"), "the query is on the line: {line:?}");
        assert!(
            !line.contains("type to filter"),
            "the placeholder must give way: {line:?}"
        );
        let caret_x = inner.x + 3 + 3;
        assert_eq!(
            buf[(caret_x, inner.y + 1)].bg,
            theme.fg,
            "the caret follows the query"
        );
    }

    #[test]
    fn an_agent_prompt_has_no_filter_line() {
        let theme = Theme::from(ThemeName::Dark);
        let mut state = SelectPopup::default();
        state.set("Allow?".into(), vec!["yes".into()], 7, true);
        let area = Rect::new(0, 0, 60, 20);
        let mut buf = Buffer::empty(area);
        SelectPopupWidget::new(&state, &theme, "No options", "▶ ")
            .with_filter_placeholder("type to filter")
            .render(area, &mut buf);

        let text = rows_text(&buf).join("\n");
        assert!(!text.contains('\u{1f50d}'));
        assert!(!text.contains("type to filter"));
    }

    #[test]
    fn layout_window_shows_all_options_when_short() {
        let state = state_with(5, 3);
        let layout = layout(&state, Rect::new(0, 0, 100, 30), 0);
        assert_eq!(layout.visible, 5);
        assert_eq!(layout.offset, 0);
    }

    #[test]
    fn layout_widens_to_fit_footer_hint() {
        let state = state_with(5, 0);
        // Short options give a narrow popup; a wide footer must widen it so
        // the bottom-border hint is not clipped.
        let no_footer = layout(&state, Rect::new(0, 0, 100, 30), 0);
        let wide_footer = layout(&state, Rect::new(0, 0, 100, 30), 60);
        assert!(
            wide_footer.popup_area.width > no_footer.popup_area.width,
            "popup must widen to fit the footer (no_footer={}, wide_footer={})",
            no_footer.popup_area.width,
            wide_footer.popup_area.width
        );
    }

    #[test]
    fn layout_window_scrolls_to_keep_selection_visible() {
        let state = state_with(30, 25);
        let layout = layout(&state, Rect::new(0, 0, 100, 30), 0);
        assert!(
            layout.visible < 30,
            "long list must cap the visible window (visible={})",
            layout.visible
        );
        assert!(
            layout.offset <= 25 && 25 < layout.offset + layout.visible,
            "selected 25 must be inside the visible window offset={} visible={}",
            layout.offset,
            layout.visible
        );
    }

    #[test]
    fn layout_window_keeps_selection_visible_on_short_terminal() {
        let state = state_with(30, 25);
        // A 13-row terminal yields a ~7-row main area (status + input + bottom
        // bars take the rest); 7 rows fit exactly one option row.
        let layout = layout(&state, Rect::new(0, 0, 100, 7), 0);
        assert_eq!(layout.visible, 1, "7-row main area fits one list row");
        assert_eq!(layout.offset, 25, "window must jump to the selected row");
    }

    #[test]
    fn layout_window_clamps_at_the_end() {
        let state = state_with(30, 29);
        let layout = layout(&state, Rect::new(0, 0, 100, 30), 0);
        assert_eq!(
            layout.offset + layout.visible,
            30,
            "window must not run past the end"
        );
        assert!(29 >= layout.offset && 29 < layout.offset + layout.visible);
    }
}
