use crate::state::{App, FocusedPanel, InputMode, Status};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    widgets::Paragraph,
};

/// Render the bottom bar, showing focused panel, shortcut hints, working directory, Git branch,
/// Model info, token stats, task elapsed time, TUI uptime, and account balance.
pub(crate) fn render_bottom_bar(frame: &mut Frame, area: Rect, app: &App) {
    let msgs = app.msgs();
    let focus = match app.focused_panel {
        FocusedPanel::Plan => msgs.bottom_focus_log_plan,
        FocusedPanel::Log => msgs.bottom_focus_log,
    };
    let _tips = match app.focused_panel {
        FocusedPanel::Log => msgs.bottom_tips_log,
        FocusedPanel::Plan => msgs.bottom_tips_plan,
    };
    let branch = if app.status_bar.git_branch.is_empty() {
        msgs.bottom_branch_unknown
    } else {
        &app.status_bar.git_branch
    };
    let model = if app.status_bar.model_name.is_empty() {
        msgs.bottom_model_unknown.to_string()
    } else {
        let mut info = app.status_bar.model_name.clone();
        if app.status_bar.model_max_tokens > 0 {
            info.push_str(&format!(" | max={}", app.status_bar.model_max_tokens));
        }
        if let Some(budget) = app.status_bar.model_thinking_budget {
            info.push_str(&format!(" | think={budget}"));
        }
        info
    };
    let elapsed = app
        .task_start_time
        .map(|t| {
            let dur = chrono::Local::now().signed_duration_since(t);
            let secs = dur.num_seconds().max(0);
            format!("{:02}:{:02}", secs / 60, secs % 60)
        })
        .unwrap_or_else(|| "--:--".to_string());

    let uptime = {
        let dur = chrono::Local::now().signed_duration_since(app.process_start_time);
        let secs = dur.num_seconds().max(0) as u64;
        let d = secs / 86400;
        let h = (secs % 86400) / 3600;
        let m = (secs % 3600) / 60;
        let s = secs % 60;
        if d > 0 {
            format!("{}d {:02}:{:02}:{:02}", d, h, m, s)
        } else if h > 0 {
            format!("{}:{:02}:{:02}", h, m, s)
        } else {
            format!("{:02}:{:02}", m, s)
        }
    };

    {
        let has_balance = app.balance_info.is_some();
        let constraints: Vec<Constraint> = if has_balance {
            vec![
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(1),
            ]
        } else {
            vec![Constraint::Length(1), Constraint::Length(1)]
        };
        let areas = Layout::default()
            .direction(Direction::Vertical)
            .constraints(constraints)
            .split(area);
        let top_area = areas[0];
        let mid_area = areas[1];
        let top_text = msgs
            .bottom_top_tmpl
            .replacen("{}", focus, 1)
            //.replacen("{}", tips, 1)
            .replacen("{}", &app.workspace_dir, 1)
            .replacen("{}", branch, 1);
        let cache_str = if app.status_bar.token_cache_hit > 0 {
            msgs.bottom_cache_hit
                .replace("{}", &app.status_bar.token_cache_hit.to_string())
        } else {
            String::new()
        };
        let mid_text = msgs
            .bottom_mid_tmpl
            .replacen("{}", &model, 1)
            .replacen("{}", &app.status_bar.token_total.to_string(), 1)
            .replacen("{}", &app.status_bar.token_prompt.to_string(), 1)
            .replacen("{}", &app.status_bar.token_completion.to_string(), 1)
            .replacen("{}", &cache_str, 1)
            .replacen("{}", &elapsed, 1)
            .replacen("{}", &uptime, 1);
        let style = Style::default()
            .bg(app.theme.bottom_bar_bg)
            .fg(app.theme.bottom_bar_fg);
        let bar1 = Paragraph::new(top_text).style(style);
        let bar2 = Paragraph::new(mid_text).style(style);
        frame.render_widget(bar1, top_area);
        frame.render_widget(bar2, mid_area);

        if let Some(bi) = &app.balance_info {
            let bottom_area = areas[2];
            let status = if bi.is_available {
                msgs.bottom_balance_ok
            } else {
                msgs.bottom_balance_err
            };
            let entries: String = bi
                .balance_infos
                .iter()
                .map(|e| {
                    format!(
                        " {}:total={} grant={} topup={}",
                        e.currency, e.total_balance, e.granted_balance, e.topped_up_balance
                    )
                })
                .collect::<Vec<_>>()
                .join(" |");
            let balance_text = msgs
                .bottom_balance_tmpl
                .replacen("{}", status, 1)
                .replacen("{}", &entries, 1);
            let bar3 = Paragraph::new(balance_text).style(style);
            frame.render_widget(bar3, bottom_area);
        }
    }
}

/// Render the top status bar, showing current mode, focused panel, and Agent execution state.
pub(crate) fn render_status_bar(frame: &mut Frame, area: Rect, app: &App) {
    let msgs = app.msgs();
    let mode_indicator = match app.input_mode {
        InputMode::Normal => msgs.mode_normal,
        InputMode::Insert => msgs.mode_insert,
        InputMode::Search => msgs.mode_search,
        InputMode::Palette => msgs.mode_palette,
        InputMode::Select => msgs.mode_select,
    };
    let focus_str = match app.focused_panel {
        FocusedPanel::Plan => msgs.focus_plan,
        FocusedPanel::Log => msgs.focus_log,
    };
    let (status_text, status_style) = match &app.status {
        Status::Idle => {
            let theme_label = match app.theme.name {
                crate::theme::ThemeName::Dark => msgs.theme_dark,
                crate::theme::ThemeName::Light => msgs.theme_light,
                crate::theme::ThemeName::SolarizedDark => msgs.theme_solarized_dark,
                crate::theme::ThemeName::SolarizedLight => msgs.theme_solarized_light,
                crate::theme::ThemeName::GruvboxDark => msgs.theme_gruvbox_dark,
                crate::theme::ThemeName::Nord => msgs.theme_nord,
                crate::theme::ThemeName::Retro => msgs.theme_retro,
                crate::theme::ThemeName::Kawaii => msgs.theme_kawaii,
                crate::theme::ThemeName::Japanese => msgs.theme_japanese,
            };
            let lang_label = app.language.label();
            (
                msgs.status_idle_tmpl
                    .replacen("{}", mode_indicator, 1)
                    .replacen("{}", focus_str, 1)
                    .replacen("{}", theme_label, 1)
                    .replacen("{}", lang_label, 1),
                Style::default()
                    .bg(app.theme.status_bar_bg)
                    .fg(app.theme.fg),
            )
        }
        Status::Planning => (
            format!(
                "{} {} | {}",
                mode_indicator, focus_str, msgs.status_planning
            ),
            Style::default()
                .bg(app.theme.status_bar_bg)
                .fg(app.theme.accent),
        ),
        Status::Executing {
            current_step,
            total,
        } => (
            format!(
                "{} {} | {}",
                mode_indicator,
                focus_str,
                msgs.status_executing_tmpl
                    .replacen("{}", &(current_step + 1).to_string(), 1)
                    .replacen("{}", &total.to_string(), 1)
            ),
            Style::default()
                .bg(app.theme.status_bar_bg)
                .fg(app.theme.warning),
        ),
        Status::WaitingForUser { prompt, .. } => (
            msgs.status_waiting_user_tmpl
                .replacen("{}", mode_indicator, 1)
                .replacen("{}", focus_str, 1)
                .replacen("{}", prompt, 1),
            Style::default()
                .bg(app.theme.status_bar_bg)
                .fg(app.theme.warning),
        ),
        Status::Done => (
            format!(
                "{} {} | {}",
                mode_indicator,
                focus_str,
                msgs.status_done_tmpl.replace("{}", "")
            ),
            Style::default()
                .bg(app.theme.success)
                .fg(app.theme.fg)
                .add_modifier(Modifier::BOLD),
        ),
    };
    let (display_text, display_style) = if app.party_mode {
        (
            msgs.status_party_tmpl.replace("{}", focus_str),
            Style::default()
                .bg(Color::Rgb(255, 105, 180))
                .fg(Color::Rgb(255, 255, 255))
                .add_modifier(Modifier::BOLD),
        )
    } else if let Some((ref msg, _)) = app.flash_msg {
        (
            format!("⚠ {}", msg),
            Style::default()
                .bg(app.theme.warning)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        (status_text, status_style)
    };
    let status_bar = Paragraph::new(display_text).style(display_style);
    frame.render_widget(status_bar, area);
}
