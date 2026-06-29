pub(crate) mod code_popup;
pub(crate) mod command_palette;
pub(crate) mod diff_popup;
pub(crate) mod slash_command;
pub(crate) mod file_picker;
pub(crate) mod help;
pub(crate) mod history;
pub(crate) mod select;
pub(crate) mod thinking_popup;

use ratatui::{
    Frame,
    layout::Rect,
};

/// Centered popup geometry (80% of parent, minimum 40×10).
pub(crate) fn centered_popup_area(area: Rect) -> Rect {
    let popup_width = (area.width as f32 * 0.8).max(40.0) as u16;
    let popup_height = (area.height as f32 * 0.8).max(10.0) as u16;
    let popup_x = area.x + (area.width.saturating_sub(popup_width)) / 2;
    let popup_y = area.y + (area.height.saturating_sub(popup_height)) / 2;
    Rect::new(popup_x, popup_y, popup_width, popup_height)
}

/// Popup shadow rendering is intentionally disabled to avoid visible right/bottom
/// dark bands in terminal themes with low contrast.
pub(crate) fn render_popup_shadow(_frame: &mut Frame, _popup_area: Rect) {
}
