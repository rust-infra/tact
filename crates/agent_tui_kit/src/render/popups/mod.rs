//! Popup chrome helpers and popup renderers (pure).
//!
//! Moved from `crates/tui/src/render/popups` in the Ctx migration slice.
//! Each renderer takes `&RenderCtx` (or the popup's own state) and returns
//! mouse hit areas; the host applies them after the frame.

pub mod code_popup;
pub mod diff_popup;
pub mod history;
pub mod mermaid_popup;
pub mod select;
pub mod subagent_popup;
pub mod system_prompt_popup;
pub mod thinking_popup;

use ratatui::{
    Frame,
    layout::{Alignment, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear},
};

use crate::{
    theme::Theme,
    widgets::button::{Button, ButtonState, ButtonTheme},
};

/// Mouse hit areas a popup render pass returns for the host to apply after
/// the frame (kit renderers stay pure; `MouseState` lives in the app).
#[derive(Default, Clone)]
pub struct PopupMouseSurface {
    pub code_popup_area: Rect,
    pub mermaid_popup_area: Rect,
    pub thinking_popup_area: Rect,
    pub subagent_popup_area: Rect,
    pub diff_popup_area: Rect,
    pub body_area: Rect,
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

/// Key that means "copy" in every popup footer. `render_popup_chrome` is the
/// one place that swaps it for the success confirmation.
const COPY_HINT_KEY: &str = "y";

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
        Span::styled("[x]", Style::default().fg(theme.muted)),
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

/// Clear + bordered frame for a list-style popup; returns the inner content area.
pub fn render_list_popup_chrome(
    frame: &mut Frame,
    popup_area: Rect,
    title: impl Into<ratatui::text::Line<'static>>,
    border_type: BorderType,
    bg: ratatui::style::Color,
) -> Rect {
    frame.render_widget(Clear, popup_area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(border_type)
        .title(title)
        .style(Style::default().bg(bg));
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
}
