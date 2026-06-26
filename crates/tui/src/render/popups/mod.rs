pub(crate) mod code_popup;
pub(crate) mod command_palette;
pub(crate) mod diff_popup;
pub(crate) mod file_picker;
pub(crate) mod help;
pub(crate) mod history;
pub(crate) mod select;
pub(crate) mod thinking_popup;

use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Style},
    widgets::Block,
};

/// Render a subtle shadow behind a popup for a 3D depth effect.
///
/// The shadow is rendered as a dark block offset by (2, 1) cells to the
/// right and down from the popup area. This creates a floating-window
/// appearance without needing true alpha blending.
pub(crate) fn render_popup_shadow(frame: &mut Frame, popup_area: Rect) {
    if popup_area.width == 0 || popup_area.height == 0 {
        return;
    }
    // Shadow offset: 2 right, 1 down
    let shadow_area = Rect::new(
        popup_area.x.saturating_add(2),
        popup_area.y.saturating_add(1),
        popup_area.width,
        popup_area.height,
    );
    // Semi-transparent dark fill for shadow
    let shadow = Block::default().style(Style::default().bg(Color::Rgb(15, 15, 28)));
    frame.render_widget(shadow, shadow_area);
}
