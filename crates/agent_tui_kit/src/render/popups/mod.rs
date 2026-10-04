//! Popup chrome helpers and popup renderers (pure).
//!
//! Moved from `crates/tui/src/render/popups` in the Ctx migration slice.
//! Each renderer takes `&RenderCtx` (or the popup's own state) and returns
//! mouse hit areas; the host applies them after the frame.

pub mod code_popup;
pub mod diff_popup;
pub mod history;
pub mod mermaid_popup;
pub mod scrollable_popup;
pub mod select;
pub mod subagent_popup;
pub mod system_prompt_popup;
pub mod task_dag_popup;
pub mod thinking_popup;

use ratatui::{
    Frame,
    layout::{Alignment, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Scrollbar, ScrollbarState},
};
use unicode_width::UnicodeWidthStr;

use crate::{
    theme::Theme,
    widgets::button::{Button, ButtonState, ButtonTheme},
};

/// Mouse hit areas a popup render pass returns for the host to apply after
/// the frame (kit renderers stay pure; `MouseState` lives in the app).
///
/// One popup draws per pass, so there is a single [`Self::popup_area`] slot —
/// the five per-popup fields this replaced (`code_popup_area` …) were a union
/// of which exactly one member was ever set, which is a struct pretending to
/// be five.
#[derive(Default, Clone)]
pub struct PopupMouseSurface {
    /// Outer rect of the popup this pass drew (empty when it drew nothing).
    pub popup_area: Rect,
    /// Selectable body rect inside the popup's border.
    pub body_area: Rect,
    /// One hit row per rendered body row, in screen order.
    pub hit_rows: Vec<crate::state::PopupHitRow>,
    /// Render-time cache write-back: the selection text the thinking popup
    /// computed for its active block (host applies it to the popup state).
    pub thinking_selection_text: Option<String>,
}
/// A single footer hint: key + label, e.g. ("↑/↓", "scroll"), ("Esc", "close").
pub struct FooterHint {
    pub key: &'static str,
    pub label: &'static str,
}

/// Centered popup geometry (80% of parent, minimum 40×10).
pub fn centered_popup_area(area: Rect) -> Rect {
    let popup_width = (area.width as f32 * 0.8).max(40.0) as u16;
    let popup_height = (area.height as f32 * 0.8).max(10.0) as u16;
    let popup_x = area.x + (area.width.saturating_sub(popup_width)) / 2;
    let popup_y = area.y + (area.height.saturating_sub(popup_height)) / 2;
    Rect::new(popup_x, popup_y, popup_width, popup_height)
}

/// Centered fixed-size list popup geometry.
pub fn centered_list_popup_area(area: Rect, width: u16, height: u16) -> Rect {
    let popup_width = width.min(area.width);
    let popup_height = height.min(area.height);
    let popup_x = area.x + (area.width.saturating_sub(popup_width)) / 2;
    let popup_y = area.y + (area.height.saturating_sub(popup_height)) / 2;
    Rect::new(popup_x, popup_y, popup_width, popup_height)
}

/// Inner content rect for a one-cell bordered block.
pub fn popup_inner(area: Rect) -> Rect {
    Rect::new(
        area.x + 1,
        area.y + 1,
        area.width.saturating_sub(2),
        area.height.saturating_sub(2),
    )
}

/// Draw a popup's vertical scrollbar down the right edge of `popup_area`.
///
/// Note the rect: the bar belongs on the popup's border column, which
/// [`popup_inner`] has already excluded from the body. Drawing it at the body's
/// right edge would sit it one column inside the border.
///
/// `viewport` is how many rows are visible and `offset` the first row drawn.
/// The caller owns both, deliberately: the writers that clamp to
/// `total - viewport` count *display* rows, while the diff popup counts source
/// lines and lets the last one reach the top. What is shared — and was written
/// out four times — is the bar's geometry and that `offset` must be the very
/// value the caller sliced its rows with, or the bar points somewhere the
/// content does not.
pub fn render_popup_scrollbar(
    frame: &mut Frame,
    popup_area: Rect,
    total: usize,
    viewport: usize,
    offset: usize,
) {
    let scrollbar =
        Scrollbar::default().orientation(ratatui::widgets::ScrollbarOrientation::VerticalRight);
    let mut state = ScrollbarState::new(total)
        .viewport_content_length(viewport)
        .position(offset);
    frame.render_stateful_widget(scrollbar, popup_area, &mut state);
}

/// Key that means "copy" in every popup footer. `render_popup_chrome` is the
/// one place that swaps it for the success confirmation.
const COPY_HINT_KEY: &str = "y";

/// The close affordance drawn at the end of a chrome popup's title row.
///
/// One definition, two consumers: the chrome that draws it and
/// [`title_close_suffix_width`], which `subagent_popup` uses to reserve room
/// for it before truncating a long title. They used to spell `"[x]"` twice and
/// could silently drift, clipping or over-truncating the title row.
pub const POPUP_CLOSE_MARKER: &str = "[x]";

/// Display width of the title-row close affordance — the separating space plus
/// [`POPUP_CLOSE_MARKER`], i.e. what `render_popup_chrome` appends after the
/// caller's title.
pub fn title_close_suffix_width() -> usize {
    1 + UnicodeWidthStr::width(POPUP_CLOSE_MARKER)
}

/// RN-style popup chrome: Clear + styled border block + title row + optional footer.
///
/// `footer_note` is free-form text drawn at the *front* of the bottom border,
/// before the key hints — used by the tool popups to print the raw tool id
/// (dynamic text, so it cannot live in the `&'static str` [`FooterHint`]s).
///
/// `copy_done` is the confirmation label (`✓ Copied`) to draw *in place of* the
/// [`COPY_HINT_KEY`] hint; the host passes it only while a copy is fresh, and
/// the label is rendered by the shared button component in its success state.
/// Returns the inner content Rect.
pub fn render_popup_chrome(
    frame: &mut Frame,
    popup_area: Rect,
    theme: &Theme,
    title: &str,
    footer_note: Option<&str>,
    footer: Option<&[FooterHint]>,
    copy_done: Option<&str>,
) -> Rect {
    frame.render_widget(Clear, popup_area);

    let title_spans = vec![
        Span::styled(
            title,
            Style::default().fg(theme.fg).add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(POPUP_CLOSE_MARKER, Style::default().fg(theme.muted)),
    ];

    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_type(theme.block_border_type())
        .border_style(Style::default().fg(theme.border))
        .title(Line::from(title_spans))
        .style(Style::default().bg(theme.bg));

    let hints = footer.unwrap_or(&[]);
    if footer_note.is_some() || !hints.is_empty() {
        let separator = || Span::styled(" | ", Style::default().fg(theme.muted));
        let mut footer_spans: Vec<Span<'_>> = Vec::new();
        if let Some(note) = footer_note {
            footer_spans.push(Span::styled(note, Style::default().fg(theme.muted)));
        }
        for hint in hints {
            if !footer_spans.is_empty() {
                footer_spans.push(separator());
            }
            if let Some(label) = copy_done
                && hint.key == COPY_HINT_KEY
            {
                let button =
                    Button::new(label, ButtonTheme::from_theme(theme)).state(ButtonState::Success);
                footer_spans.extend(button.line().spans);
                continue;
            }
            footer_spans.push(Span::styled(hint.key, Style::default().fg(theme.accent)));
            footer_spans.push(Span::styled(hint.label, Style::default().fg(theme.muted)));
        }
        block = block.title_bottom(Line::from(footer_spans).alignment(Alignment::Center));
    }

    frame.render_widget(block, popup_area);
    popup_inner(popup_area)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn centered_popup_uses_eighty_percent() {
        let parent = Rect::new(0, 0, 100, 50);
        let popup = centered_popup_area(parent);
        assert_eq!(popup.width, 80);
        assert_eq!(popup.height, 40);
        assert_eq!(popup.x, 10);
        assert_eq!(popup.y, 5);
    }

    #[test]
    fn centered_popup_enforces_minimum() {
        let parent = Rect::new(0, 0, 20, 6);
        let popup = centered_popup_area(parent);
        assert_eq!(popup.width, 40, "min width floor");
        assert_eq!(popup.height, 10, "min height floor");
    }

    #[test]
    fn list_popup_area_is_centered() {
        let parent = Rect::new(0, 0, 100, 40);
        let popup = centered_list_popup_area(parent, 48, 12);
        assert_eq!(popup.width, 48);
        assert_eq!(popup.height, 12);
        assert_eq!(popup.x, 26);
        assert_eq!(popup.y, 14);
    }

    /// The chrome draws `" "` + [`POPUP_CLOSE_MARKER`], and `subagent_popup`
    /// reserves [`title_close_suffix_width`] for it. If either side grows the
    /// suffix without the other, the subagent title row clips or over-truncates:
    /// this is the test that fails first.
    #[test]
    fn title_close_suffix_width_matches_what_the_chrome_draws() {
        let drawn = format!(" {POPUP_CLOSE_MARKER}");
        assert_eq!(
            title_close_suffix_width(),
            UnicodeWidthStr::width(drawn.as_str())
        );
    }
}
