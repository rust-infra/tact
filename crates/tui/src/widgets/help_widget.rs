use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph, Widget},
};

use crate::{i18n::Messages, theme::Theme};

/// Help widget, showing help text.
pub struct HelpWidget<'a> {
    msgs: &'a Messages,
    theme: &'a Theme,
}

impl<'a> Widget for HelpWidget<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let msgs = self.msgs;
        let header_style = Style::default()
            .fg(Color::Rgb(140, 170, 220))
            .add_modifier(Modifier::BOLD);
        let normal_style = Style::default().fg(self.theme.fg);
        let dim_style = Style::default().fg(Color::Rgb(120, 120, 140));

        let help_text = vec![
            // ── Main header ──
            Line::from(Span::styled(msgs.help_header_shortcuts, header_style)),
            Line::from(""),
            // ── Normal Mode ──
            Line::from(Span::styled(msgs.help_normal_header, dim_style)),
            Line::from(Span::styled(msgs.help_tab, normal_style)),
            Line::from(Span::styled(msgs.help_e, normal_style)),
            Line::from(Span::styled(msgs.help_jk, normal_style)),
            Line::from(Span::styled(msgs.help_gg, normal_style)),
            Line::from(Span::styled(msgs.help_G, normal_style)),
            Line::from(Span::styled(msgs.help_y, normal_style)),
            Line::from(Span::styled(msgs.help_t, normal_style)),
            Line::from(Span::styled(msgs.help_colon, normal_style)),
            Line::from(""),
            // ── Insert Mode ──
            Line::from(Span::styled(msgs.help_insert_header, dim_style)),
            Line::from(Span::styled(msgs.help_type_task, normal_style)),
            Line::from(Span::styled(msgs.help_ctrl_z, normal_style)),
            Line::from(""),
            // ── Global ──
            Line::from(Span::styled(msgs.help_global_header, dim_style)),
            Line::from(Span::styled(msgs.help_yn, normal_style)),
            Line::from(Span::styled(msgs.help_ctrl_h, normal_style)),
            Line::from(Span::styled(msgs.help_ctrl_t, normal_style)),
            Line::from(Span::styled(msgs.help_ctrl_l, normal_style)),
            Line::from(Span::styled(msgs.help_ctrl_qmark, normal_style)),
            Line::from(Span::styled(msgs.help_q, normal_style)),
            Line::from(""),
            // ── Mouse ──
            Line::from(Span::styled(msgs.help_mouse_header, dim_style)),
            Line::from(Span::styled(msgs.help_click_drag, normal_style)),
            Line::from(Span::styled(msgs.help_scroll, normal_style)),
            Line::from(Span::styled(msgs.help_y_copy, normal_style)),
        ];
        let para = Paragraph::new(help_text)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .title(msgs.help_title),
            )
            .style(Style::default().fg(self.theme.fg).bg(self.theme.bg));
        para.render(area, buf);
    }
}

impl<'a> HelpWidget<'a> {
    pub fn new(msgs: &'a Messages, theme: &'a Theme) -> Self {
        HelpWidget { msgs, theme }
    }
}
