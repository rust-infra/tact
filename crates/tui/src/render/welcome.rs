use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Paragraph, Widget},
};

#[derive(Default)]
pub struct LogoWidget {}

impl Widget for LogoWidget {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let logo_lines = vec![
            Line::from(Span::styled(
                "  ████████╗",
                Style::default().fg(Color::Green),
            )),
            Line::from(Span::styled(
                "  ╚══██╔══╝",
                Style::default().fg(Color::Green),
            )),
            Line::from(Span::styled(
                "     ██║   ",
                Style::default().fg(Color::Green),
            )),
            Line::from(Span::styled(
                "     ██║   ",
                Style::default().fg(Color::Green),
            )),
            Line::from(Span::styled(
                "     ██║   ",
                Style::default().fg(Color::Green),
            )),
            Line::from(Span::styled(
                "     ╚═╝   ",
                Style::default().fg(Color::Green),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "  Agent TUI",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(ratatui::style::Modifier::BOLD),
            )),
        ];

        let h = logo_lines.len() as u16;
        let center_h = area.height.saturating_sub(h) / 2;
        let chunks = Layout::vertical([
            Constraint::Length(center_h),
            Constraint::Length(h),
            Constraint::Min(0),
        ])
        .split(area);

        Paragraph::new(logo_lines).centered().render(chunks[1], buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logo_widget_renders_agent_label() {
        let area = Rect::new(0, 0, 40, 20);
        let mut buf = Buffer::empty(area);
        LogoWidget::default().render(area, &mut buf);

        let mut text = String::new();
        for y in 0..area.height {
            for x in 0..area.width {
                text.push_str(buf[(x, y)].symbol());
            }
        }
        assert!(
            text.contains("Agent TUI"),
            "logo should render the Agent TUI label"
        );
    }

    #[test]
    fn logo_widget_does_not_panic_on_tiny_area() {
        let area = Rect::new(0, 0, 4, 2);
        let mut buf = Buffer::empty(area);
        LogoWidget::default().render(area, &mut buf);
    }
}
