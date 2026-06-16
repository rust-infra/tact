use crate::state::App;
use crate::widgets::popup_widget::PopupWidget;
use ratatui::style::Style;
use ratatui::widgets::ListItem;
use ratatui::{Frame, layout::Rect};


pub(crate) fn render_history_panel(frame: &mut Frame, area: Rect, app: &App) {
    let items: Vec<ListItem> = app
        .task_history
        .iter()
        .rev()
        .map(|entry| {
            let mut text = format!("[{}] {}", entry.timestamp, entry.task);
            if !entry.summary.is_empty() {
                text.push_str(&format!(" -> {}", entry.summary));
            }
            ListItem::new(text).style(Style::default().fg(app.theme.accent))
        })
        .collect();
    let widget = PopupWidget::default()
        .with_list(items)
        .with_theme(&app.theme)
        .with_title(app.msgs().history_title);
    frame.render_widget(widget, area);
}
