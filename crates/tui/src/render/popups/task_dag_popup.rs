//! `/tasks-dag` scrollable Mermaid popup (rendered via ratatui-markdown).

use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Paragraph, Scrollbar, ScrollbarState, Wrap},
};

use crate::widgets::state::{App, render_task_dag_lines};

pub(crate) fn render_task_dag_popup(frame: &mut Frame, area: Rect, app: &mut App) {
    let popup_area = super::centered_popup_area(area);
    let footer: &[super::FooterHint] = &[
        super::FooterHint {
            key: "y",
            label: " copy ",
        },
        super::FooterHint {
            key: "j/k",
            label: " scroll ",
        },
        super::FooterHint {
            key: "Esc",
            label: " close ",
        },
    ];
    let inner =
        super::render_popup_chrome(frame, popup_area, &app.theme, " tasks-dag ", Some(footer));

    // The mermaid layout depends on width; re-render when the popup width changes.
    let width = inner.width as usize;
    if app
        .task_dag_popup
        .as_ref()
        .is_some_and(|p| p.render_width != width)
    {
        let (source, lines) = render_task_dag_lines(&app.task_panel().snapshot, &app.theme, width);
        if let Some(p) = app.task_dag_popup.as_mut() {
            p.lines = lines;
            p.mermaid_source = source;
            p.render_width = width;
        }
    }

    let popup = match &app.task_dag_popup {
        Some(p) => p,
        None => return,
    };
    let total = popup.lines.len();
    if total == 0 {
        return;
    }

    let content_height = inner.height as usize;
    let max_scroll = total.saturating_sub(1);
    let scroll = (popup.scroll as usize).min(max_scroll);
    let start_line = scroll;
    let end_line = (scroll + content_height).min(total);

    let mut text = Text::default();
    text.push_line(Line::from(Span::styled(
        format!("Tasks DAG ({} lines)", total),
        Style::default()
            .fg(app.theme.accent)
            .add_modifier(Modifier::BOLD),
    )));
    text.push_line(Line::from(""));
    text.lines
        .extend(popup.lines[start_line..end_line].iter().cloned());

    let para = Paragraph::new(text).wrap(Wrap { trim: false });

    frame.render_widget(para, inner);

    let scrollbar =
        Scrollbar::default().orientation(ratatui::widgets::ScrollbarOrientation::VerticalRight);
    let mut state = ScrollbarState::new(total)
        .viewport_content_length(content_height)
        .position(scroll);
    frame.render_stateful_widget(scrollbar, popup_area, &mut state);

    app.mouse.task_dag_popup_area = popup_area;
}
