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
//! │  │                                                     │ │
//! │  │                                                     │ │
//! │  ╰▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔╯ │  ← card: bottom border + label
//! └─────────────────────────────────────────────────────────┘
//! ```
//!
//! Height formula: `1 (summary) + 1 (top border) + N (preview) + 1 (bottom border)`.
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
//! │    c. Pull card_bottom from self.msgs                                  │
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

use std::time::Instant;

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph, Widget},
};

use crate::{
    i18n::Messages,
    render::renderable::Renderable,
    widgets::tool_widget::{
        TOOL_HEADER_ROWS, ToolPhase, ToolRenderOutput, build_meta_text, running_elapsed_us,
        tool_card_inner_rows, tool_visual_rows,
    },
};

pub(crate) struct ToolCell {
    title_line: Line<'static>,
    _title_raw: String,
    phase: ToolPhase,
    permission_label: Option<String>,
    error_message: Option<String>,
    duration_us: Option<u64>,
    size_bytes: Option<usize>,
    started_at: Option<Instant>,
    spinner_char: char,
    card_only: bool,
    has_detail_card: bool,
    use_diff_gutter: bool,
    detail_title: Option<String>,
    detail_preview: Vec<String>,
    detail_total_lines: usize,
    card_bottom: String,
    tool_phase_running: &'static str,
    tool_phase_success: &'static str,
    tool_phase_failed: &'static str,
    tool_meta_sep: &'static str,
    step_success_prefix: &'static str,
    step_fail_prefix: &'static str,
    accent: Color,
    bg: Color,
    fg: Color,
    success: Color,
    warning: Color,
    error: Color,
    card_border_type: BorderType,
}

impl ToolCell {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn from_output(
        output: ToolRenderOutput,
        started_at: Option<Instant>,
        spinner_char: char,
        card_only: bool,
        accent: Color,
        bg: Color,
        fg: Color,
        success: Color,
        warning: Color,
        error: Color,
        card_border_type: BorderType,
        msgs: &Messages,
    ) -> Self {
        Self {
            title_line: output.title_line,
            _title_raw: output.title_raw,
            phase: output.phase,
            permission_label: output.permission_label,
            error_message: output.error_message,
            duration_us: output.duration_us,
            size_bytes: output.size_bytes,
            started_at,
            spinner_char,
            card_only,
            has_detail_card: output.layout.has_detail_card,
            use_diff_gutter: output.use_diff_gutter,
            detail_title: output.detail_title,
            detail_preview: output.detail_preview,
            detail_total_lines: output.detail_total_lines,
            card_bottom: output.card_bottom,
            tool_phase_running: msgs.tool_phase_running,
            tool_phase_success: msgs.tool_phase_success,
            tool_phase_failed: msgs.tool_phase_failed,
            tool_meta_sep: msgs.tool_meta_sep,
            step_success_prefix: msgs.step_success_prefix,
            step_fail_prefix: msgs.step_fail_prefix,
            accent,
            bg,
            fg,
            success,
            warning,
            error,
            card_border_type,
        }
    }

    fn meta_line(&self) -> Line<'static> {
        let duration_us = if self.phase == ToolPhase::Running {
            self.started_at.map(running_elapsed_us).or(self.duration_us)
        } else {
            self.duration_us
        };
        let text = build_meta_text(
            self.phase,
            self.permission_label.as_deref(),
            self.size_bytes,
            duration_us,
            self.error_message
                .as_deref()
                .filter(|_| !(self.has_detail_card && self.phase == ToolPhase::Failed)),
            self.spinner_char,
            self.tool_phase_running,
            self.tool_phase_success,
            self.tool_phase_failed,
            self.tool_meta_sep,
            self.step_success_prefix,
            self.step_fail_prefix,
        );
        let style = match self.phase {
            ToolPhase::Running => Style::default().fg(self.warning),
            ToolPhase::Success => Style::default().fg(self.success),
            ToolPhase::Failed => Style::default().fg(self.error),
        };
        Line::from(Span::styled(text, style))
    }

    fn card_inner_rows(&self) -> usize {
        tool_card_inner_rows(self.detail_preview.len(), self.detail_total_lines)
    }

    fn detail_card_lines(&self, width: u16) -> Vec<Line<'static>> {
        if !self.has_detail_card {
            return Vec::new();
        }
        let num_width = (self.detail_total_lines + 1).to_string().len().max(3);
        let gutter_cols = if self.use_diff_gutter { 3 } else { 1 };
        let code_width = (width as usize).saturating_sub(num_width + gutter_cols + 1);

        let num_style = Style::default().fg(Color::Gray).bg(self.bg);
        let text_style = Style::default().fg(self.fg).bg(self.bg);
        let plus_style = Style::default().fg(self.success).bg(self.bg);

        let lines: Vec<Line<'static>> = self
            .detail_preview
            .iter()
            .enumerate()
            .map(|(i, line)| {
                let num = format!("{:>nw$}", i + 1, nw = num_width);
                let trimmed: String = line.chars().take(code_width).collect();
                if self.use_diff_gutter {
                    Line::from(vec![
                        Span::styled(format!(" {} ", num), num_style),
                        Span::styled("+ ", plus_style),
                        Span::styled(trimmed, text_style),
                    ])
                } else {
                    Line::from(vec![
                        Span::styled(format!(" {} ", num), num_style),
                        Span::styled(trimmed, text_style),
                    ])
                }
            })
            .collect();

        lines
    }

    fn card_bottom_text(&self) -> String {
        let base = self.card_bottom.trim();
        if self.detail_total_lines > self.detail_preview.len() {
            format!(
                " {}/{} lines | {} ",
                self.detail_preview.len(),
                self.detail_total_lines,
                base
            )
        } else {
            self.card_bottom.clone()
        }
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
        tool_visual_rows(
            self.has_detail_card,
            self.detail_preview.len(),
            self.detail_total_lines,
            self.card_only,
        ) as u16
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
        let area =
            crate::render::util::indent_rect(area, crate::render::util::LOG_TOOL_BLOCK_INDENT);
        if area.height == 0 || area.width == 0 {
            return;
        }

        // ── Header rows (title + meta) ───────────────────────────
        if !self.card_only {
            for row in 0..TOOL_HEADER_ROWS {
                if row < skip_lines {
                    continue;
                }
                let vis_off = row - skip_lines;
                if vis_off >= area.height as usize {
                    break;
                }
                let row_area = Rect::new(area.x, area.y + vis_off as u16, area.width, 1);
                if row == 0 {
                    Paragraph::new(vec![self.title_line.clone()])
                        .style(Style::default().fg(self.fg).bg(self.bg))
                        .render(row_area, buf);
                } else {
                    Paragraph::new(vec![self.meta_line()])
                        .style(Style::default().bg(self.bg))
                        .render(row_area, buf);
                }
            }
        }

        if !self.has_detail_card {
            return;
        }

        let card_total = 1 + self.card_inner_rows() + 1;

        let (card_area_y_offset, card_skip) = if self.card_only {
            (0, skip_lines)
        } else {
            (
                TOOL_HEADER_ROWS.saturating_sub(skip_lines),
                skip_lines.saturating_sub(TOOL_HEADER_ROWS),
            )
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
        let card_area = Rect::new(
            area.x,
            area.y + card_area_y_offset as u16,
            area.width,
            remaining_h,
        );

        // ── Case A: full card (card_skip == 0) ──────────────────────
        //
        // The card's top border is visible. Draw the full block with rounded
        // borders, title, and title_bottom label.
        if card_skip == 0 {
            let title = self.detail_title.clone().unwrap_or_default();

            let card_block = Block::default()
                .borders(Borders::ALL)
                .border_type(self.card_border_type)
                .border_style(
                    Style::default()
                        .fg(self.accent)
                        .add_modifier(Modifier::BOLD),
                )
                .style(Style::default().bg(self.bg))
                .title(Span::styled(
                    title,
                    Style::default()
                        .fg(self.accent)
                        .add_modifier(Modifier::BOLD),
                ))
                .title_bottom(Line::from(Span::styled(
                    self.card_bottom_text(),
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
            .border_type(self.card_border_type)
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
    use crate::widgets::tool_widget::{TOOL_HEADER_ROWS, ToolLayout, ToolPhase, tool_visual_rows};

    /// Build a `ToolRenderOutput` for a hypothetical "Step 1: write_file".
    ///
    /// `has_card` controls whether the card is present; `preview_count` sets
    /// how many preview lines are stored; `total` is the real file line count.
    fn make_output(has_card: bool, preview_count: usize, total: usize) -> ToolRenderOutput {
        let preview: Vec<String> = (1..=preview_count)
            .map(|i| format!("line-{i:02}"))
            .collect();
        ToolRenderOutput {
            title_line: Line::from("Write  src/main.rs"),
            title_raw: "Write  src/main.rs".into(),
            phase: ToolPhase::Success,
            permission_label: None,
            error_message: None,
            duration_us: Some(12_000),
            size_bytes: Some(128),
            tool_name: "write_file".into(),
            use_diff_gutter: true,
            arg_summary: "src/main.rs".into(),
            arg_full: "src/main.rs".into(),
            layout: ToolLayout {
                visual_rows: tool_visual_rows(has_card, preview_count, total, false),
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
            detail_full: None,
            card_bottom: " Double-click for full code ".into(),
        }
    }

    fn tool_cell(output: ToolRenderOutput) -> ToolCell {
        tool_cell_mode(output, false, None)
    }

    fn tool_cell_mode(
        output: ToolRenderOutput,
        card_only: bool,
        started_at: Option<std::time::Instant>,
    ) -> ToolCell {
        let msgs = crate::i18n::Messages::by_language(crate::i18n::Language::English);
        ToolCell::from_output(
            output,
            started_at,
            '⠋',
            card_only,
            Color::Cyan,
            Color::Black,
            Color::White,
            Color::Green,
            Color::Yellow,
            Color::Red,
            BorderType::Rounded,
            &msgs,
        )
    }

    // ── Normal mode tests ───────────────────────────────────────

    #[test]
    fn height_no_card_is_two() {
        let cell = tool_cell(make_output(false, 0, 0));
        assert_eq!(cell.height(80), TOOL_HEADER_ROWS as u16);
    }

    #[test]
    fn height_with_card_no_overflow() {
        // 10 preview lines, 10 total → no overflow row needed
        let cell = tool_cell(make_output(true, 10, 10));
        // 1 (summary) + 1 (top border) + 10 (preview) + 1 (bottom border) = 13
        assert_eq!(cell.height(80), 2 + 1 + 10 + 1);
    }

    #[test]
    fn height_with_card_and_overflow() {
        let cell = tool_cell(make_output(true, 10, 15));
        // Overflow is merged into the bottom hint, so no extra inner row.
        assert_eq!(cell.height(80), 2 + 1 + 10 + 1);
    }

    #[test]
    fn height_with_card_small_preview() {
        let cell = tool_cell(make_output(true, 3, 3));
        assert_eq!(cell.height(80), 2 + 1 + 3 + 1);
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
    fn render_partial_single_visible_header_row() {
        // Viewport shows only 1 row of a 2-row header — must not paint meta below area.
        let cell = tool_cell(make_output(false, 0, 0));
        let area = Rect::new(0, 0, 60, 1);
        let mut buf = Buffer::empty(area);
        cell.render_partial(area, &mut buf, 0);
        assert_eq!(buf.area, area);
    }

    #[test]
    fn render_partial_header_meta_when_title_scrolled_off() {
        let cell = tool_cell(make_output(false, 0, 0));
        let area = Rect::new(0, 0, 60, 1);
        let mut buf = Buffer::empty(area);
        cell.render_partial(area, &mut buf, 1);
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
        let cell = tool_cell_mode(make_output(false, 0, 0), true, None);
        assert_eq!(cell.height(80), 0);
    }

    #[test]
    fn card_only_height_with_card() {
        // 5 preview, 5 total → no overflow
        // height = 1 (top border) + 5 (preview) + 1 (bottom) = 7
        let cell = tool_cell_mode(make_output(true, 5, 5), true, None);
        assert_eq!(cell.height(80), 7);
    }

    #[test]
    fn card_only_height_with_overflow() {
        // 3 preview, 10 total → overflow row present
        // Overflow is merged into the bottom hint, so no extra inner row.
        // height = 1 + 3 + 1 = 5
        let cell = tool_cell_mode(make_output(true, 3, 10), true, None);
        assert_eq!(cell.height(80), 5);
    }

    #[test]
    fn card_only_renders_into_buffer() {
        let cell = tool_cell_mode(make_output(true, 3, 5), true, None);
        let area = Rect::new(0, 0, 60, 10);
        let mut buf = Buffer::empty(area);
        cell.render(area, &mut buf);
        assert_eq!(buf.area, area);
    }

    #[test]
    fn card_only_render_partial_skip_borders() {
        let cell = tool_cell_mode(make_output(true, 5, 5), true, None);
        let area = Rect::new(0, 0, 60, 10);
        let mut buf = Buffer::empty(area);
        // skip_lines=1: skip top border, render interior content
        cell.render_partial(area, &mut buf, 1);
        assert_eq!(buf.area, area);
    }

    #[test]
    fn overflow_is_merged_into_bottom_hint() {
        let cell = tool_cell(make_output(true, 3, 10));
        let bottom = cell.card_bottom_text();
        assert!(bottom.contains("3/10 lines"));
        assert!(bottom.contains("Double-click for full code"));
    }
}
