use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, List, ListItem, Paragraph, Widget},
};
use unicode_width::UnicodeWidthStr;

use crate::{render::util::wrap_line, state::SelectPopup};

/// Layout result for the select popup.
pub struct SelectPopupLayout {
    /// Outer popup rect (borders included).
    pub popup_area: Rect,
    /// Prompt lines (already truncated to fit the popup).
    pub prompt_lines: Vec<Line<'static>>,
    /// Number of option rows visible inside the popup.
    pub visible: usize,
    /// Scroll offset into `options` (index of the first visible option).
    pub offset: usize,
}

/// Compute the select popup geometry for a given state and available area.
///
/// The popup height is capped by `area`. The visible option window is derived
/// from the focused index so the selected row is always on screen — once the
/// option list overflows the popup, the window scrolls to keep the selection
/// in view (same behavior as the slash-command popup for long lists).
///
/// `footer_width` is the display width of the bottom-border navigation hint;
/// the popup is widened to fit it so the hint is never clipped.
pub fn select_popup_layout(
    state: &SelectPopup,
    area: Rect,
    fg_color: Color,
    footer_width: u16,
) -> SelectPopupLayout {
    let option_count = state.options.len().max(1);
    let max_w = area.width.saturating_sub(4).max(1);

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
    // borders(2) + separator(1) + at least 1 list row; the navigation hint
    // lives in the bottom border (`title_bottom`), so it consumes no content
    // height.
    let max_prompt_rows = max_popup_h.saturating_sub(2 + 1 + 1).max(1) as usize;
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

    // The option window must agree with the popup height: if the List were
    // sized to the full option count, the bottom rows would be clipped and
    // the selected row could land outside the visible content.
    let max_list_rows = max_popup_h.saturating_sub(2 + 1 + prompt_rows).max(1) as usize;
    let visible = option_count.min(max_list_rows);
    let offset = scroll_offset(state.selected, option_count, visible);

    let popup_height = (prompt_rows + 1 + visible as u16 + 2).min(max_popup_h);
    let popup_area =
        crate::render::popups::centered_list_popup_area(area, popup_width, popup_height);

    SelectPopupLayout {
        popup_area,
        prompt_lines,
        visible,
        offset,
    }
}

/// Keep `selected` inside `[offset, offset + visible)` once the list overflows
/// the popup. The selected row is pinned near the bottom with ~2 context rows
/// below it, matching the slash-command popup anchor.
fn scroll_offset(selected: usize, option_count: usize, visible: usize) -> usize {
    if option_count <= visible {
        return 0;
    }
    let max_offset = option_count.saturating_sub(visible);
    let pin = visible.saturating_sub(3).min(visible.saturating_sub(1));
    selected.saturating_sub(pin).min(max_offset)
}

/// Selection popup widget: displays prompt and option list centered, supports keyboard/mouse selection.
pub struct SelectPopupWidget<'a> {
    state: &'a SelectPopup,
    /// Highlight background color for selected item.
    highlight_color: Color,
    /// Normal option foreground color.
    fg_color: Color,
    /// Popup background color.
    bg_color: Color,
    /// Hint text when there are no options.
    empty_text: &'static str,
    /// Selected item prefix arrow.
    arrow: &'static str,
    /// Border type for the popup frame.
    border_type: BorderType,
    /// Navigation hint rendered in the bottom border (e.g. `↑↓/j/k`).
    footer: Option<ratatui::text::Line<'static>>,
}

impl<'a> SelectPopupWidget<'a> {
    pub fn new(
        state: &'a SelectPopup,
        highlight_color: Color,
        fg_color: Color,
        bg_color: Color,
        empty_text: &'static str,
        arrow: &'static str,
    ) -> Self {
        SelectPopupWidget {
            state,
            highlight_color,
            fg_color,
            bg_color,
            empty_text,
            arrow,
            border_type: BorderType::Rounded,
            footer: None,
        }
    }

    /// Set a custom border type.
    pub fn with_border_type(mut self, border_type: BorderType) -> Self {
        self.border_type = border_type;
        self
    }

    /// Set the navigation hint rendered in the bottom border (styled spans).
    pub fn with_footer(mut self, footer: ratatui::text::Line<'static>) -> Self {
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
        select_popup_layout(self.state, area, self.fg_color, self.footer_width()).popup_area
    }
}

impl Widget for SelectPopupWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer)
    where
        Self: Sized,
    {
        let SelectPopupLayout {
            popup_area,
            prompt_lines,
            visible,
            offset,
        } = select_popup_layout(self.state, area, self.fg_color, self.footer_width());
        let prompt_rows = prompt_lines.len() as u16;

        Clear.render(popup_area, buf);

        let title = if self.state.multi {
            " Multi-select "
        } else {
            " Select "
        };
        let mut block = Block::default()
            .borders(Borders::ALL)
            .border_type(self.border_type)
            .title(title)
            .style(Style::default().bg(self.bg_color));
        if let Some(footer) = self.footer.clone() {
            block = block.title_bottom(footer);
        }
        block.render(popup_area, buf);

        let inner = crate::render::popups::popup_inner(popup_area);
        let constraints = vec![
            Constraint::Length(prompt_rows),
            Constraint::Length(1),
            Constraint::Length(visible as u16),
        ];
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints(constraints)
            .split(inner);

        Paragraph::new(prompt_lines).render(chunks[0], buf);

        let items: Vec<ListItem> = if self.state.options.is_empty() {
            vec![ListItem::new(Span::styled(
                self.empty_text,
                Style::default().fg(Color::Gray),
            ))]
        } else {
            let selected = self
                .state
                .selected
                .min(self.state.options.len().saturating_sub(1));
            let end = (offset + visible).min(self.state.options.len());
            self.state.options[offset..end]
                .iter()
                .enumerate()
                .map(|(i, opt)| {
                    let abs_i = offset + i;
                    let is_focused = abs_i == selected;
                    let style = if is_focused {
                        Style::default().bg(self.highlight_color).fg(Color::White)
                    } else {
                        Style::default().fg(self.fg_color)
                    };
                    let cursor = if is_focused { self.arrow } else { "  " };
                    let text = if self.state.multi {
                        let mark = if self.state.checked.get(abs_i).copied().unwrap_or(false) {
                            "[x]"
                        } else {
                            "[ ]"
                        };
                        format!("{cursor}{mark} {opt}")
                    } else {
                        format!("{cursor}{opt}")
                    };
                    ListItem::new(Span::styled(text, style))
                })
                .collect()
        };

        List::new(items)
            .block(Block::default())
            .render(chunks[2], buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state_with(n: usize, selected: usize) -> SelectPopup {
        SelectPopup {
            options: (0..n).map(|i| format!("opt-{i:02}")).collect(),
            selected,
            ..SelectPopup::default()
        }
    }

    #[test]
    fn layout_window_shows_all_options_when_short() {
        let state = state_with(5, 3);
        let layout = select_popup_layout(&state, Rect::new(0, 0, 100, 30), Color::White, 0);
        assert_eq!(layout.visible, 5);
        assert_eq!(layout.offset, 0);
    }

    #[test]
    fn layout_widens_to_fit_footer_hint() {
        let state = state_with(5, 0);
        // Short options give a narrow popup; a wide footer must widen it so
        // the bottom-border hint is not clipped.
        let no_footer = select_popup_layout(&state, Rect::new(0, 0, 100, 30), Color::White, 0);
        let wide_footer = select_popup_layout(&state, Rect::new(0, 0, 100, 30), Color::White, 60);
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
        let layout = select_popup_layout(&state, Rect::new(0, 0, 100, 30), Color::White, 0);
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
        let layout = select_popup_layout(&state, Rect::new(0, 0, 100, 7), Color::White, 0);
        assert_eq!(layout.visible, 1, "7-row main area fits one list row");
        assert_eq!(layout.offset, 25, "window must jump to the selected row");
    }

    #[test]
    fn layout_window_clamps_at_the_end() {
        let state = state_with(30, 29);
        let layout = select_popup_layout(&state, Rect::new(0, 0, 100, 30), Color::White, 0);
        assert_eq!(
            layout.offset + layout.visible,
            30,
            "window must not run past the end"
        );
        assert!(29 >= layout.offset && 29 < layout.offset + layout.visible);
    }
}
