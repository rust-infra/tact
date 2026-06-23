//! Renderable cell for tool invocation results.
//!
//! # Architecture
//!
//! `ToolCell` is the *render* half of a two-stage tool rendering pipeline:
//!
//! ```text
//!   agent.rs                    ToolWidget (widgets/)           ToolCell (render/cells/)
//! ══════════                   ════════════════════            ════════════════════════
//!   handle_step_completed      从 Theme/Messages 构建
//!      │                       摘要行 + detail 预览
//!      │                               │
//!      │                      .build() → ToolRenderOutput
//!      │                               │
//!      │          ┌────────────────────┘
//!      │          ▼
//!      │   ToolCell::from_output(...)   ← 拷贝所有数据为 owned
//!      │          │
//!      │   push into LogColumnRenderer  ← 走 Renderable 体系
//!      ▼
//! ```
//!
//! ## Why two types?
//!
//! `ToolWidget` borrows `Theme` and `Messages` — it's a temporary builder that needs
//! i18n and color lookups. `ToolCell` owns everything so it can be stored in the
//! `LogColumnRenderer` cell list and rendered across many frames without holding
//! lifetime-bound references.
//!
//! ## What it replaces
//!
//! Currently (before migration), tool results are rendered in two disjoint places:
//!
//! 1. **Summary line** — pushed into `App.messages[]` as a plain string, rendered
//!    by `TextCell` (see `render/cells/text.rs`).
//! 2. **Detail card** — stored as `DiffBlock` (start_idx / end_idx / file_path /
//!    preview_lines), drawn as a ratatui overlay by `render_diff_cards()` in
//!    `render/log.rs`.
//!
//! This split means the summary and the card are decoupled: the card's position is
//! computed from a `DiffBlock.placeholder_index` that `agent.rs` manually injects
//! into `messages[]`. That's fragile — any upstream layout change can misalign
//! the overlay.
//!
//! `ToolCell` bundles both into **one** `Renderable` unit with a deterministic
//! `height()`. The `LogColumnRenderer` then treats it like any other cell —
//! scrolling, clipping, and hit-testing all work out of the box.
//!
//! ## Visual layout
//!
//! ```text
//! ┌─ cell boundary ──────────────────────────────────────────┐
//! │  ✔ Step 3: write_file (src/main.rs)          [0.3s]      │  ← summary line (always 1 row)
//! │  ╭─────────────────────────────────────────────────────╮ │
//! │  │ Wrote src/main.rs (15 lines)                        │ │  ← card: top border + title
//! │  │                                                     │ │
//! │  │    1 + use std::io;                                 │ │  ← preview lines (N rows)
//! │  │    2 + fn main() {                                  │ │
//! │  │  ... and 10 more lines                              │ │  ← overflow line (1 row, optional)
//! │  │                                                     │ │
//! │  ╰▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔╯ │  ← card: bottom border + label
//! └─────────────────────────────────────────────────────────┘
//! ```
//!
//! Height formula: `1 (summary) + 1 (top border) + N (preview + optional overflow) + 1 (bottom border)`.
//!
//! ## Rendering with `skip_lines`
//!
//! `LogColumnRenderer` passes a `skip_lines` offset when only a slice of the cell
//! is visible. The cell's own row numbering is:
//!
//! ```text
//! row 0: summary
//! row 1: card top border
//! row 2..2+N-1: card content
//! row 2+N: card bottom border
//! ```
//!
//! `render_partial` maps `skip_lines` to the visible subset:
//!
//! | skip_lines | visible portion         | drawn area        |
//! |------------|-------------------------|-------------------|
//! | 0          | summary + full card     | area               |
//! | 1          | card (top border first) | area               |
//! | 2..        | card inner only         | area (no borders) |
//!
//! When `skip_lines > 0` the card's top border is off-screen, so we skip
//! the border decoration and render only the inner content rows. A full
//! partial-border implementation will be added when the cell is wired into
//! the pipeline.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph, Widget},
};

use crate::{
    render::renderable::Renderable,
    widgets::tool_widget::{ToolPhase, ToolRenderOutput},
};

/// Owned visual data for one tool invocation result in the log column.
///
/// All fields are `'static` so the cell can be stored in `LogColumnRenderer`'s
/// `Vec<(usize, Box<dyn Renderable>)>` without lifetime constraints.
pub(crate) struct ToolCell {
    /// Pre-built styled summary line (e.g. "✔ Step 3: write_file (src/main.rs)").
    summary_line: Line<'static>,
    /// Raw text of the summary (reserved for future search highlighting, like
    /// `TextCell.raw_text`).
    _summary_raw: String,
    /// Execution phase (Success / Failure / Running). Currently a placeholder;
    /// will be inferred from `StepResult` when `ToolCell` is wired into `agent.rs`.
    phase: ToolPhase,
    /// Whether this tool produced a detail card (file content preview, etc.).
    has_detail_card: bool,
    /// Card title — e.g. "Wrote src/main.rs (15 lines)". Displayed in the top border.
    detail_title: Option<String>,
    /// First N lines of the tool output, pre-split. Stored as `Vec<String>` instead
    /// of `Vec<Line>` because the per-line styling (line numbers, "+" gutter) is
    /// built at render time in `detail_card_lines()`.
    detail_preview: Vec<String>,
    /// Total number of lines in the tool output (may be greater than
    /// `detail_preview.len()` when the output is truncated).
    detail_total_lines: usize,
    /// Decorative bottom border label, e.g. "▔▔▔" — set from i18n messages.
    card_bottom: String,
    /// Overflow message template, e.g. "... and {} more lines" — set from i18n.
    overflow_tmpl: String,
    // ── Theme colors (copied at construction time) ──────────────────────
    accent: Color,
    bg: Color,
    fg: Color,
    success: Color,
}

impl ToolCell {
    /// Build an owned `ToolCell` from the builder's output plus theme / i18n data.
    ///
    /// This is a pure data copy — the `ToolRenderOutput` owns no borrowed state,
    /// so this is just moving fields and adding the color/text singletons.
    ///
    /// # Parameters
    ///
    /// - `accent`, `bg`, `fg`, `success`: colors from `Theme`
    /// - `card_bottom`: i18n message for the card's bottom border decoration
    /// - `overflow_tmpl`: i18n template for the "... and N more lines" line
    pub(crate) fn from_output(
        output: ToolRenderOutput,
        accent: Color,
        bg: Color,
        fg: Color,
        success: Color,
        card_bottom: String,
        overflow_tmpl: String,
    ) -> Self {
        Self {
            summary_line: output.summary,
            _summary_raw: output.summary_raw,
            phase: ToolPhase::Success, // placeholder; will be set from StepResult.status
            has_detail_card: output.layout.has_detail_card,
            detail_title: output.detail_title,
            detail_preview: output.detail_preview,
            detail_total_lines: output.detail_total_lines,
            card_bottom,
            overflow_tmpl,
            accent,
            bg,
            fg,
            success,
        }
    }

    /// Number of content rows *inside* the card borders (excluding the top and
    /// bottom border lines themselves).
    ///
    /// Equals `detail_preview.len()` plus one extra row for the overflow message
    /// when `total_lines > preview.len()`.
    fn card_inner_rows(&self) -> usize {
        if !self.has_detail_card {
            return 0;
        }
        let overflow = if self.detail_total_lines > self.detail_preview.len() {
            1
        } else {
            0
        };
        self.detail_preview.len() + overflow
    }

    /// Build the styled content lines for the card's interior, given the available
    /// inner width (already accounting for the 1-char border on each side).
    ///
    /// Each line has the format:
    ///
    /// ```text
    ///  NN + content…
    /// ```
    ///
    /// where `NN` is a right-aligned line number padded to `num_width` characters,
    /// followed by a green `+` gutter and the (possibly truncated) content.
    ///
    /// If `total_lines > preview.len()`, a final `"... and N more lines"` row
    /// is appended using the `overflow_tmpl` template.
    ///
    /// # Width allocation
    ///
    /// ```text
    /// |__ num_width __||_ gutter _||_________ code_width ________|
    ///   NN             +           content…
    /// ```
    ///
    /// `num_width` is at least 3 (to accommodate "999") and grows with the
    /// total line count (e.g. 4 digits for 1000+ lines).
    /// `code_width = width - num_width - 3` (the 3 includes the `+ ` gutter
    /// and one extra safety char).
    fn detail_card_lines(&self, width: u16) -> Vec<Line<'static>> {
        if !self.has_detail_card {
            return Vec::new();
        }
        let num_width = (self.detail_total_lines + 1).to_string().len().max(3);
        let code_width = (width as usize).saturating_sub(num_width + 3);

        let num_style = Style::default().fg(Color::Gray).bg(self.bg);
        let text_style = Style::default().fg(self.fg).bg(self.bg);
        let plus_style = Style::default().fg(self.success).bg(self.bg);

        let mut lines: Vec<Line<'static>> = self
            .detail_preview
            .iter()
            .enumerate()
            .map(|(i, line)| {
                let num = format!("{:>nw$}", i + 1, nw = num_width);
                let trimmed: String = line.chars().take(code_width).collect();
                Line::from(vec![
                    Span::styled(format!(" {} ", num), num_style),
                    Span::styled("+ ", plus_style),
                    Span::styled(trimmed, text_style),
                ])
            })
            .collect();

        if self.detail_total_lines > self.detail_preview.len() {
            let remaining = self.detail_total_lines - self.detail_preview.len();
            lines.push(Line::from(Span::styled(
                self.overflow_tmpl.replace("{}", &remaining.to_string()),
                Style::default().fg(Color::Gray).bg(self.bg),
            )));
        }

        lines
    }
}

impl Renderable for ToolCell {
    /// Total cell height in rows, independent of viewport.
    ///
    /// ```text
    /// no card:    1 (summary only)
    /// with card:  1 + 1 + card_inner_rows + 1
    ///              ^   ^   ^                 ^
    ///              |   |   |                 bottom border
    ///              |   |   preview + optional overflow
    ///              |   top border
    ///              summary
    /// ```
    fn height(&self, _width: u16) -> u16 {
        if self.has_detail_card {
            1_u16 + 1 + self.card_inner_rows() as u16 + 1
        } else {
            1
        }
    }

    /// Render a rectangular slice of this cell into `buf`, starting `skip_lines`
    /// rows past the top of the cell.
    ///
    /// `skip_lines` is the mechanism by which `LogColumnRenderer` handles scroll:
    /// when a cell starts above the viewport, only the portion that overlaps the
    /// visible area is drawn. The cell doesn't know its absolute position — it
    /// only knows how many of its own rows to skip.
    ///
    /// # Row layout within the cell
    ///
    /// ```text
    /// row 0:          summary line (always present)
    /// row 1:          card top border (only if has_detail_card)
    /// row 2..2+N-1:   card preview lines + optional overflow row
    /// row 2+N:        card bottom border
    /// ```
    ///
    /// # skip_lines semantics
    ///
    /// | skip_lines | What's visible                      |
    /// |------------|-------------------------------------|
    /// | 0          | Summary + full card with borders    |
    /// | 1          | Card with borders (summary clipped) |
    /// | 2..2+N-1   | Card interior only (borders clipped)|
    /// | ≥ height   | Nothing — cell is fully off-screen  |
    fn render_partial(&self, area: Rect, buf: &mut Buffer, skip_lines: usize) {
        if area.height == 0 || area.width == 0 {
            return;
        }

        // ── Row 0: summary ──────────────────────────────────────────
        // Only drawn when skip_lines == 0 (the summary is the first row).
        if skip_lines == 0 {
            let summary_area = Rect::new(area.x, area.y, area.width, 1);
            Paragraph::new(vec![self.summary_line.clone()])
                .style(Style::default().fg(self.fg).bg(self.bg))
                .render(summary_area, buf);
        }

        // No card → done.
        if !self.has_detail_card {
            return;
        }

        let card_total = 1 + self.card_inner_rows() + 1; // top_border + inner_rows + bottom_border

        // ── Map skip_lines to card-relative coordinates ─────────────
        //
        // If the summary is visible (skip_lines == 0), the card starts at
        // `area.y + 1`. If the summary is already skipped (skip_lines >= 1),
        // the card top row lands at `area.y`.
        let (card_area_y_offset, card_skip) = if skip_lines == 0 {
            (1, 0) // summary visible → card starts one row down
        } else {
            (0, skip_lines.saturating_sub(1)) // summary clipped → card starts at area.y
        };

        // Entire card is above the viewport — nothing to draw.
        if card_skip >= card_total {
            return;
        }

        // Ensure there's room for the card after the offset.
        let remaining_h = area.height.saturating_sub(card_area_y_offset as u16);
        if remaining_h == 0 {
            return;
        }
        let card_area = Rect::new(area.x, area.y + card_area_y_offset, area.width, remaining_h);

        // ── Case A: full card (card_skip == 0) ──────────────────────
        //
        // The card's top border is visible. Draw the full block with rounded
        // borders, title, and title_bottom label.
        if card_skip == 0 {
            let title = self.detail_title.clone().unwrap_or_default();

            let card_block = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(self.accent))
                .style(Style::default().bg(self.bg))
                .title(title)
                .title_bottom(Line::from(Span::styled(
                    &self.card_bottom,
                    Style::default()
                        .fg(self.accent)
                        .add_modifier(Modifier::ITALIC),
                )));

            card_block.render(card_area, buf);

            // The block's borders consume 1 char on each side.
            let inner = Rect::new(
                card_area.x + 1,
                card_area.y + 1,
                card_area.width.saturating_sub(2),
                card_area.height.saturating_sub(2),
            );

            if inner.height > 0 {
                let lines = self.detail_card_lines(inner.width);
                Paragraph::new(lines)
                    .style(Style::default().bg(self.bg))
                    .render(inner, buf);
            }
            return;
        }

        // ── Case B: partial card (card_skip >= 1) ───────────────────
        //
        // The card's top border is off-screen. We skip the border decoration
        // (no rounded corners, no title) and draw only the interior content
        // rows that fall within the viewport.
        //
        // TODO: when ToolCell replaces diff.rs, implement partial border
        // drawing so that the left/right/bottom borders still appear when
        // the top is scrolled away.
        let inner_skip = card_skip.saturating_sub(1); // skip past the (off-screen) top border
        let inner = Rect::new(
            card_area.x + 1,
            card_area.y + 1,
            card_area.width.saturating_sub(2),
            card_area.height.saturating_sub(2),
        );

        if inner.height > 0 {
            let all = self.detail_card_lines(inner.width);
            let visible: Vec<Line<'static>> = all
                .into_iter()
                .skip(inner_skip)
                .take(inner.height as usize)
                .collect();
            if !visible.is_empty() {
                Paragraph::new(visible)
                    .style(Style::default().bg(self.bg))
                    .render(inner, buf);
            }
        }
    }

    /// Full render (convenience shorthand for `render_partial(area, buf, 0)`).
    fn render(&self, area: Rect, buf: &mut Buffer) {
        self.render_partial(area, buf, 0);
    }
}

// ── Tests ────────────────────────────────────────────────────────────
//
// Tests use `ToolRenderOutput` / `ToolLayout` from `widgets::tool_widget`
// to construct realistic cell data without depending on the full `AgentUpdate`
// pipeline.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::widgets::tool_widget::ToolLayout;

    /// Build a `ToolRenderOutput` for a hypothetical "Step 1: write_file".
    ///
    /// `has_card` controls whether the card is present; `preview_count` sets
    /// how many preview lines are stored; `total` is the real file line count.
    fn make_output(has_card: bool, preview_count: usize, total: usize) -> ToolRenderOutput {
        let preview: Vec<String> = (1..=preview_count)
            .map(|i| format!("line-{i:02}"))
            .collect();
        let overflow = if total > preview_count { 1 } else { 0 };
        ToolRenderOutput {
            summary: Line::from("✔ Step 1: write_file (src/main.rs)"),
            summary_raw: "✔ Step 1: write_file (src/main.rs)".into(),
            layout: ToolLayout {
                // placeholder_lines is the legacy layout field; tests don't use it
                placeholder_lines: if has_card {
                    2 + preview_count + overflow + 1
                } else {
                    1
                },
                preview_lines: preview_count,
                has_detail_card: has_card,
            },
            detail_title: if has_card {
                Some("Wrote src/main.rs (15 lines)".into())
            } else {
                None
            },
            detail_preview: preview,
            detail_total_lines: total,
        }
    }

    /// Construct a `ToolCell` with a fixed theme for deterministic testing.
    fn tool_cell(output: ToolRenderOutput) -> ToolCell {
        ToolCell::from_output(
            output,
            Color::Cyan,       // accent
            Color::Black,      // bg
            Color::White,      // fg
            Color::Green,      // success
            "▔▔▔".into(),      // card_bottom
            "... and {} more lines".into(),
        )
    }

    #[test]
    fn height_no_card_is_one() {
        let cell = tool_cell(make_output(false, 0, 0));
        assert_eq!(cell.height(80), 1);
    }

    #[test]
    fn height_with_card_no_overflow() {
        // 10 preview lines, 10 total → no overflow row needed
        let cell = tool_cell(make_output(true, 10, 10));
        // 1 (summary) + 1 (top border) + 10 (preview) + 1 (bottom border) = 13
        assert_eq!(cell.height(80), 13);
    }

    #[test]
    fn height_with_card_and_overflow() {
        // 10 preview lines, 15 total → overflow row present
        let cell = tool_cell(make_output(true, 10, 15));
        // 1 + 1 + (10 preview + 1 overflow) + 1 = 14
        assert_eq!(cell.height(80), 14);
    }

    #[test]
    fn height_with_card_small_preview() {
        let cell = tool_cell(make_output(true, 3, 3));
        assert_eq!(cell.height(80), 1 + 1 + 3 + 1);
    }

    #[test]
    fn renders_into_buffer_without_panicking() {
        let cell = tool_cell(make_output(true, 3, 5));
        let area = Rect::new(0, 0, 60, 10);
        let mut buf = Buffer::empty(area);
        cell.render(area, &mut buf);
        // Sanity: buffer area unchanged
        assert_eq!(buf.area, area);
    }

    #[test]
    fn render_no_card() {
        let cell = tool_cell(make_output(false, 0, 0));
        let area = Rect::new(0, 0, 60, 2);
        let mut buf = Buffer::empty(area);
        cell.render(area, &mut buf);
        assert_eq!(buf.area, area);
    }

    #[test]
    fn render_partial_skip_summary_only() {
        // Card is present, skip_lines=1: summary is clipped, card renders from
        // its top border.
        let cell = tool_cell(make_output(true, 5, 5));
        let area = Rect::new(0, 0, 60, 10);
        let mut buf = Buffer::empty(area);
        cell.render_partial(area, &mut buf, 1);
        assert_eq!(buf.area, area);
    }

    #[test]
    fn render_partial_skip_beyond_card_is_noop() {
        // Card has 3 preview + 0 overflow = 3 inner rows.
        // Total cell = 6 rows. skip_lines=20 → entire cell off-screen.
        let cell = tool_cell(make_output(true, 3, 3));
        let area = Rect::new(0, 0, 60, 10);
        let mut buf = Buffer::empty(area);
        cell.render_partial(area, &mut buf, 20);
        assert_eq!(buf.area, area);
    }
}
