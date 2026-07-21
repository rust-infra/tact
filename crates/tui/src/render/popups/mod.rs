pub(crate) mod code_popup;
pub(crate) mod command_palette;
pub(crate) mod diff_popup;
pub(crate) mod file_picker;
pub(crate) mod help;
pub(crate) mod history;
pub(crate) mod select;
pub(crate) mod selectable_text;
pub(crate) mod slash_command;
pub(crate) mod system_prompt_popup;
pub(crate) mod thinking_popup;

use ratatui::{
    Frame,
    layout::Rect,
    style::Style,
    widgets::{Block, BorderType, Borders, Clear},
};

/// Centered popup geometry (80% of parent, minimum 40×10).
pub(crate) fn centered_popup_area(area: Rect) -> Rect {
    let popup_width = (area.width as f32 * 0.8).max(40.0) as u16;
    let popup_height = (area.height as f32 * 0.8).max(10.0) as u16;
    let popup_x = area.x + (area.width.saturating_sub(popup_width)) / 2;
    let popup_y = area.y + (area.height.saturating_sub(popup_height)) / 2;
    Rect::new(popup_x, popup_y, popup_width, popup_height)
}

/// Centered fixed-size list popup geometry.
pub(crate) fn centered_list_popup_area(area: Rect, width: u16, height: u16) -> Rect {
    let popup_width = width.min(area.width);
    let popup_height = height.min(area.height);
    let popup_x = area.x + (area.width.saturating_sub(popup_width)) / 2;
    let popup_y = area.y + (area.height.saturating_sub(popup_height)) / 2;
    Rect::new(popup_x, popup_y, popup_width, popup_height)
}

/// Inner content rect for a one-cell bordered block.
pub(crate) fn popup_inner(area: Rect) -> Rect {
    Rect::new(area.x + 1, area.y + 1, area.width.saturating_sub(2), area.height.saturating_sub(2))
}

/// Clear + bordered frame for a list-style popup; returns the inner content area.
pub(crate) fn render_list_popup_chrome(
    frame: &mut Frame,
    popup_area: Rect,
    title: impl Into<ratatui::text::Line<'static>>,
    border_type: BorderType,
    bg: ratatui::style::Color,
) -> Rect {
    frame.render_widget(Clear, popup_area);
    let block =
        Block::default().borders(Borders::ALL).border_type(border_type).title(title).style(Style::default().bg(bg));
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
