//! Live per-task stats line, pinned to the Log panel's last content row.
//!
//! The stats block written when a task ends is a **log row**: it is the record
//! of a finished turn, and it scrolls away as soon as the next turn writes
//! anything. While a task is in flight the same line is drawn *live* on the Log
//! panel's last content row instead — one row, repainted every frame from the
//! counters the bottom bar already reads, never part of the log.
//!
//! The handoff is deliberate: the live row disappears exactly when
//! `App::add_task_stats_block` writes its frozen twin in the same place, so the
//! numbers do not move. Idle draws nothing — the frozen row is already the last
//! thing in the log, and a second copy above the input would spend a row of
//! every idle frame saying the same numbers twice.

use ratatui::{
    Frame,
    layout::{Alignment, Rect},
    style::Style,
    text::{Line, Span},
    widgets::{Borders, Paragraph},
};

use crate::{i18n::Messages, state::StatusBarState};

use super::ctx::RenderCtx;

/// Rows the live line takes from the Log viewport.
pub const LIVE_STATS_ROWS: u16 = 1;

/// The stats body shared by the live row and the frozen end-of-turn row.
///
/// Single source of truth: `App::add_task_stats_block` writes exactly this
/// string into the log when a task ends, so the live row and the row it hands
/// off to cannot drift in shape.
pub fn task_stats_body(msgs: &Messages, secs: i64, status_bar: &StatusBarState) -> String {
    let secs = secs.max(0);
    let mm_ss = format!("{:02}:{:02}", secs / 60, secs % 60);
    let mut parts = vec![format!("⏱ {mm_ss}")];
    if !status_bar.model_name.is_empty() {
        parts.push(status_bar.model_name.clone());
    }
    if status_bar.token_total > 0 {
        let mut detail = format!("{} tokens", status_bar.token_total);
        let sub: Vec<String> = [
            ("prompt", status_bar.token_prompt),
            ("completion", status_bar.token_completion),
            ("cache", status_bar.token_cache_hit),
            ("reasoning", status_bar.token_reasoning),
        ]
        .into_iter()
        .filter(|(_, v)| *v > 0)
        .map(|(k, v)| format!("{k} {v}"))
        .collect();
        if !sub.is_empty() {
            detail.push_str(&format!(" ({})", sub.join(" · ")));
        }
        parts.push(detail);
    }
    format!("{}{}", msgs.task_stats_prefix, parts.join(" · "))
}

/// Content rows inside a Log panel drawn with `borders`.
fn content_rows(area: Rect, borders: Borders) -> u16 {
    let top = u16::from(borders.contains(Borders::TOP));
    let bottom = u16::from(borders.contains(Borders::BOTTOM));
    area.height.saturating_sub(top + bottom)
}

/// Rows the host must subtract from the Log viewport for the live line.
///
/// Takes the raw in-flight flag (not a `RenderCtx`) so the app-side prepare
/// phase can ask before it takes its mutable borrow. Zero when no task is
/// running, and zero when reserving would leave no log row at all — a panel
/// that small must keep its content.
pub fn live_stats_reserve(area: Rect, borders: Borders, task_in_flight: bool) -> u16 {
    u16::from(task_in_flight && content_rows(area, borders) > LIVE_STATS_ROWS)
}

/// The row the live line is drawn on: the last content row, i.e. directly above
/// the bottom border (or above the sticky strip when the Log omits it).
pub fn live_stats_row(area: Rect, borders: Borders) -> Option<Rect> {
    if content_rows(area, borders) <= LIVE_STATS_ROWS {
        return None;
    }
    let left = u16::from(borders.contains(Borders::LEFT));
    let right = u16::from(borders.contains(Borders::RIGHT));
    let bottom = u16::from(borders.contains(Borders::BOTTOM));
    Some(Rect {
        x: area.x.saturating_add(left),
        y: area
            .y
            .saturating_add(area.height)
            .saturating_sub(1 + bottom),
        width: area.width.saturating_sub(left + right),
        height: LIVE_STATS_ROWS,
    })
}

/// Live wall clock of the task in flight.
fn live_elapsed_secs(ctx: &RenderCtx) -> i64 {
    ctx.task_start_time
        .map(|start| {
            chrono::Local::now()
                .signed_duration_since(*start)
                .num_seconds()
                .max(0)
        })
        .unwrap_or(0)
}

/// Draw the live stats line over `area`.
///
/// The line is **centered**: it is a HUD strip on the turn boundary, not a log
/// row — the rule above it is full-width, and the number that used to sit
/// centered in that rule now sits centered under it. (The frozen row written at
/// task end stays left-aligned: it is a transcript row with a `⎘` affordance,
/// not a HUD.)
///
/// The whole row is painted with the theme background first — a row that only
/// paints its glyphs leaves the previous frame's style in the tail (AGENTS.md
/// render invariant), which shows up as a band when the line gets shorter.
pub fn render_live_stats_band(frame: &mut Frame, area: Rect, ctx: &RenderCtx) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let theme = ctx.theme;
    let base = Style::default().bg(theme.bg);
    let buf = frame.buffer_mut();
    for x in area.left()..area.right() {
        buf[(x, area.y)].set_symbol(" ").set_style(base);
    }

    let body = task_stats_body(&ctx.messages, live_elapsed_secs(ctx), ctx.status_bar);
    let line = Line::from(Span::styled(
        body,
        Style::default().fg(theme.accent).bg(theme.bg),
    ));
    frame.render_widget(
        Paragraph::new(line)
            .alignment(Alignment::Center)
            .style(base),
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::task_stats_body;
    use crate::{i18n::Messages, state::StatusBarState};

    fn status_bar(model: &str) -> StatusBarState {
        let mut state = StatusBarState::new("main".into());
        state.model_name = model.into();
        state
    }

    #[test]
    fn body_matches_the_frozen_row_shape() {
        let msgs = Messages::by_language(crate::i18n::Language::English);
        let mut bar = status_bar("deepseek-flash");
        bar.token_prompt = 60_826;
        bar.token_completion = 3_420;
        bar.token_total = 64_246;
        bar.token_cache_hit = 60_032;
        bar.token_reasoning = 1_810;

        assert_eq!(
            task_stats_body(&msgs, 45, &bar),
            "Task stats:⏱ 00:45 · deepseek-flash · 64246 tokens (prompt 60826 · completion 3420 · cache 60032 · reasoning 1810)"
        );
    }

    #[test]
    fn body_skips_the_model_and_the_token_detail_when_empty() {
        let msgs = Messages::by_language(crate::i18n::Language::English);
        let bar = status_bar("");

        assert_eq!(task_stats_body(&msgs, 5, &bar), "Task stats:⏱ 00:05");

        // A model but no usage yet: the row is still worth drawing.
        let bar = status_bar("gpt-5");
        assert_eq!(
            task_stats_body(&msgs, 65, &bar),
            "Task stats:⏱ 01:05 · gpt-5"
        );
    }

    #[test]
    fn body_is_localized_and_clamps_negative_seconds() {
        let msgs = Messages::by_language(crate::i18n::Language::Chinese);
        let bar = status_bar("");

        assert_eq!(task_stats_body(&msgs, -3, &bar), "任务统计：⏱ 00:00");
    }
}
