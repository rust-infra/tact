use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};
use tact_protocol::BalanceEntry;

use crate::widgets::state::{App, FocusedPanel, InputMode, Status};

/// Spinner animation frames for typing/loading indicator.
const SPINNER_FRAMES: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

/// Progress bar width in cells.
const PROGRESS_BAR_WIDTH: u16 = 15;

/// Bottom bar icons (language-invariant Unicode glyphs).
const ICON_ELAPSED: &str = "◷";
const ICON_UPTIME: &str = "⊙";
const ICON_BRANCH: &str = "⎇";
const ICON_BALANCE: &str = "¤";
const ICON_TOKENS: &str = "∑";
const ICON_CACHE: &str = "▣";

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

/// Compact model string: `"modelname 8k/32k"` or `"-"`.
fn format_model_compact(name: &str, max_tokens: u32, thinking_budget: Option<u32>) -> String {
    if name.is_empty() {
        return "-".to_string();
    }
    let mut s = name.to_string();
    if max_tokens > 0 {
        s.push(' ');
        s.push_str(&format_tokens_compact(max_tokens as u64));
        if let Some(budget) = thinking_budget.filter(|b| *b > 0) {
            s.push('/');
            s.push_str(&format_tokens_compact(budget as u64));
        }
    }
    s
}

/// Format one balance entry: `"¤ CNY 9.60"`.
fn format_balance_entry(entry: &BalanceEntry) -> String {
    format!(
        "{} {} {:.2}",
        ICON_BALANCE, entry.currency, entry.total_balance
    )
}

/// Format one quota window: `"¤ label 75%"` or `"¤ label 150/200"`.
fn format_quota_window(window: &tact_protocol::UsageQuotaWindow) -> String {
    let remaining = format_quota_value(window.remaining);
    let limit = format_quota_value(window.limit);
    if let Some(pct) = window.usage_pct() {
        format!("{} {} {:.0}%", ICON_BALANCE, window.label, pct)
    } else {
        format!("{} {} {}/{}", ICON_BALANCE, window.label, remaining, limit)
    }
}

/// Format cache hit percentage: `"▣45%"` or `"▣--"`.
fn format_cache_pct(hit: u64, miss: u64) -> String {
    let total = hit + miss;
    if total == 0 {
        format!("{}--", ICON_CACHE)
    } else {
        let pct = hit.saturating_mul(100) / total;
        format!("{}{}%", ICON_CACHE, pct)
    }
}

/// Context usage meter with ` · ` separator: `"[████░░░░] 0% · 6.6K/1M"`.
fn format_context_meter_new(used: u32, window: usize) -> String {
    let pct = context_usage_pct(used, window);
    let bar = render_usage_bar(pct as f64);
    format!(
        "{bar} {pct}% · {}/{}",
        format_tokens_compact(used as u64),
        format_tokens_compact(window as u64)
    )
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

/// Drop group: if `droppable`, the segment can be removed when space is tight.
struct DropGroup {
    droppable: bool,
    spans: Vec<Span<'static>>,
}

/// Compute total unicode width of concatenated Span text content.
fn group_total_width(groups: &[DropGroup]) -> u16 {
    groups
        .iter()
        .flat_map(|g| &g.spans)
        .map(|s| unicode_width::UnicodeWidthStr::width(s.content.as_ref()) as u16)
        .sum()
}

/// Remove droppable groups (from end) until total width ≤ target.
fn fit_row_spans(target: u16, groups: &mut Vec<DropGroup>) {
    while !groups.is_empty() && group_total_width(groups) > target {
        if let Some(pos) = groups.iter().rposition(|g| g.droppable) {
            groups.remove(pos);
        } else {
            break;
        }
    }
}

/// Render the bottom bar, showing focused panel, task elapsed time, TUI uptime, working
/// directory, Git branch, model info, token stats, and account balance.
pub(crate) fn render_bottom_bar(frame: &mut Frame, area: Rect, app: &App) {
    frame.render_widget(ratatui::widgets::Clear, area);

    let msgs = app.msgs();
    let theme = &app.theme;
    let dim = Style::default().fg(theme.muted_fg());
    let primary = Style::default().fg(theme.fg);
    let secondary = Style::default().fg(theme.bottom_bar_fg);
    let accent = Style::default().fg(theme.accent);

    let focus = match app.focused_panel {
        FocusedPanel::Log => "[Log]",
    };

    // --- Row 1 ---
    let branch = if app.status_bar.git_branch.is_empty() {
        msgs.bottom_branch_unknown
    } else {
        &app.status_bar.git_branch
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

    let mut row1_groups: Vec<DropGroup> = Vec::new();
    // Focus
    row1_groups.push(DropGroup {
        droppable: false,
        spans: vec![
            Span::styled(focus.to_string(), primary),
            Span::styled(" · ", dim),
        ],
    });
    // Elapsed: ◷ MM:SS
    row1_groups.push(DropGroup {
        droppable: false,
        spans: vec![
            Span::styled(ICON_ELAPSED.to_string(), dim),
            Span::styled(format!(" {} · ", elapsed), primary),
        ],
    });
    // Path (droppable)
    row1_groups.push(DropGroup {
        droppable: true,
        spans: vec![Span::styled(format!("{} · ", app.workspace_dir), secondary)],
    });
    // Uptime: ⊙ HH:MM:SS (droppable — dropped before path per spec)
    row1_groups.push(DropGroup {
        droppable: true,
        spans: vec![Span::styled(
            format!("{} {} · ", ICON_UPTIME, uptime),
            secondary,
        )],
    });
    // Branch: ⎇ branchname
    row1_groups.push(DropGroup {
        droppable: false,
        spans: vec![
            Span::styled(ICON_BRANCH.to_string(), dim),
            Span::styled(format!(" {}", branch), accent),
        ],
    });
    // Account (if present, never dropped)
    if let Some(acct_spans) = build_account_spans(app, theme) {
        row1_groups.push(DropGroup {
            droppable: false,
            spans: vec![Span::styled(" · ", dim)],
        });
        row1_groups.push(DropGroup {
            droppable: false,
            spans: acct_spans,
        });
    }

    // --- Row 2 ---
    let model = format_model_compact(
        &app.status_bar.model_name,
        app.status_bar.model_max_tokens,
        app.status_bar.model_thinking_budget,
    );
    let meter = format_context_meter_new(app.status_bar.token_total, app.model_context_window);
    let token_str = format!("{}{}", ICON_TOKENS, app.status_bar.token_total);
    let cache_str = format_cache_pct(
        app.status_bar.token_cache_hit.into(),
        app.status_bar.token_cache_miss.into(),
    );

    let mut row2_groups: Vec<DropGroup> = Vec::new();
    // Model + context meter
    row2_groups.push(DropGroup {
        droppable: false,
        spans: vec![Span::styled(model, primary)],
    });
    row2_groups.push(DropGroup {
        droppable: true,
        spans: vec![Span::styled(" · ", dim), Span::styled(meter, primary)],
    });
    // Token total (droppable after cache)
    row2_groups.push(DropGroup {
        droppable: true,
        spans: vec![Span::styled(" · ", dim), Span::styled(token_str, secondary)],
    });
    // Cache hit (most droppable)
    row2_groups.push(DropGroup {
        droppable: true,
        spans: vec![Span::styled(" · ", dim), Span::styled(cache_str, secondary)],
    });

    fit_row_spans(area.width, &mut row1_groups);
    fit_row_spans(area.width, &mut row2_groups);

    let row1_spans: Vec<Span> = row1_groups.into_iter().flat_map(|g| g.spans).collect();
    let row2_spans: Vec<Span> = row2_groups.into_iter().flat_map(|g| g.spans).collect();

    let row_count = area.height.max(1) as usize;
    let constraints: Vec<Constraint> = (0..row_count).map(|_| Constraint::Length(1)).collect();
    let areas = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(area);
    let top_area = areas[0];
    let mid_area = areas.get(1).copied().unwrap_or(top_area);

    let bg = Style::default().bg(theme.bottom_bar_bg);
    frame.render_widget(Paragraph::new(Line::from(row1_spans)).style(bg), top_area);
    frame.render_widget(Paragraph::new(Line::from(row2_spans)).style(bg), mid_area);
}

/// Build account balance/quota spans for row 1. Returns None when no account.
fn build_account_spans(app: &App, theme: &crate::theme::Theme) -> Option<Vec<Span<'static>>> {
    app.account_rx.as_ref()?;
    if let Some(bi) = &app.account.balance {
        let fg = if bi.is_available {
            theme.success
        } else {
            theme.error
        };
        let entries: Vec<String> = bi.balance_infos.iter().map(format_balance_entry).collect();
        return Some(vec![Span::styled(
            entries.join(" · "),
            Style::default().fg(fg),
        )]);
    }
    if let Some(quota) = &app.account.quota {
        let fg = if quota.is_available {
            theme.success
        } else {
            theme.error
        };
        let entries: Vec<String> = quota.windows.iter().map(format_quota_window).collect();
        return Some(vec![Span::styled(
            entries.join(" · "),
            Style::default().fg(fg),
        )]);
    }
    None
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
        FocusedPanel::Log => "Log",
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
    use ratatui::{Terminal, backend::TestBackend, layout::Rect};
    use tact_protocol::{BalanceEntry, BalanceInfo};

    use super::{
        super::test_harness::{buffer_text, make_app, render_app_text},
        render_bottom_bar, render_usage_bar,
    };

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
            row1.contains("01:05"),
            "elapsed time should be on row 1, got:\n{row1}"
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
            row2.contains("∑42"),
            "token stats should stay on row 2, got:\n{row2}"
        );
    }

    #[test]
    fn bottom_bar_shows_compact_model_with_limits() {
        let mut app = make_app();
        app.status_bar.model_name = "mock-model".into();
        app.status_bar.model_max_tokens = 128_000;
        app.status_bar.model_thinking_budget = Some(32_000);
        app.status_bar.model_reasoning_effort = Some("high".into());
        let backend = TestBackend::new(120, 2);
        let mut terminal = Terminal::new(backend).expect("terminal");

        terminal
            .draw(|frame| render_bottom_bar(frame, Rect::new(0, 0, 120, 2), &app))
            .expect("draw");

        let text = buffer_text(terminal.backend().buffer());
        assert!(
            text.contains("mock-model") && text.contains("128K/32K"),
            "bottom bar should show compact model with max/think, got:\n{text}"
        );
    }

    #[test]
    fn bottom_bar_shows_compact_model_when_effort_is_absent() {
        let mut app = make_app();
        app.status_bar.model_name = "mock-model".into();
        app.status_bar.model_max_tokens = 128_000;
        app.status_bar.model_thinking_budget = Some(32_000);
        let backend = TestBackend::new(120, 2);
        let mut terminal = Terminal::new(backend).expect("terminal");

        terminal
            .draw(|frame| render_bottom_bar(frame, Rect::new(0, 0, 120, 2), &app))
            .expect("draw");

        let text = buffer_text(terminal.backend().buffer());
        assert!(
            text.contains("128K/32K"),
            "bottom bar should show compact model limits even without effort, got:\n{text}"
        );
    }

    #[test]
    fn format_model_compact_empty() {
        assert_eq!(super::format_model_compact("", 0, None), "-");
    }
    #[test]
    fn format_model_compact_name_only() {
        assert_eq!(super::format_model_compact("gpt4", 0, None), "gpt4");
    }
    #[test]
    fn format_model_compact_with_max() {
        assert_eq!(
            super::format_model_compact("claude", 8_000, None),
            "claude 8K"
        );
    }
    #[test]
    fn format_model_compact_with_max_and_think() {
        assert_eq!(
            super::format_model_compact("deepseek", 32_000, Some(8_000)),
            "deepseek 32K/8K"
        );
    }
    #[test]
    fn format_cache_pct_before_first_sample() {
        assert_eq!(super::format_cache_pct(0, 0), "▣--");
    }
    #[test]
    fn format_cache_pct_with_data() {
        assert_eq!(super::format_cache_pct(30, 70), "▣30%");
    }
    #[test]
    fn format_cache_pct_full_hit() {
        assert_eq!(super::format_cache_pct(100, 0), "▣100%");
    }
    #[test]
    fn format_balance_entry_renders() {
        let entry = BalanceEntry {
            currency: "USD".into(),
            total_balance: 12.50,
            granted_balance: 10.0,
            topped_up_balance: 2.50,
        };
        let result = super::format_balance_entry(&entry);
        assert!(result.contains("¤"), "expected ¤, got {result}");
        assert!(result.contains("USD"), "expected USD, got {result}");
        assert!(result.contains("12.50"), "expected 12.50, got {result}");
    }
    #[test]
    fn format_quota_window_with_pct() {
        use tact_protocol::UsageQuotaWindow;
        let w = UsageQuotaWindow {
            label: "daily".into(),
            remaining: Some(150.0),
            limit: Some(200.0),
            reset_time: None,
        };
        let result = super::format_quota_window(&w);
        assert!(result.contains("¤"), "expected ¤, got {result}");
        assert!(result.contains("daily"), "expected daily, got {result}");
        assert!(result.contains("25%"), "expected 25%, got {result}");
    }
    #[test]
    fn format_quota_window_infinite() {
        use tact_protocol::UsageQuotaWindow;
        let w = UsageQuotaWindow {
            label: "monthly".into(),
            remaining: Some(500.0),
            limit: None,
            reset_time: None,
        };
        let result = super::format_quota_window(&w);
        assert!(result.contains("500/∞"), "expected 500/∞, got {result}");
    }
}
