use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};
use tact_protocol::BalanceEntry;

use crate::{
    i18n::Messages,
    state::{FocusedPanel, InputMode, Status},
    theme::Theme,
};

use crate::render::ctx::RenderCtx;

/// Spinner animation frames for typing/loading indicator.
const SPINNER_FRAMES: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

/// Progress bar width in cells.
const PROGRESS_BAR_WIDTH: u16 = 15;

/// Bottom bar icons (language-invariant Unicode glyphs).
const ICON_UPTIME: &str = "⊙";
const ICON_BRANCH: &str = "⎇";
const ICON_BALANCE: &str = "¤";
/// U+2211 + subscript t/o/k (U+209C U+2092 U+2096).
const ICON_TOKENS: &str = "∑ₜₒₖ";
const ICON_CACHE: &str = "▣";
const SEP_ROW1: &str = " │ ";
const SEP_ROW2: &str = "  ";
const BAR_FILLED: char = '■'; // U+25A0
const BAR_EMPTY: char = '·'; // U+00B7

/// Partial block characters from 1/8 to 7/8 width (U+258F … U+2589).
/// Low usage clamps to at least `▍` — see `partial_block_char`.
const PARTIAL_BLOCKS: [char; 7] = ['▏', '▎', '▍', '▌', '▋', '▊', '▉'];

const USAGE_BAR_WIDTH: u16 = 10;

/// Short elapsed label for the status bar during active runs.
///
/// Derived-method migration of `App::format_task_elapsed` (design doc §2.2):
/// pure function of the i18n label + task start time.
pub fn format_task_elapsed(
    msgs: &Messages,
    task_start_time: Option<&chrono::DateTime<chrono::Local>>,
) -> String {
    let Some(start) = task_start_time else {
        return String::new();
    };
    let secs = chrono::Local::now()
        .signed_duration_since(*start)
        .num_seconds()
        .max(0);
    let mm_ss = format!("{:02}:{:02}", secs / 60, secs % 60);
    format!("⏱ {} {}", msgs.bottom_elapsed, mm_ss)
}

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
    let exact = (pct / 100.0) * inner_width as f64;
    let full_blocks = exact.floor() as usize;
    let fractional = exact - full_blocks as f64;

    let mut bar = String::from("[");
    // Full blocks
    for _ in 0..full_blocks.min(inner_width) {
        bar.push(BAR_FILLED);
    }
    // Boundary partial block + remaining empty
    if full_blocks < inner_width {
        if fractional > 0.0 {
            bar.push(partial_block_char(fractional));
            for _ in (full_blocks + 1)..inner_width {
                bar.push(BAR_EMPTY);
            }
        } else {
            for _ in full_blocks..inner_width {
                bar.push(BAR_EMPTY);
            }
        }
    }
    bar.push(']');
    bar
}

/// Map a fraction (0, 1] to the closest partial-block character.
///
/// Any positive fraction paints at least `▍` (3/8). Terminal fonts often
/// render `▏`/`▎` as a hairline that reads as empty next to `·`, so 1% of a
/// large context window looked like no progress despite the numeric label.
fn partial_block_char(frac: f64) -> char {
    if frac <= 0.0 {
        return BAR_EMPTY;
    }
    // frac is (0, 1]; map to 1..=8, then floor at 3 for visibility.
    let idx = ((frac * 8.0).round() as usize).clamp(3, 8);
    match idx {
        3..=7 => PARTIAL_BLOCKS[idx - 1],
        8 => BAR_FILLED,
        _ => PARTIAL_BLOCKS[2], // ▍
    }
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

/// Model name only: `"modelname"` or `"-"`.
fn format_model_name(name: &str) -> String {
    if name.is_empty() {
        "-".to_string()
    } else {
        name.to_string()
    }
}

/// Labeled max-output segment: `"max_out_token 73K"` / `None` when max is 0.
///
/// The value is the effective text-output budget. Effort-semantic models
/// (openai / deepseek / kimi k3) count reasoning inside the SAME `max_tokens`
/// envelope as the output text, so the reasoning share is subtracted using the
/// same tier convention as `compact_summary_reasoning_reserve_percent`
/// (percent of the text budget, added on top): text = envelope × 100/(100+pct).
/// Budget-semantic models (Anthropic-style `thinking_budget`) keep thinking in
/// a separate envelope, so no subtraction applies.
fn format_max_out_tokens(
    label: &str,
    max_tokens: u32,
    thinking_budget: Option<u32>,
    reasoning_effort: Option<&str>,
) -> Option<String> {
    if max_tokens == 0 {
        return None;
    }
    let pct = match reasoning_effort {
        Some("none") => 0,
        Some("minimal") | Some("low") => 25,
        Some("medium") => 50,
        Some("high") => 75,
        Some("xhigh") | Some("max") => 100,
        _ => 0,
    };
    let max_out = if thinking_budget.is_some() {
        // Separate thinking envelope: max_tokens already is the output limit.
        max_tokens
    } else {
        max_tokens.saturating_mul(100) / (100 + pct)
    };
    Some(format!("{label} {}", format_tokens_compact(max_out as u64)))
}

/// Thinking segment: `"think high"`, `"think 32K"`, or `None`.
///
/// Effort takes precedence over budget: effort and budget are mutually
/// exclusive semantics (effort-semantic models never send a budget), so a
/// stale `thinking_budget` left over from a budget-semantic model must not
/// render a meaningless `think high(32K)` next to an explicit effort.
fn format_think_segment(label: &str, effort: Option<&str>, budget: Option<u32>) -> Option<String> {
    if let Some(level) = effort.filter(|e| !e.is_empty()) {
        return Some(format!("{label} {level}"));
    }
    let budget = budget.filter(|b| *b > 0)?;
    Some(format!("{label} {}", format_tokens_compact(budget as u64)))
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

/// Format cache hit percentage: `"▣ cache% 45%"` or `"▣ cache% --"`.
fn format_cache_pct(hit: u64, miss: u64, label: &str) -> String {
    let total = hit + miss;
    if total == 0 {
        format!("{ICON_CACHE} {label} --")
    } else {
        let pct = hit.saturating_mul(100).checked_div(total).unwrap_or(0);
        format!("{ICON_CACHE} {label} {pct}%")
    }
}

/// Context usage meter: `"ctx [■■····] 0% 6.6K/1M"`.
fn format_context_meter(label: &str, used: u32, window: usize) -> String {
    let pct = context_usage_pct(used, window);
    let bar = render_usage_bar(pct as f64);
    format!(
        "{label} {bar} {pct}% {}/{}",
        format_tokens_compact(used as u64),
        format_tokens_compact(window as u64)
    )
}

/// Last-call total tokens: `"∑ₜₒₖ 6584"`.
fn format_token_total(total: u32) -> String {
    format!("{ICON_TOKENS} {total}")
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

/// Render the bottom bar, showing TUI uptime, working directory, Git branch,
/// model info, token stats, and account balance.
/// Prompt elapsed is shown on the task-end separator, not here.
pub fn render_bottom_bar(frame: &mut Frame, area: Rect, ctx: &RenderCtx) {
    frame.render_widget(ratatui::widgets::Clear, area);

    let msgs = &ctx.messages;
    let theme = &ctx.theme;
    let dim = Style::default().fg(theme.muted_fg());
    let primary = Style::default().fg(theme.fg);
    let secondary = Style::default().fg(theme.bottom_bar_fg);
    let accent = Style::default().fg(theme.accent);

    // --- Row 1 ---
    let branch = if ctx.status_bar.git_branch.is_empty() {
        msgs.bottom_branch_unknown
    } else {
        &ctx.status_bar.git_branch
    };
    let uptime = {
        let dur = chrono::Local::now().signed_duration_since(ctx.process_start_time);
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

    #[allow(clippy::vec_init_then_push)]
    let mut row1_groups: Vec<DropGroup> = vec![
        // Permission mode (never dropped — security-critical indicator)
        {
            let (perm_label, perm_style) = match ctx.status_bar.permission_mode.as_str() {
                "plan" => (
                    msgs.bottom_permission_plan,
                    Style::default().fg(theme.warning),
                ),
                "default" => (msgs.bottom_permission_default, secondary),
                _ => (
                    msgs.bottom_permission_auto,
                    Style::default().fg(theme.success),
                ),
            };
            DropGroup {
                droppable: false,
                spans: vec![
                    Span::styled(perm_label, perm_style),
                    Span::styled(SEP_ROW1.to_string(), dim),
                ],
            }
        },
        // Path (droppable)
        DropGroup {
            droppable: true,
            spans: vec![
                Span::styled(ctx.workspace_dir.to_string(), secondary),
                Span::styled(SEP_ROW1.to_string(), dim),
            ],
        },
        // Uptime: last droppable on row1 so it drops before path
        DropGroup {
            droppable: true,
            spans: vec![
                Span::styled(ICON_UPTIME.to_string(), dim),
                Span::styled(format!(" {} {}", msgs.bottom_uptime, uptime), secondary),
                Span::styled(SEP_ROW1.to_string(), dim),
            ],
        },
        // Branch: ⎇ branchname
        DropGroup {
            droppable: false,
            spans: vec![
                Span::styled(ICON_BRANCH.to_string(), dim),
                Span::styled(format!(" {}", branch), accent),
            ],
        },
    ];
    // Account (if present, never dropped)
    #[allow(clippy::vec_init_then_push)]
    if let Some(acct_spans) = build_account_spans(ctx, theme) {
        row1_groups.push(DropGroup {
            droppable: false,
            spans: vec![Span::styled(SEP_ROW1.to_string(), dim)],
        });
        row1_groups.push(DropGroup {
            droppable: false,
            spans: acct_spans,
        });
    }

    // --- Row 2 ---
    let model = format_model_name(&ctx.status_bar.model_name);
    let out = format_max_out_tokens(
        msgs.bottom_out,
        ctx.status_bar.model_max_tokens,
        ctx.status_bar.model_thinking_budget,
        ctx.status_bar.model_reasoning_effort.as_deref(),
    );
    let think = format_think_segment(
        msgs.bottom_think,
        ctx.status_bar.model_reasoning_effort.as_deref(),
        ctx.status_bar.model_thinking_budget,
    );
    let meter = format_context_meter(
        msgs.bottom_ctx,
        ctx.status_bar.token_total,
        ctx.model_context_window,
    );
    let token_str = format_token_total(ctx.status_bar.token_total);
    let cache_str = format_cache_pct(
        ctx.status_bar.token_cache_hit.into(),
        ctx.status_bar.token_cache_miss.into(),
        msgs.bottom_cache_pct,
    );

    #[allow(clippy::vec_init_then_push)]
    let mut row2_groups: Vec<DropGroup> = vec![DropGroup {
        droppable: false,
        spans: vec![Span::styled(model, primary)],
    }];
    if let Some(out) = out {
        row2_groups.push(DropGroup {
            droppable: false,
            spans: vec![
                Span::styled(SEP_ROW2.to_string(), dim),
                Span::styled(out, primary),
            ],
        });
    }
    if let Some(think) = think {
        row2_groups.push(DropGroup {
            droppable: false,
            spans: vec![
                Span::styled(SEP_ROW2.to_string(), dim),
                Span::styled(think, primary),
            ],
        });
    }
    // Drop order from end: cache → ∑ → ctx
    row2_groups.push(DropGroup {
        droppable: true,
        spans: vec![
            Span::styled(SEP_ROW2.to_string(), dim),
            Span::styled(meter, primary),
        ],
    });
    row2_groups.push(DropGroup {
        droppable: true,
        spans: vec![
            Span::styled(SEP_ROW2.to_string(), dim),
            Span::styled(token_str, secondary),
        ],
    });
    row2_groups.push(DropGroup {
        droppable: true,
        spans: vec![
            Span::styled(SEP_ROW2.to_string(), dim),
            Span::styled(cache_str, secondary),
        ],
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
fn build_account_spans(ctx: &RenderCtx, theme: &Theme) -> Option<Vec<Span<'static>>> {
    let account = ctx.account?;
    if let Some(bi) = &account.balance {
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
    if let Some(quota) = &account.quota {
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
pub fn render_status_bar(frame: &mut Frame, area: Rect, ctx: &RenderCtx) {
    let msgs = &ctx.messages;

    // Mode indicator with emoji
    let (mode_emoji, mode_indicator) = match ctx.input_mode {
        InputMode::Normal => ("◆", msgs.mode_normal),
        InputMode::Insert => ("◇", msgs.mode_insert),
        InputMode::Palette => ("⚡", msgs.mode_palette),
        InputMode::Select => ("▣", msgs.mode_select),
        InputMode::FilePicker => ("📎", msgs.mode_file_picker),
    };
    let mode_str = format!("{} {}", mode_emoji, mode_indicator);

    let focus_str = match ctx.focused_panel {
        FocusedPanel::Log => "Log",
    };

    let (status_text, status_style) = match &ctx.status {
        Status::Idle => {
            let theme_label = match ctx.theme.name {
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
                crate::theme::ThemeName::Ink => msgs.theme_ink,
                crate::theme::ThemeName::InkLight => msgs.theme_ink_light,
            };
            let lang_label = ctx.language.label();
            (
                msgs.status_idle_tmpl
                    .replacen("{}", &mode_str, 1)
                    .replacen("{}", focus_str, 1)
                    .replacen("{}", theme_label, 1)
                    .replacen("{}", lang_label, 1),
                Style::default()
                    .bg(ctx.theme.status_bar_bg)
                    .fg(ctx.theme.fg),
            )
        }
        Status::Planning => {
            let spinner = SPINNER_FRAMES[ctx.spinner_frame as usize];
            let elapsed = format_task_elapsed(&ctx.messages, ctx.task_start_time);
            (
                format!(
                    "{} {} │ {} {}  {}",
                    mode_str, focus_str, spinner, msgs.status_planning, elapsed
                ),
                Style::default()
                    .bg(ctx.theme.status_bar_bg)
                    .fg(ctx.theme.accent),
            )
        }
        Status::Executing {
            current_step: _,
            total,
        } => {
            let spinner = SPINNER_FRAMES[ctx.spinner_frame as usize];
            // With parallel tools, `current_step` is no longer a reliable UI
            // progress anchor. Derive progress from completed + active steps.
            let completed = ctx
                .plan
                .steps
                .iter()
                .filter(|s| s.output.as_ref().is_some())
                .count()
                .min(*total);
            let running = ctx.tools.active.len();
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
            let progress_bar = render_progress_bar(progress_idx, *total, ctx.theme);
            let exec_right = if running > 0 {
                format!("{} │ {} {}", step_label, running_label, progress_bar)
            } else {
                format!("{} {}", step_label, progress_bar)
            };
            (
                format!(
                    "{} {} │ {} {}  {}",
                    mode_str,
                    focus_str,
                    spinner,
                    exec_right,
                    format_task_elapsed(&ctx.messages, ctx.task_start_time)
                ),
                Style::default()
                    .bg(ctx.theme.status_bar_bg)
                    .fg(ctx.theme.warning),
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
                .bg(ctx.theme.success)
                .fg(ctx.theme.fg)
                .add_modifier(Modifier::BOLD),
        ),
    };
    let (display_text, display_style) = if let Some(msg) = ctx.flash_msg {
        (
            format!("⚠ {}", msg),
            Style::default()
                .bg(ctx.theme.warning)
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
    use tact_protocol::BalanceEntry;

    use super::render_usage_bar;

    #[test]
    fn render_usage_bar_scales_to_width() {
        assert_eq!(render_usage_bar(0.0), "[········]");
        assert_eq!(render_usage_bar(50.0), "[■■■■····]");
        assert_eq!(render_usage_bar(100.0), "[■■■■■■■■]");
    }

    #[test]
    fn render_usage_bar_uses_mid_height_glyphs() {
        assert_eq!(super::render_usage_bar(0.0), "[········]");
        assert_eq!(super::render_usage_bar(50.0), "[■■■■····]");
        assert_eq!(super::render_usage_bar(100.0), "[■■■■■■■■]");
    }

    #[test]
    fn render_usage_bar_partial_block_at_low_percent() {
        // 1% → at least ▍ (hairline ▏ was effectively invisible in terminals)
        let bar = super::render_usage_bar(1.0);
        assert_eq!(bar, "[▍·······]");
        assert_ne!(bar, "[········]", "1% must differ from 0%");
        // Sub-1% still shows the minimum visible partial (not empty)
        let bar_half = super::render_usage_bar(0.5);
        assert_eq!(bar_half, "[▍·······]");
        // 6% and 10% show progressively wider partial blocks
        let bar6 = super::render_usage_bar(6.0);
        let bar10 = super::render_usage_bar(10.0);
        assert_eq!(bar6, "[▌·······]");
        assert_eq!(bar10, "[▊·······]");
        assert_ne!(bar6, bar, "6% must differ from 1%");
        assert_ne!(bar10, bar6, "10% must differ from 6%");
    }

    #[test]
    fn format_max_out_tokens_labeled() {
        assert_eq!(
            super::format_max_out_tokens("max_out_token", 8_000, None, None),
            Some("max_out_token 8K".into())
        );
        assert_eq!(
            super::format_max_out_tokens("max_out_token", 0, None, None),
            None
        );
    }

    #[test]
    fn format_max_out_tokens_subtracts_effort_share() {
        // Effort-semantic models count reasoning inside max_tokens; the
        // reserve convention is pct% of the TEXT budget on top, so the
        // text share is envelope × 100/(100+pct).
        let cases = [
            // (effort, expected share of max_tokens)
            (Some("none"), 1_000_000),
            (Some("low"), 800_000),
            (Some("medium"), 666_666),
            (Some("high"), 571_428),
            (Some("xhigh"), 500_000),
            (Some("max"), 500_000),
        ];
        for (effort, expected) in cases {
            assert_eq!(
                super::format_max_out_tokens("max_out_token", 1_000_000, None, effort),
                Some(format!(
                    "max_out_token {}",
                    super::format_tokens_compact(expected)
                )),
                "effort {effort:?}"
            );
        }
        // No effort → no subtraction.
        assert_eq!(
            super::format_max_out_tokens("max_out_token", 128_000, None, None),
            Some("max_out_token 128K".into())
        );
    }

    #[test]
    fn format_max_out_tokens_budget_keeps_full_envelope() {
        // Budget-semantic models (Anthropic) keep thinking in a separate
        // envelope, so max_tokens is already the output limit.
        assert_eq!(
            super::format_max_out_tokens("max_out_token", 128_000, Some(32_000), None),
            Some("max_out_token 128K".into())
        );
        assert_eq!(
            super::format_max_out_tokens("max_out_token", 128_000, Some(32_000), Some("high")),
            Some("max_out_token 128K".into())
        );
    }

    #[test]
    fn format_think_with_effort_takes_precedence_over_stale_budget() {
        assert_eq!(
            super::format_think_segment("思考", Some("high"), Some(32_000)),
            Some("思考 high".into())
        );
        assert_eq!(
            super::format_think_segment("think", Some("medium"), Some(8_000)),
            Some("think medium".into())
        );
    }

    #[test]
    fn format_think_budget_only() {
        assert_eq!(
            super::format_think_segment("思考", None, Some(32_000)),
            Some("思考 32K".into())
        );
    }

    #[test]
    fn format_think_omitted_without_budget() {
        assert_eq!(super::format_think_segment("think", None, Some(0)), None);
        assert_eq!(super::format_think_segment("think", None, None), None);
    }

    #[test]
    fn format_think_shows_effort_without_budget() {
        // Effort-semantic models (openai / deepseek / kimi k3) have no budget;
        // the effort alone must still render so the bar does not lose the info.
        assert_eq!(
            super::format_think_segment("think", Some("high"), None),
            Some("think high".into())
        );
    }

    #[test]
    fn format_cache_pct_with_label() {
        assert_eq!(super::format_cache_pct(0, 0, "缓存%"), "▣ 缓存% --");
        assert_eq!(super::format_cache_pct(30, 70, "cache%"), "▣ cache% 30%");
        assert_eq!(super::format_cache_pct(100, 0, "缓存%"), "▣ 缓存% 100%");
    }

    #[test]
    fn format_context_meter_labeled() {
        let s = super::format_context_meter("ctx", 0, 1_000_000);
        assert!(s.starts_with("ctx ["), "got {s}");
        assert!(s.contains("0%"), "got {s}");
        assert!(s.contains("0/1M"), "got {s}");
        assert!(
            !s.contains('█') && !s.contains('░'),
            "old glyphs present: {s}"
        );
        assert!(
            s.contains('·') || s.contains('■'),
            "expected mid-height glyphs: {s}"
        );
    }

    #[test]
    fn format_token_total_icon() {
        assert_eq!(super::format_token_total(6584), "∑ₜₒₖ 6584");
    }

    #[test]
    fn sigma_tok_unicode_width_is_sane() {
        let w = unicode_width::UnicodeWidthStr::width(super::ICON_TOKENS);
        assert!(
            (1..=8).contains(&w),
            "∑ₜₒₖ width {w} looks pathological; consider ∑_tok fallback"
        );
    }

    #[test]
    #[test]
    #[test]
    #[test]
    #[test]
    #[test]
    #[test]
    #[test]
    fn format_model_name_empty() {
        assert_eq!(super::format_model_name(""), "-");
    }
    #[test]
    fn format_model_name_only() {
        assert_eq!(super::format_model_name("gpt4"), "gpt4");
    }
    #[test]
    fn format_cache_pct_before_first_sample() {
        assert_eq!(super::format_cache_pct(0, 0, "cache%"), "▣ cache% --");
    }
    #[test]
    fn format_cache_pct_with_data() {
        assert_eq!(super::format_cache_pct(30, 70, "cache%"), "▣ cache% 30%");
    }
    #[test]
    fn format_cache_pct_full_hit() {
        assert_eq!(super::format_cache_pct(100, 0, "cache%"), "▣ cache% 100%");
    }
    #[test]
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
