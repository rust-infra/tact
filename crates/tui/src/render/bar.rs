use crate::widgets::state::{App, FocusedPanel, InputMode, Status};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    widgets::Paragraph,
};

/// Spinner animation frames for typing/loading indicator.
const SPINNER_FRAMES: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

/// Progress bar width in cells.
const PROGRESS_BAR_WIDTH: u16 = 15;

fn format_mm_ss(total_secs: i64) -> String {
    let secs = total_secs.max(0);
    format!("{:02}:{:02}", secs / 60, secs % 60)
}

const USAGE_BAR_WIDTH: u16 = 10;

/// Render a text-based usage progress bar like `[█████░░░░░]`.
/// Format a quota number for display; `None` (no numeric cap) renders as `∞`.
fn format_quota_value(value: Option<f64>) -> String {
    match value {
        Some(v) => {
            if v.fract() == 0.0 {
                format!("{v:.0}")
            } else {
                format!("{v}")
            }
        }
        None => "∞".to_string(),
    }
}

fn render_usage_bar(pct: f64) -> String {
    let inner_width = USAGE_BAR_WIDTH.saturating_sub(2) as usize;
    let fill_chars = ((pct / 100.0) * inner_width as f64).round() as usize;
    let mut bar = String::from("[");
    for i in 0..inner_width {
        if i < fill_chars {
            bar.push('█');
        } else {
            bar.push('░');
        }
    }
    bar.push(']');
    bar
}

/// Compact token count for status display (`590`, `12.5K`, `200K`).
fn format_tokens_compact(n: u64) -> String {
    if n < 1_000 {
        n.to_string()
    } else if n < 1_000_000 {
        let k = n as f64 / 1_000.0;
        if (k * 10.0).round() % 10.0 == 0.0 {
            format!("{:.0}K", k)
        } else {
            format!("{:.1}K", k)
        }
    } else {
        let m = n as f64 / 1_000_000.0;
        if (m * 10.0).round() % 10.0 == 0.0 {
            format!("{:.0}M", m)
        } else {
            format!("{:.1}M", m)
        }
    }
}

/// Context usage vs model_context_window.
///
/// TODO: align closer to Codex (12K baseline / effective window %).
/// For now: last_token_usage.total_tokens / model_context_window.
fn context_usage_pct(used: u32, window: usize) -> u8 {
    if window == 0 {
        0
    } else {
        ((used as u128) * 100 / window as u128).min(100) as u8
    }
}

fn format_context_meter(used: u32, window: usize) -> String {
    let pct = context_usage_pct(used, window);
    let bar = render_usage_bar(pct as f64);
    format!(
        "{bar} {pct}% │ {}/{}",
        format_tokens_compact(used as u64),
        format_tokens_compact(window as u64)
    )
}

/// Render a text-based progress bar like `[█████░░░░░] 50%`
/// Uses a smooth formula: (current + 0.5) / total, so the current step
/// is treated as half-done. This avoids showing 0% on the first step
/// and 100% before the last step finishes.
fn render_progress_bar(current: usize, total: usize, _theme: &crate::theme::Theme) -> String {
    if total == 0 {
        return String::new();
    }
    // Smooth progress: current step is half-done
    let filled = ((current as f64 + 0.5) / total as f64).min(1.0);
    // PROGRESS_BAR_WIDTH - 2 for the '[' and ']'
    let inner_width = PROGRESS_BAR_WIDTH.saturating_sub(2) as usize;
    let fill_chars = (filled * inner_width as f64).round() as usize;
    let mut bar = String::from("[");
    for i in 0..inner_width {
        if i < fill_chars {
            bar.push('█');
        } else {
            bar.push('░');
        }
    }
    bar.push(']');
    let pct = (filled * 100.0).round() as u8;
    bar.push_str(&format!(" {}%", pct));
    bar
}

/// Format balance or usage quota for the bottom bar (appended to row 1).
fn format_account_suffix(app: &App) -> Option<String> {
    let msgs = app.msgs();
    app.account_rx.as_ref()?;
    if let Some(bi) = &app.account.balance {
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
                    " {}:total={:.2} grant={:.2} topup={:.2}",
                    e.currency, e.total_balance, e.granted_balance, e.topped_up_balance
                )
            })
            .collect::<Vec<_>>()
            .join(" |");
        return Some(
            msgs.bottom_balance_tmpl
                .replacen("{}", status, 1)
                .replacen("{}", &entries, 1),
        );
    }
    if let Some(quota) = &app.account.quota {
        let status = if quota.is_available {
            msgs.bottom_balance_ok
        } else {
            msgs.bottom_balance_err
        };
        let entries: String = quota
            .windows
            .iter()
            .map(|w| {
                let remaining = format_quota_value(w.remaining);
                let limit = format_quota_value(w.limit);
                if let Some(pct) = w.usage_pct() {
                    format!(
                        " {}:{:.0}% {} {}/{}",
                        w.label,
                        pct,
                        render_usage_bar(pct),
                        remaining,
                        limit
                    )
                } else {
                    format!(" {}:{}/{}", w.label, remaining, limit)
                }
            })
            .collect::<Vec<_>>()
            .join(" |");
        return Some(
            msgs.bottom_usage_tmpl
                .replacen("{}", status, 1)
                .replacen("{}", &entries, 1),
        );
    }
    None
}

/// Render the bottom bar, showing focused panel, task elapsed time, TUI uptime, working
/// directory, Git branch, model info, token stats, and account balance.
pub(crate) fn render_bottom_bar(frame: &mut Frame, area: Rect, app: &App) {
    frame.render_widget(ratatui::widgets::Clear, area);
    let msgs = app.msgs();
    let focus = match app.focused_panel {
        FocusedPanel::Plan => msgs.bottom_focus_log_plan,
        FocusedPanel::Log => msgs.bottom_focus_log,
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
            if let Some(effort) = app.status_bar.model_reasoning_effort.as_deref() {
                info.push_str(&format!(" | think={budget} ({effort})"));
            } else {
                info.push_str(&format!(" | think={budget}"));
            }
        }
        info
    };
    let elapsed = if let Some(start) = app.task_start_time {
        let secs = chrono::Local::now()
            .signed_duration_since(start)
            .num_seconds()
            .max(0);
        format_mm_ss(secs)
    } else if let Some(secs) = app.last_prompt_elapsed_secs {
        format_mm_ss(secs)
    } else {
        "--:--".to_string()
    };

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
        let row_count = area.height.max(1) as usize;
        let constraints: Vec<Constraint> = (0..row_count).map(|_| Constraint::Length(1)).collect();
        let areas = Layout::default()
            .direction(Direction::Vertical)
            .constraints(constraints)
            .split(area);
        let top_area = areas[0];
        let mid_area = areas.get(1).copied().unwrap_or(top_area);
        let mut top_text = msgs
            .bottom_top_tmpl
            .replacen("{}", focus, 1)
            .replacen("{}", &elapsed, 1)
            .replacen("{}", &uptime, 1)
            .replacen("{}", &app.workspace_dir, 1)
            .replacen("{}", branch, 1);
        if let Some(account) = format_account_suffix(app) {
            top_text.push_str(" │ ");
            top_text.push_str(&account);
        }
        let cache_str = if app.status_bar.token_total > 0
            || app.status_bar.token_cache_hit > 0
            || app.status_bar.token_cache_miss > 0
            || app.status_bar.token_reasoning > 0
        {
            let cache_total = app.status_bar.token_cache_hit + app.status_bar.token_cache_miss;
            let hit_pct = app
                .status_bar
                .token_cache_hit
                .saturating_mul(100)
                .checked_div(cache_total)
                .unwrap_or(0);
            msgs.bottom_cache_tmpl
                .replacen("{}", &hit_pct.to_string(), 1)
                .replacen("{}", &app.status_bar.token_reasoning.to_string(), 1)
        } else {
            String::new()
        };
        let meter = format_context_meter(
            app.status_bar.token_total,
            app.model_context_window,
        );
        let mid_text = msgs
            .bottom_mid_tmpl
            .replacen("{}", &model, 1)
            .replacen("{}", &meter, 1)
            .replacen("{}", &app.status_bar.token_total.to_string(), 1)
            .replacen("{}", &app.status_bar.token_prompt.to_string(), 1)
            .replacen("{}", &app.status_bar.token_completion.to_string(), 1)
            .replacen("{}", &cache_str, 1);
        let style = Style::default()
            .bg(app.theme.bottom_bar_bg)
            .fg(app.theme.bottom_bar_fg);
        let bar1 = Paragraph::new(top_text).style(style);
        let bar2 = Paragraph::new(mid_text).style(style);
        frame.render_widget(bar1, top_area);
        frame.render_widget(bar2, mid_area);
    }
}

/// Render the top status bar, showing current mode, focused panel, and Agent execution state.
pub(crate) fn render_status_bar(frame: &mut Frame, area: Rect, app: &App) {
    let msgs = app.msgs();

    // Mode indicator with emoji
    let (mode_emoji, mode_indicator) = match app.input_mode {
        InputMode::Normal => ("◆", msgs.mode_normal),
        InputMode::Insert => ("◇", msgs.mode_insert),
        InputMode::Palette => ("⚡", msgs.mode_palette),
        InputMode::Select => ("▣", msgs.mode_select),
        InputMode::FilePicker => ("📎", msgs.mode_file_picker),
    };
    let mode_str = format!("{} {}", mode_emoji, mode_indicator);

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
                crate::theme::ThemeName::Brutal => msgs.theme_brutal,
            };
            let lang_label = app.language.label();
            (
                msgs.status_idle_tmpl
                    .replacen("{}", &mode_str, 1)
                    .replacen("{}", focus_str, 1)
                    .replacen("{}", theme_label, 1)
                    .replacen("{}", lang_label, 1),
                Style::default()
                    .bg(app.theme.status_bar_bg)
                    .fg(app.theme.fg),
            )
        }
        Status::Planning => {
            let spinner = SPINNER_FRAMES[app.spinner_frame as usize];
            (
                format!(
                    "{} {} │ {} {}",
                    mode_str, focus_str, spinner, msgs.status_planning
                ),
                Style::default()
                    .bg(app.theme.status_bar_bg)
                    .fg(app.theme.accent),
            )
        }
        Status::Executing {
            current_step: _,
            total,
        } => {
            let spinner = SPINNER_FRAMES[app.spinner_frame as usize];
            // With parallel tools, `current_step` is no longer a reliable UI
            // progress anchor. Derive progress from completed + active steps.
            let completed = app
                .plan
                .steps
                .iter()
                .filter(|s| s.output.as_ref().is_some())
                .count()
                .min(*total);
            let running = app.tools.active.len();
            let display_step = if *total == 0 {
                0
            } else if running > 0 {
                (completed + 1).min(*total)
            } else {
                completed.max(1).min(*total)
            };
            let step_label = msgs
                .status_executing_tmpl
                .replacen("{}", &display_step.to_string(), 1)
                .replacen("{}", &total.to_string(), 1);
            let running_label = msgs
                .status_running_tmpl
                .replacen("{}", &running.to_string(), 1);
            // Smooth progress: treat the current step as half-done so the bar
            // never shows 0% (we're actively working) nor 100% (not done yet).
            // Formula: (current_step + 0.5) / total
            //   1 step:  0.5/1 = 50%
            //   3-step step 0: 0.5/3 ≈ 17%
            //   3-step step 1: 1.5/3 = 50%
            //   3-step step 2: 2.5/3 ≈ 83%
            let progress_idx = if *total == 0 {
                0
            } else {
                completed.min(total.saturating_sub(1))
            };
            let progress_bar = render_progress_bar(progress_idx, *total, &app.theme);
            let exec_right = if running > 0 {
                format!("{} │ {} {}", step_label, running_label, progress_bar)
            } else {
                format!("{} {}", step_label, progress_bar)
            };
            (
                format!("{} {} │ {} {}", mode_str, focus_str, spinner, exec_right),
                Style::default()
                    .bg(app.theme.status_bar_bg)
                    .fg(app.theme.warning),
            )
        }
        Status::Done => (
            format!(
                "{} {} │ ✅ {}",
                mode_str,
                focus_str,
                msgs.status_done_tmpl.replace("{}", "")
            ),
            Style::default()
                .bg(app.theme.success)
                .fg(app.theme.fg)
                .add_modifier(Modifier::BOLD),
        ),
    };
    let (display_text, display_style) = if let Some((ref msg, _)) = app.flash_msg {
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

#[cfg(test)]
mod render_tests {
    use super::super::test_harness::{buffer_text, make_app, render_app_text};
    use super::render_bottom_bar;
    use super::render_usage_bar;
    use ratatui::{Terminal, backend::TestBackend, layout::Rect};
    use tact_protocol::{BalanceEntry, BalanceInfo};

    #[test]
    fn render_usage_bar_scales_to_width() {
        assert_eq!(render_usage_bar(0.0), "[░░░░░░░░]");
        assert_eq!(render_usage_bar(50.0), "[████░░░░]");
        assert_eq!(render_usage_bar(100.0), "[████████]");
    }

    #[test]
    fn bottom_bar_shows_balance_row_when_available() {
        let (_tx, account_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = make_app();
        app.account_rx = Some(account_rx);
        app.account.balance = Some(BalanceInfo {
            is_available: true,
            balance_infos: vec![BalanceEntry {
                currency: "USD".into(),
                total_balance: 12.50,
                granted_balance: 10.00,
                topped_up_balance: 2.50,
            }],
        });

        let text = render_app_text(&mut app, 120, 12);
        assert!(
            text.contains("12.50") || text.contains("USD"),
            "balance should append on bottom bar row 1, got:\n{text}"
        );
    }

    #[test]
    fn bottom_bar_renders_without_panic_when_idle() {
        let app = make_app();
        let backend = TestBackend::new(100, 2);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| render_bottom_bar(frame, Rect::new(0, 0, 100, 2), &app))
            .expect("draw");
        assert!(!buffer_text(terminal.backend().buffer()).trim().is_empty());
    }

    #[test]
    fn bottom_bar_shows_context_usage_meter_on_row_2() {
        let mut app = make_app();
        app.model_context_window = 200_000;
        app.status_bar.model_name = "mock-model".into();
        app.status_bar.token_total = 590;
        app.status_bar.token_prompt = 400;
        app.status_bar.token_completion = 190;

        let backend = TestBackend::new(160, 2);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| render_bottom_bar(frame, Rect::new(0, 0, 160, 2), &app))
            .expect("draw");

        let text = buffer_text(terminal.backend().buffer());
        let lines: Vec<&str> = text.lines().collect();
        assert!(lines.len() >= 2, "expected 2 rows, got:\n{text}");
        let row2 = lines[1];
        assert!(
            row2.contains("mock-model") && row2.contains("590/200K") && row2.contains("%"),
            "row 2 should show model + meter + ratio, got:\n{row2}"
        );
        assert!(
            row2.contains('[') && row2.contains(']'),
            "row 2 should include progress bar brackets, got:\n{row2}"
        );
    }

    #[test]
    fn bottom_bar_shows_elapsed_and_uptime_on_row_1() {
        let mut app = make_app();
        app.last_prompt_elapsed_secs = Some(65); // 01:05
        app.status_bar.model_name = "mock-model".into();
        app.status_bar.token_total = 42;
        app.workspace_dir = "/tmp/tact-ws".into();
        app.status_bar.git_branch = "main".into();

        let backend = TestBackend::new(140, 2);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| render_bottom_bar(frame, Rect::new(0, 0, 140, 2), &app))
            .expect("draw");

        let text = buffer_text(terminal.backend().buffer());
        let lines: Vec<&str> = text.lines().collect();
        assert!(
            lines.len() >= 2,
            "bottom bar should render two rows, got:\n{text}"
        );
        let row1 = lines[0];
        let row2 = lines[1];
        assert!(
            row1.contains("Elapsed:01:05") && row1.contains("Up:"),
            "elapsed and uptime should be on row 1, got:\n{row1}"
        );
        assert!(
            row1.contains("/tmp/tact-ws") && row1.contains("main"),
            "cwd and branch should remain on row 1, got:\n{row1}"
        );
        assert!(
            !row2.contains("Elapsed:") && !row2.contains("Up:"),
            "elapsed/uptime must not appear on row 2, got:\n{row2}"
        );
        assert!(
            row2.contains("Tok:42"),
            "token stats should stay on row 2, got:\n{row2}"
        );
    }

    #[test]
    fn bottom_bar_shows_reasoning_effort_with_thinking_budget() {
        let mut app = make_app();
        app.status_bar.model_name = "mock-model".into();
        app.status_bar.model_thinking_budget = Some(32_000);
        app.status_bar.model_reasoning_effort = Some("high".into());
        let backend = TestBackend::new(120, 2);
        let mut terminal = Terminal::new(backend).expect("terminal");

        terminal
            .draw(|frame| render_bottom_bar(frame, Rect::new(0, 0, 120, 2), &app))
            .expect("draw");

        let text = buffer_text(terminal.backend().buffer());
        assert!(
            text.contains("think=32000 (high)"),
            "bottom bar should show budget and effort, got:\n{text}"
        );
    }

    #[test]
    fn bottom_bar_preserves_thinking_budget_when_effort_is_unavailable() {
        let mut app = make_app();
        app.status_bar.model_name = "mock-model".into();
        app.status_bar.model_thinking_budget = Some(32_000);
        let backend = TestBackend::new(120, 2);
        let mut terminal = Terminal::new(backend).expect("terminal");

        terminal
            .draw(|frame| render_bottom_bar(frame, Rect::new(0, 0, 120, 2), &app))
            .expect("draw");

        let text = buffer_text(terminal.backend().buffer());
        assert!(
            text.contains("think=32000"),
            "bottom bar should preserve the budget display, got:\n{text}"
        );
        assert!(
            !text.contains("think=32000 ("),
            "bottom bar should omit a missing effort, got:\n{text}"
        );
    }
}
