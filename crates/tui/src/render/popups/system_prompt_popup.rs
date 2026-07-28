use ratatui::{
    Frame,
    layout::Rect,
    text::Text,
    widgets::{Paragraph, Scrollbar, ScrollbarState, Wrap},
};

use crate::widgets::state::App;

pub(crate) fn render_system_prompt_popup(frame: &mut Frame, area: Rect, app: &mut App) {
    let Some(popup) = app.system_prompt_popup.as_ref() else {
        return;
    };
    let popup_area = super::centered_popup_area(area);
    let inner = super::render_popup_chrome(
        frame,
        popup_area,
        &app.theme,
        &format!(" {} ", popup.title),
        None,
    );
    let lines = popup.rendered.clone();
    let total = lines.len().max(1);
    let content_height = inner.height as usize;
    let scroll = (popup.scroll as usize).min(total.saturating_sub(1));
    let text = Text::from(lines);
    let paragraph = Paragraph::new(text)
        .scroll((scroll as u16, 0))
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, inner);
    let scrollbar =
        Scrollbar::default().orientation(ratatui::widgets::ScrollbarOrientation::VerticalRight);
    let mut scrollbar_state = ScrollbarState::new(total)
        .viewport_content_length(content_height)
        .position(scroll);
    frame.render_stateful_widget(scrollbar, popup_area, &mut scrollbar_state);
}
