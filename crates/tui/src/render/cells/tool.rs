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
//!
//! # TODO — Next steps
//!
//! The cell is currently a scaffold; no production code path constructs it yet.
//! Planned integration order (each step can be its own commit):
//!
//! ```text
//! ┌─ TODO(step 1): wire ToolCell construction into agent.rs ──── DONE ✓ ──┐
//! │                                                                       │
//! │  In handle_step_completed:                                            │
//! │    a. Call ToolWidget::build() to produce ToolRenderOutput            │
//! │    b. Pull accent/bg/fg/success from self.theme                       │
//! │    c. Pull card_bottom/overflow_tmpl from self.msgs                   │
//! │    d. ToolCell::from_output(...) → Box<dyn Renderable>                │
//! │    e. Push into LogColumnRenderer alongside other cells               │
//! │                                                                       │
//! │  Files: widgets/state/app/agent.rs                                    │
//! └───────────────────────────────────────────────────────────────────────┘
//!
//! ┌─ TODO(step 2): set phase from StepResult.status ── DONE ✓ ────────────┐
//! │                                                                       │
//! │  ToolCell.phase is currently hardcoded to ToolPhase::Success. After   │
//! │  step 1, pass the actual StepStatus through ToolWidget and into       │
//! │  ToolRenderOutput, then use it in from_output. The phase drives the   │
//! │  summary icon color: ✔ green / ✖ red / ⏳ yellow.                     │
//! │                                                                       │
//! │  Files: render/cells/tool.rs, widgets/tool_widget.rs                  │
//! └───────────────────────────────────────────────────────────────────────┘
//!
//! ┌─ TODO(step 3): remove placeholder-row injection from agent.rs (partial) ─ DONE ✓ ┐
//! │                                                                       │
//! │  Placeholder rows are still injected for visual positioning. Once     │
//! │  ToolCell handles its own height() and independent positioning,       │
//! │  delete:                                                              │
//! │    - The blank message push in agent.rs                               │
//! │    - The placeholder_names tracking                                   │
//! │                                                                       │
//! │  Files: widgets/state/app/agent.rs                                    │
//! └───────────────────────────────────────────────────────────────────────┘
//!
//! ┌─ TODO(step 4): remove render_diff_cards overlay from log.rs ── DONE ✓ ┐
//! │                                                                       │
//! │  render_diff_cards() has been removed. ToolCells are now pushed       │
//! │  directly into LogColumnRenderer in the Phase 3 while loop.           │
//! │  render/cells/diff.rs has been deleted.                               │
//! │                                                                       │
//! │  Done in: render/log.rs, render/cells/mod.rs                          │
//! └───────────────────────────────────────────────────────────────────────┘
//!
//! ┌─ TODO(step 5): partial border drawing when card top is scrolled away ── DONE ✓ ┐
//! │                                                                       │
//! │  Currently when skip_lines > 0, the card's top border is off-screen   │
//! │  and we draw only raw content lines with no border decoration.        │
//! │  Implement partial Block rendering: draw left + right + bottom        │
//! │  borders (and possibly a clipped title) when only a portion of the    │
//! │  card is visible.                                                     │
//! │                                                                       │
//! │  Files: render/cells/tool.rs (render_partial, case B)                 │
//! └───────────────────────────────────────────────────────────────────────┘
//!
//! ┌─ TODO(step 6): wire _summary_raw into search ── DEFERRED ─────────────┐
//! │                                                                       │
//! │  _summary_raw is the unstyled copy of the summary line. Once search   │
//! │  covers LogColumnRenderer cells (not just TextCell raw_text), include │
//! │  this field in the search corpus.                                     │
//! │                                                                       │
//! │  Files: render/cells/tool.rs, widgets/state/app/search.rs             │
//! └───────────────────────────────────────────────────────────────────────┘
//! ```

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
    ///
    /// Raw text of the summary line. Already covered by `raw_messages`-based
    /// search; reserved for future cell-based search in LogColumnRenderer.
    _summary_raw: String,
    /// Execution phase (Success / Failure / Running).
    phase: ToolPhase,
    /// When true, the summary line is not rendered by this cell (it's handled by
    /// a separate `TextCell` in `messages[]`). Only the detail card is drawn.
    card_only: bool,
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
    /// - `card_only`: when true, the summary is not rendered by this cell (it's
    ///   already handled by a separate `TextCell`). Only the detail card is drawn.
    /// - `accent`, `bg`, `fg`, `success`: colors from `Theme`
    /// - `card_bottom`: i18n message for the card's bottom border decoration
    /// - `overflow_tmpl`: i18n template for the "... and N more lines" line
    pub(crate) fn from_output(
        output: ToolRenderOutput,
        card_only: bool,
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
            phase: output.phase,
            card_only,
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
    /// card_only = false (full cell):
    ///   no card:    1 (summary only)
    ///   with card:  1 + 1 + card_inner_rows + 1
    ///                ^   ^   ^                 ^
    ///                |   |   |                 bottom border
    ///                |   |   preview + optional overflow
    ///                |   top border
    ///                summary
    ///
    /// card_only = true (summary handled by separate TextCell):
    ///   no card:    0 (nothing to draw — shouldn't happen)
    ///   with card:  1 + card_inner_rows + 1
    ///                ^   ^                 ^
    ///                |   |                 bottom border
    ///                |   preview + optional overflow
    ///                top border
    /// ```
    fn height(&self, _width: u16) -> u16 {
        if self.card_only {
            if self.has_detail_card {
                1_u16 + self.card_inner_rows() as u16 + 1
            } else {
                0
            }
        } else if self.has_detail_card {
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
    /// Normal mode (`card_only = false`):
    ///
    /// ```text
    /// row 0:          summary line (always present)
    /// row 1:          card top border (only if has_detail_card)
    /// row 2..2+N-1:   card preview lines + optional overflow row
    /// row 2+N:        card bottom border
    /// ```
    ///
    /// Card-only mode (`card_only = true`):
    ///
    /// ```text
    /// row 0:          card top border (only if has_detail_card)
    /// row 1..1+N-1:   card preview lines + optional overflow row
    /// row 1+N:        card bottom border
    /// ```
    ///
    /// # skip_lines semantics (normal mode)
    ///
    /// | skip_lines | What's visible                      |
    /// |------------|-------------------------------------|
    /// | 0          | Summary + full card with borders    |
    /// | 1          | Card with borders (summary clipped) |
    /// | 2..2+N-1   | Card interior only (borders clipped)|
    /// | ≥ height   | Nothing — cell is fully off-screen  |
    ///
    /// # skip_lines semantics (card-only mode)
    ///
    /// | skip_lines | What's visible                      |
    /// |------------|-------------------------------------|
    /// | 0          | Card with borders                   |
    /// | 1..1+N-1   | Card interior only (borders clipped)|
    /// | ≥ height   | Nothing — cell is fully off-screen  |
    fn render_partial(&self, area: Rect, buf: &mut Buffer, skip_lines: usize) {
        if area.height == 0 || area.width == 0 {
            return;
        }

        // ── Summary (normal mode only) ───────────────────────────
        if !self.card_only {
            if skip_lines == 0 {
                let summary_area = Rect::new(area.x, area.y, area.width, 1);
                Paragraph::new(vec![self.summary_line.clone()])
                    .style(Style::default().fg(self.fg).bg(self.bg))
                    .render(summary_area, buf);
            }
        }

        // No card → done.
        if !self.has_detail_card {
            return;
        }

        let card_total = 1 + self.card_inner_rows() + 1; // top_border + inner_rows + bottom_border

        // ── Map skip_lines to card-relative coordinates ─────────
        //
        // In normal mode the summary occupies row 0; in card_only mode
        // the card starts at row 0. We compute `card_skip` (how many card
        // rows to skip) and whether the card starts at area.y or area.y+1.
        let (card_area_y_offset, card_skip) = if self.card_only {
            // Row 0 is the card top border — no summary offset.
            (0, skip_lines)
        } else if skip_lines == 0 {
            // Summary visible (row 0); card starts at area.y + 1.
            (1, 0)
        } else {
            // Summary clipped; card starts at area.y.
            (0, skip_lines.saturating_sub(1))
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
        // The card's top border is off-screen. Draw left + right borders on
        // every visible row, and a bottom border with rounded corners when
        // the card's bottom edge is within the viewport.
        let inner_skip = card_skip.saturating_sub(1); // skip past the (off-screen) top border
        let show_bottom = card_skip + card_area.height as usize >= card_total;

        let mut borders = Borders::LEFT | Borders::RIGHT;
        if show_bottom {
            borders |= Borders::BOTTOM;
        }
        let bottom_space = if show_bottom { 1_u16 } else { 0 };

        let card_block = Block::default()
            .borders(borders)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(self.accent))
            .style(Style::default().bg(self.bg));
        card_block.render(card_area, buf);

        let inner = Rect::new(
            card_area.x + 1,
            card_area.y,
            card_area.width.saturating_sub(2),
            card_area.height.saturating_sub(bottom_space),
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
    use crate::widgets::tool_widget::{ToolLayout, ToolPhase};

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
            phase: ToolPhase::Success,
            arg_summary: "src/main.rs".into(),
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
        tool_cell_mode(output, false)
    }

    fn tool_cell_mode(output: ToolRenderOutput, card_only: bool) -> ToolCell {
        ToolCell::from_output(
            output,
            card_only,
            Color::Cyan,       // accent
            Color::Black,      // bg
            Color::White,      // fg
            Color::Green,      // success
            "▔▔▔".into(),      // card_bottom
            "... and {} more lines".into(),
        )
    }

    // ── Normal mode tests ───────────────────────────────────────

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

    // ── Card-only mode tests ────────────────────────────────────

    #[test]
    fn card_only_height_no_card_is_zero() {
        let cell = tool_cell_mode(make_output(false, 0, 0), true);
        assert_eq!(cell.height(80), 0);
    }

    #[test]
    fn card_only_height_with_card() {
        // 5 preview, 5 total → no overflow
        // height = 1 (top border) + 5 (preview) + 1 (bottom) = 7
        let cell = tool_cell_mode(make_output(true, 5, 5), true);
        assert_eq!(cell.height(80), 7);
    }

    #[test]
    fn card_only_height_with_overflow() {
        // 3 preview, 10 total → overflow row present
        // height = 1 + (3 + 1) + 1 = 6
        let cell = tool_cell_mode(make_output(true, 3, 10), true);
        assert_eq!(cell.height(80), 6);
    }

    #[test]
    fn card_only_renders_into_buffer() {
        let cell = tool_cell_mode(make_output(true, 3, 5), true);
        let area = Rect::new(0, 0, 60, 10);
        let mut buf = Buffer::empty(area);
        cell.render(area, &mut buf);
        assert_eq!(buf.area, area);
    }

    #[test]
    fn card_only_render_partial_skip_borders() {
        let cell = tool_cell_mode(make_output(true, 5, 5), true);
        let area = Rect::new(0, 0, 60, 10);
        let mut buf = Buffer::empty(area);
        // skip_lines=1: skip top border, render interior content
        cell.render_partial(area, &mut buf, 1);
        assert_eq!(buf.area, area);
    }
}
