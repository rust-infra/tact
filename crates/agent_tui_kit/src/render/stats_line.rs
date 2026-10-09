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
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::{Borders, Paragraph},
};
use unicode_width::UnicodeWidthStr;

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

/// Shortest runway the mascot will use, in columns.
///
/// Below this the row has no mascot at all. That is the whole degrade path: a
/// narrow row *is* a row with no padding to give away, so there is nothing to
/// branch on. A two-column round trip would read as a twitch, not a walk.
pub const MASCOT_MIN_RUNWAY: u16 = 4;

/// The mascot, right-facing and left-facing.
///
/// A terminal cannot mirror a glyph, so "turning around" is a swap to the
/// same-family glyph that faces the other way — `md-transfer_right` /
/// `md-transfer_left`. Both advance exactly one column (checked against the
/// font's advances: identical to `0`), which is what keeps the runway maths
/// exact and stops the mascot from ever nudging the text.
pub const MASCOT_RIGHT: char = '\u{F0530}';
pub const MASCOT_LEFT: char = '\u{F0DA2}';

/// Runway on each side of the centered text, in columns.
///
/// The text starts at `pad`, so the mascot's far end is `pad - 2`: one blank
/// column always stays between it and the text.
fn mascot_runway(pad: u16) -> u16 {
    pad.saturating_sub(2)
}

/// Where the mascot stands on a `runway`-column lane, and which way it faces.
///
/// Positions run `0..runway`, near end to far end; the walk is a ping-pong, one
/// column per tick. `bool` is `true` while it is heading for the far end.
///
/// The tick is the same one every other animation in the TUI uses, so the
/// mascot stays in step with the tool spinner. It advances per *painted frame*,
/// not per millisecond, so a busy stream walks the mascot faster — the same
/// trade the spinners already make, and the reason this takes the raw counter
/// rather than reading the clock.
pub fn mascot_step(tick: u32, runway: u16) -> (u16, bool) {
    let leg = runway.saturating_sub(1);
    if leg == 0 {
        return (0, true);
    }
    let period = u32::from(leg) * 2;
    let step = tick % period;
    if step <= u32::from(leg) {
        (step as u16, true)
    } else {
        ((period - step) as u16, false)
    }
}

/// The mascot's two cells for this tick: left runway first, then right.
///
/// `None` when the row has no runway worth using — see [`MASCOT_MIN_RUNWAY`].
///
/// The right mascot mirrors the left: the same distance in from its own edge,
/// travelling the other way, so the two walk in together and turn together.
/// Their glyphs therefore differ — each faces its own direction of travel.
pub fn mascot_cells(area: Rect, pad: u16, tick: u32) -> Option<[(u16, char); 2]> {
    let runway = mascot_runway(pad);
    if runway < MASCOT_MIN_RUNWAY || area.width < 2 {
        return None;
    }
    let (pos, forward) = mascot_step(tick, runway);
    let (left_glyph, right_glyph) = if forward {
        (MASCOT_RIGHT, MASCOT_LEFT)
    } else {
        (MASCOT_LEFT, MASCOT_RIGHT)
    };
    let left = area.x + 1 + pos;
    let right = area.x + area.width.saturating_sub(2 + pos);
    // Runways that would meet in the middle: the row is too narrow for two.
    if right <= left + 1 {
        return None;
    }
    Some([(left, left_glyph), (right, right_glyph)])
}

/// Leading pad that centers `text_width` columns inside `content_width`.
///
/// The one centering rule for the stats line, in both of its spellings: the
/// live band places its text with it, and the frozen log row has it baked into
/// its line by the app's wrap pass.
pub fn center_pad(content_width: usize, text_width: usize) -> u16 {
    (content_width.saturating_sub(text_width) / 2) as u16
}

/// The part of [`center_pad`] a **log row** carries inside its own text.
///
/// A log row is drawn at its indent (`log_indent_at`), so only the centering
/// beyond that indent can be baked in — a pad smaller than the indent cannot
/// move the row left of it. The app's click path subtracts `indent + this` from
/// a column before mapping it to a byte offset, which is why both sides have to
/// come through here.
pub fn stats_row_pad(content_width: usize, indent: u16, text_width: usize) -> u16 {
    center_pad(content_width, text_width).saturating_sub(indent)
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
/// The line is **centered** ([`center_pad`]): it is a HUD strip on the turn
/// boundary, not a log row — the rule above it is full-width, and the number
/// that used to sit centered in that rule now sits centered under it. The
/// frozen row written at task end lands on the same column: it bakes the same
/// pad into its line.
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
    let pad = center_pad(area.width as usize, UnicodeWidthStr::width(body.as_str()));
    let line = Line::from(Span::styled(
        format!("{}{}", " ".repeat(pad as usize), body),
        Style::default().fg(theme.accent).bg(theme.bg),
    ));
    frame.render_widget(Paragraph::new(line).style(base), area);

    // The mascot walks the blank columns either side of the text — the only
    // place on this row it can move without covering a number. Drawn last, on
    // top of the row background the band just painted. `theme.warning` on
    // purpose: the row's own text is `accent`, and a critter in the same red
    // reads as part of the sentence.
    if let Some(cells) = mascot_cells(area, pad, ctx.spinner_frame) {
        let style = Style::default().fg(theme.warning).bg(theme.bg);
        let buf = frame.buffer_mut();
        for (x, glyph) in cells {
            if x < area.right() {
                buf[(x, area.y)]
                    .set_symbol(glyph.encode_utf8(&mut [0; 4]))
                    .set_style(style);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        MASCOT_LEFT, MASCOT_MIN_RUNWAY, MASCOT_RIGHT, mascot_cells, mascot_step, task_stats_body,
    };
    use crate::{i18n::Messages, state::StatusBarState};
    use ratatui::layout::Rect;

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

    /// One full round trip on a six-column runway: out to the far end, back to
    /// the near one, and out again — no skipped endpoint, no stuck position.
    #[test]
    fn the_mascot_walks_to_the_far_end_and_back() {
        let walk: Vec<(u16, bool)> = (0..12).map(|t| mascot_step(t, 6)).collect();

        assert_eq!(
            walk,
            vec![
                (0, true),
                (1, true),
                (2, true),
                (3, true),
                (4, true),
                (5, true),
                (4, false),
                (3, false),
                (2, false),
                (1, false),
                (0, true),
                (1, true),
            ]
        );
    }

    /// A terminal cannot mirror a glyph, so the turn *is* the swap. If the swap
    /// lands anywhere but the two ends, the mascot walks backwards half the way.
    #[test]
    fn the_mascot_turns_to_face_the_way_it_is_walking() {
        let area = Rect::new(0, 0, 40, 1);
        let facing: Vec<char> = (0..14)
            .map(|t| mascot_cells(area, 10, t).expect("runway")[0].1)
            .collect();

        // runway 8 -> leg 7: out on 0..=7, back on 8..=13.
        assert_eq!(facing[0], MASCOT_RIGHT, "leaves the near end facing out");
        assert_eq!(facing[7], MASCOT_RIGHT, "still faces out at the far end");
        assert_eq!(facing[8], MASCOT_LEFT, "first step back is the swap");
        assert_eq!(facing[13], MASCOT_LEFT, "still faces back at the near end");
    }

    /// The mascot's whole reason for living on the padding: it must never stand
    /// on a column the text uses, at any tick or any width.
    #[test]
    fn the_mascot_stays_clear_of_the_text_columns() {
        let area = Rect::new(5, 3, 80, 1);

        for pad in MASCOT_MIN_RUNWAY + 2..40u16 {
            for tick in 0..64 {
                let [left, right] = mascot_cells(area, pad, tick).expect("a runway this wide");

                assert!(
                    (area.x + 1..=area.x + pad - 2).contains(&left.0),
                    "left mascot at {} is outside the left runway (pad {pad})",
                    left.0
                );
                assert!(
                    (area.x + area.width - pad + 1..=area.x + area.width - 2).contains(&right.0),
                    "right mascot at {} is outside the right runway (pad {pad})",
                    right.0
                );
            }
        }
    }

    /// Mirror phase: the two walk in together and turn together, so each faces
    /// its own direction of travel — never the same way.
    #[test]
    fn the_two_mascots_face_opposite_ways() {
        let area = Rect::new(0, 0, 60, 1);

        for tick in 0..40 {
            let [left, right] = mascot_cells(area, 12, tick).expect("runway");

            assert_ne!(
                left.1, right.1,
                "tick {tick}: both mascots face the same way"
            );
        }
    }

    /// A row with no padding to give away has no mascot — no half-drawn critter
    /// pressed against the numbers, and no separate narrow-terminal branch.
    #[test]
    fn a_row_without_padding_has_no_mascot() {
        let area = Rect::new(0, 0, 40, 1);

        for pad in 0..MASCOT_MIN_RUNWAY + 2 {
            assert!(
                mascot_cells(area, pad, 7).is_none(),
                "pad {pad} has no runway worth using"
            );
        }
        assert!(
            mascot_cells(area, MASCOT_MIN_RUNWAY + 2, 0).is_some(),
            "the first usable pad must actually draw one"
        );
    }

    /// Runways of 0 or 1 columns, and a counter about to wrap, must not panic —
    /// they can only ever stand still. (A 1-column runway's only position *is*
    /// the far end, so it faces out.)
    #[test]
    fn a_degenerate_runway_does_not_panic() {
        assert_eq!(mascot_step(0, 0), (0, true));
        assert_eq!(mascot_step(9, 1), (0, true));
        assert_eq!(mascot_step(u32::MAX, 2), (1, true));
        assert_eq!(mascot_step(u32::MAX.wrapping_add(1), 2), (0, true));
    }

    #[test]
    fn body_is_localized_and_clamps_negative_seconds() {
        let msgs = Messages::by_language(crate::i18n::Language::Chinese);
        let bar = status_bar("");

        assert_eq!(task_stats_body(&msgs, -3, &bar), "任务统计：⏱ 00:00");
    }
}
