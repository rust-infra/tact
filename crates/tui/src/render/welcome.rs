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
