//! Scroll-back-to-bottom floating pill (C-tier overlay).
//!
//! Drawn on the bottom border row of the Log panel (or the sticky host strip
//! when visible). A band-background pill that says the reader has scrolled
//! away and new output may have arrived.
//!
//! Design: `docs/superpowers/specs/2026-10-10-scroll-back-to-bottom-pill-design.md`.

use ratatui::{
    Frame,
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::{Clear, Paragraph},
};
use unicode_width::UnicodeWidthStr;

use crate::render::ctx::RenderCtx;

/// Rounded end caps — Nerd Font Powerline "half circle thick" glyphs.
///
/// A terminal cell is a rectangle, so a one-row pill cannot carry a real
/// `border-radius`; these two glyphs are the standard stand-in. Drawn in the
/// band colour against the panel background they round off the left and right
/// ends, which is what makes the band read as a pill rather than as a
/// selection highlight.
///
/// Verified against the local Nerd Fonts (`JetBrainsMonoNerdFontMono`,
/// `HackNerdFontMono`, `0xProtoNerdFontMono`): both advance exactly one column
/// (identical to `0`), their ink boxes span the full cell height, and each
/// overhangs its own cell slightly — so a cap meets the band with no seam.
pub const CAP_LEFT: char = '\u{e0b6}'; // ple-left_half_circle_thick
pub const CAP_RIGHT: char = '\u{e0b4}'; // ple-right_half_circle_thick

/// Minimum width the pill needs, including caps, padding and margin.
const PILL_MIN_WIDTH: u16 = 30;

/// Blank columns between a cap and the text.
const CAP_PADDING: u16 = 1;

/// Render the scroll-back pill on the bottom border row of `area`.
///
/// `area` is the panel whose bottom border hosts the pill:
/// - the Log panel rectangle when no sticky host is visible;
/// - the sticky host rectangle when it is.
///
/// Returns the hit rectangle (empty when the pill is not drawn).
pub fn render_scroll_pill(frame: &mut Frame, area: Rect, ctx: &RenderCtx) -> Rect {
    if ctx.log_following || area.width < PILL_MIN_WIDTH {
        return Rect::default();
    }

    let msgs = &ctx.messages;
    let theme = ctx.theme;

    // Build the text: badge (optional) + action (which already carries the key).
    let mut text = String::new();
    if ctx.log_unseen {
        text.push_str(msgs.scroll_back_badge);
        text.push_str(" · ");
    }
    text.push_str(msgs.scroll_back_action);

    let text_width = UnicodeWidthStr::width(text.as_str()) as u16;
    // cap + padding on each side.
    let pill_width = text_width.saturating_add(2 * (1 + CAP_PADDING));

    // Need room for left border + pill + right margin.
    if pill_width.saturating_add(2) > area.width {
        return Rect::default();
    }

    // Dock to the right, leaving a 1-column margin inside the right border.
    let pill_x = area.right().saturating_sub(pill_width + 1);
    let pill_y = area.y + area.height.saturating_sub(1);
    let pill_area = Rect::new(pill_x, pill_y, pill_width, 1);

    // Overlay invariant: clear first, then paint the band, then the glyphs.
    frame.render_widget(Clear, pill_area);

    let band_bg = theme.status_bar_bg;
    let panel_bg = theme.bg;

    {
        let buf = frame.buffer_mut();
        // Interior: solid band. (The caps are skipped here.)
        for x in (pill_area.left() + 1)..(pill_area.right() - 1) {
            buf[(x, pill_y)]
                .set_symbol(" ")
                .set_style(Style::default().bg(band_bg));
        }
        // Caps: the band colour as a rounded shape over the panel background.
        // That inversion is what draws the corner — a cap cell is *not* a band
        // cell, so the rounded end is visible instead of a hard rectangle edge.
        let cap_style = Style::default().fg(band_bg).bg(panel_bg);
        buf[(pill_area.left(), pill_y)]
            .set_symbol(CAP_LEFT.encode_utf8(&mut [0; 4]))
            .set_style(cap_style);
        buf[(pill_area.right() - 1, pill_y)]
            .set_symbol(CAP_RIGHT.encode_utf8(&mut [0; 4]))
            .set_style(cap_style);
    }

    let muted = Style::default().fg(theme.muted_fg()).bg(band_bg);
    let warning = Style::default().fg(theme.warning).bg(band_bg);

    let mut spans: Vec<Span<'static>> = Vec::new();
    if ctx.log_unseen {
        spans.push(Span::styled(msgs.scroll_back_badge, muted));
        spans.push(Span::styled(" · ", muted));
    }
    spans.push(Span::styled(msgs.scroll_back_action, warning));

    let inner = Rect::new(pill_area.x + 1 + CAP_PADDING, pill_area.y, text_width, 1);
    frame.render_widget(Paragraph::new(Line::from(spans)), inner);

    pill_area
}

#[cfg(test)]
mod tests {
    // Buffer-level tests live in `crates/tui/src/render/layout.rs` where a full
    // `App` can build a `RenderCtx`.
}
