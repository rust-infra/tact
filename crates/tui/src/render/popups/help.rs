use ratatui::{Frame, layout::Rect, style::{Color, Style}, text::Line, widgets::{Block, Borders, Paragraph}};
use crate::state::App;

pub(crate) fn render_help_panel(frame: &mut Frame, area: Rect, app: &mut App) {
    let msgs = app.msgs();
    let help_text = vec![
        Line::from(msgs.help_header_shortcuts),
        Line::from(""),
        Line::from(msgs.help_normal_header),
        Line::from(msgs.help_tab),
        Line::from(msgs.help_e),
        Line::from(msgs.help_jk),
        Line::from(msgs.help_gg),
        Line::from(msgs.help_G),
        Line::from(msgs.help_y),
        Line::from(msgs.help_t),
        Line::from(msgs.help_slash),
        Line::from(msgs.help_nN),
        Line::from(msgs.help_colon),
        Line::from(""),
        Line::from(msgs.help_insert_header),
        Line::from(msgs.help_type_task),
        Line::from(msgs.help_ctrl_z),
        Line::from(""),
        Line::from(msgs.help_global_header),
        Line::from(msgs.help_yn),
        Line::from(msgs.help_ctrl_h),
        Line::from(msgs.help_ctrl_t),
        Line::from(msgs.help_ctrl_l),
        Line::from(msgs.help_ctrl_qmark),
        Line::from(msgs.help_q),
        Line::from(""),
        Line::from(msgs.help_mouse_header),
        Line::from(msgs.help_click_drag),
        Line::from(msgs.help_scroll),
        Line::from(msgs.help_y_copy),
    ];
    let para = Paragraph::new(help_text)
        .block(Block::default().borders(Borders::ALL).title(app.msgs().help_title))
        .style(Style::default().fg(app.theme.fg).bg(app.theme.bg));
    frame.render_widget(para, area);
}

// ── Overlay popups ──

