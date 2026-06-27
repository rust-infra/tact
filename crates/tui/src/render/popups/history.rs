use crate::render::util::truncate_chars_with_ellipsis;
use crate::widgets::state::App;
use crate::widgets::popup_widget::PopupWidget;
use ratatui::{
    style::{Color, Style},
    text::Span,
    widgets::ListItem,
    Frame, layout::Rect,
};


pub(crate) fn render_history_panel(frame: &mut Frame, area: Rect, app: &App) {
    let count = app.task_history.len();
    let items: Vec<ListItem> = app
        .task_history
        .iter()
        .rev()
        .enumerate()
        .map(|(idx, entry)| {
            let is_last = idx == count.saturating_sub(1);
            let branch = if is_last { "╰──" } else { "├──" };

            let icon = if entry.summary.contains("✅") || entry.summary.contains("✓") {
                "✅"
            } else if entry.summary.contains("❌")
                || entry.summary.contains("✗")
                || entry.summary.contains("Error")
            {
                "❌"
            } else {
                "🔄"
            };

            let task_preview = truncate_chars_with_ellipsis(&entry.task, 40);

            let mut text = format!(
                " {} {} [{}] {}",
                branch, icon, entry.timestamp, task_preview
            );
            if !entry.summary.is_empty() {
                text.push_str(" → ");
                let summary_short = truncate_chars_with_ellipsis(&entry.summary, 30);
                text.push_str(&summary_short);
            }

            let line_color = if entry.summary.contains("❌") || entry.summary.contains("✗") {
                Color::Rgb(220, 80, 80)
            } else if entry.summary.contains("✅") || entry.summary.contains("✓") {
                Color::Rgb(80, 200, 120)
            } else {
                app.theme.accent
            };

            ListItem::new(Span::styled(text, Style::default().fg(line_color)))
        })
        .collect();
    let widget = PopupWidget::default()
        .with_list(items)
        .with_theme(&app.theme)
        .with_title(app.msgs().history_title);
    frame.render_widget(widget, area);
}
