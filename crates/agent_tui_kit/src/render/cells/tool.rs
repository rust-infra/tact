//! Renderable cell for tool invocation results.
//!
//! `ToolCell` is the owned render half of the tool pipeline:
//! `ToolWidget::build()` → `ToolRenderOutput` → `ToolCell::from_output()` →
//! `LogColumnRenderer`. `ToolWidget` borrows theme/i18n for building;
//! `ToolCell` owns everything so it can live across frames.

use std::time::Instant;

use unicode_width::UnicodeWidthStr;

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph, Widget},
};
use tact_protocol::{TokenUsageInfo, ToolOutputLine, ToolOutputStream};

use crate::{
    i18n::Messages,
    render::renderable::Renderable,
    widgets::button::{Button, ButtonChrome, ButtonTheme, ButtonVariant},
    widgets::tool_widget::{
        TOOL_HEADER_ROWS, TOOL_META_ROW, ToolPhase, ToolRenderOutput, build_meta_text,
        collapsed_action_text, collapsed_output_hint, meta_error, meta_suffixes,
        running_elapsed_us, tool_card_inner_rows, tool_visual_rows,
    },
};

pub struct ToolCell {
    /// Title row text (tool name + arguments); styled here with the live theme.
    title_raw: String,
    phase: ToolPhase,
    /// Background task id, drawn right after the phase label.
    task_id: Option<String>,
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
    detail_preview: Vec<ToolOutputLine>,
    detail_total_lines: usize,
    card_bottom: String,
    is_subagent: bool,
    /// Template for the card-bottom line-count prefix (`" {}/{} lines | {} "`).
    card_progress_tmpl: &'static str,
    /// Meta-row hint for a collapsed command card (line count + how to open it).
    collapsed_output_hint: Option<String>,
    /// Label of the button inside [`Self::collapsed_output_hint`].
    collapsed_action_label: Option<&'static str>,
    /// The glyphs that button draws — the tail this row splits off.
    collapsed_action: Option<String>,
    tool_phase_running: &'static str,
    tool_phase_queued: &'static str,
    tool_phase_success: &'static str,
    tool_phase_failed: &'static str,
    tool_phase_cancelled: &'static str,
    tool_meta_sep: &'static str,
    step_success_prefix: &'static str,
    step_fail_prefix: &'static str,
    accent: Color,
    bg: Color,
    fg: Color,
    muted: Color,
    success: Color,
    warning: Color,
    error: Color,
    card_border_type: BorderType,
    /// Child session id of the spawned subagent, shown (short) on the meta row.
    subagent_id: Option<String>,
    subagent_model: Option<String>,
    subagent_tokens: Option<TokenUsageInfo>,
    subagent_context_window: Option<usize>,
    /// The cancel button's own label, taken from the frame's messages so a
    /// `/lang` switch repaints a live card in the new language.
    subagent_cancel_label: &'static str,
    /// Whether this card is a live async subagent, i.e. draws the meta-row
    /// `[Cancel]` button at all.
    subagent_cancel: bool,
    /// This block is the head of a run that is drawn expanded, so its title row
    /// carries the collapse affordance.
    ///
    /// Expanding a run replaces its one summary row (which carried `⏵`) with its
    /// members. Without a marker on the head that took the summary's row, the
    /// way back would be invisible: the reader would have to know that the
    /// title row is a control. `⏷` is the widening counterpart of the summary's
    /// `⏵`, on the row the summary occupied.
    run_expanded_head: bool,
}

impl ToolCell {
    #[allow(clippy::too_many_arguments)]
    pub fn from_output(
        output: ToolRenderOutput,
        started_at: Option<Instant>,
        spinner_char: char,
        card_only: bool,
        accent: Color,
        bg: Color,
        fg: Color,
        muted: Color,
        success: Color,
        warning: Color,
        error: Color,
        card_border_type: BorderType,
        msgs: &Messages,
    ) -> Self {
        // Collapsed output (command / read / edit) draws no card, so the meta
        // row is the only place left to say that output exists and how to open
        // it. A subagent card is the same shape whenever it has no body: just
        // spawned, folded shut by the reader, or finished with nothing to show.
        // Those are the states where a reader most wants to look inside, and the
        // meta row was the only thing left clickable, so it gets the same
        // affordance as a collapsed command.
        let subagent_without_body =
            matches!(output.visual_kind, tact_protocol::ToolVisualKind::Subagent)
                && !output.layout.has_detail_card;
        let wants_collapsed_affordance = output.layout.detail_collapsed || subagent_without_body;
        let collapsed_output_hint = wants_collapsed_affordance
            .then(|| collapsed_output_hint(msgs, output.detail_total_lines));
        // The button's own halves: the label it is built from, and the glyphs
        // it draws, which are the tail the meta row splits off to style.
        let collapsed_action_label =
            wants_collapsed_affordance.then_some(msgs.tool_collapsed_output_action);
        let collapsed_action = wants_collapsed_affordance.then(|| collapsed_action_text(msgs));
        // Card chrome is localized too, and the cell is built once per frame
        // with the *current* messages: deriving it here (instead of reading it
        // off the output) is what keeps the card in the language the rest of the
        // frame is drawn in after a `/lang` switch.
        let detail_title = output.card_title(msgs);
        let card_bottom = output.card_bottom(msgs);
        Self {
            title_raw: output.title_raw,
            phase: output.phase,
            task_id: output.task_id,
            permission_label: output.permission_label,
            error_message: output.error_message,
            duration_us: output.duration_us,
            size_bytes: output.size_bytes,
            started_at,
            spinner_char,
            card_only,
            has_detail_card: output.layout.has_detail_card,
            use_diff_gutter: output.use_diff_gutter,
            detail_title,
            detail_preview: output.detail_preview,
            detail_total_lines: output.detail_total_lines,
            card_bottom,
            is_subagent: matches!(output.visual_kind, tact_protocol::ToolVisualKind::Subagent),
            card_progress_tmpl: msgs.tool_card_progress_tmpl,
            collapsed_output_hint,
            collapsed_action_label,
            collapsed_action,
            tool_phase_running: msgs.tool_phase_running,
            tool_phase_queued: msgs.tool_phase_queued,
            tool_phase_success: msgs.tool_phase_success,
            tool_phase_failed: msgs.tool_phase_failed,
            tool_phase_cancelled: msgs.tool_phase_cancelled,
            tool_meta_sep: msgs.tool_meta_sep,
            step_success_prefix: msgs.step_success_prefix,
            step_fail_prefix: msgs.step_fail_prefix,
            accent,
            bg,
            fg,
            muted,
            success,
            warning,
            error,
            card_border_type,
            subagent_id: output.subagent_id,
            subagent_model: output.subagent_model,
            subagent_tokens: output.subagent_tokens,
            subagent_context_window: output.subagent_context_window,
            subagent_cancel_label: msgs.subagent_cancel_btn,
            subagent_cancel: false,
            run_expanded_head: false,
        }
    }

    /// Mark this card as the head of a run drawn expanded, so its title row
    /// draws the collapse affordance and the host knows the row is a control.
    #[must_use]
    pub fn with_run_expanded_head(mut self, expanded_head: bool) -> Self {
        self.run_expanded_head = expanded_head;
        self
    }

    /// Mark this card as the live async subagent it belongs to, so it draws the
    /// meta-row `[Cancel]` button. The host routes the click back to the child
    /// id through [`Self::subagent_cancel_button_rect`].
    #[must_use]
    pub fn with_subagent_cancel(mut self, cancel: bool) -> Self {
        self.subagent_cancel = cancel;
        self
    }

    /// Title row, styled with this frame's theme colors.
    fn title_line(&self) -> Line<'static> {
        Line::from(Span::styled(
            self.title_raw.clone(),
            Style::default().fg(self.fg).add_modifier(Modifier::BOLD),
        ))
    }

    /// The title row as it is drawn.
    ///
    /// An expanded run head adds nothing to this line: its collapse glyph is
    /// painted into the indent gutter to the left (see `render_partial`), so the
    /// title keeps its own columns and every member of the run stays aligned
    /// with the head. Prefixing the glyph would push the head one column right
    /// of the members beneath it, and the run would read as a ladder.
    fn drawn_title_line(&self) -> Line<'static> {
        self.title_line()
    }

    /// Where the collapse glyph goes: the two columns just left of the title,
    /// inside the block's indent gutter.
    ///
    /// `row_area` is already the title's own row (the caller indented it by
    /// `LOG_TOOL_BLOCK_INDENT`), so this only steps left. Painting into the
    /// gutter rather than in front of the title is what keeps the run's members
    /// aligned: a prefix would push the head one column right of the members
    /// beneath it.
    ///
    /// `None` when this block is not an expanded run head, or when the gutter is
    /// too narrow to hold the glyph without leaving the panel.
    fn run_toggle_glyph_rect(&self, row_area: Rect) -> Option<Rect> {
        if !self.run_expanded_head || row_area.x < 2 {
            return None;
        }
        Some(Rect::new(row_area.x - 2, row_area.y, 2, 1))
    }

    fn meta_line(&self) -> Line<'static> {
        let duration_us = if self.phase == ToolPhase::Running {
            self.started_at.map(running_elapsed_us).or(self.duration_us)
        } else {
            self.duration_us
        };
        let mut text = build_meta_text(
            self.phase,
            self.task_id.as_deref(),
            self.permission_label.as_deref(),
            self.size_bytes,
            duration_us,
            meta_error(
                self.phase,
                self.has_detail_card,
                self.error_message.as_deref(),
            ),
            self.spinner_char,
            self.tool_phase_running,
            self.tool_phase_queued,
            self.tool_phase_success,
            self.tool_phase_failed,
            self.tool_phase_cancelled,
            self.tool_meta_sep,
            self.step_success_prefix,
            self.step_fail_prefix,
        );
        // Subagent metadata and the collapsed-output hint share one formatter
        // with the widget, which needs the finished row's exact text.
        text.push_str(&meta_suffixes(
            self.tool_meta_sep,
            self.subagent_id.as_deref(),
            self.subagent_model.as_deref(),
            self.subagent_tokens.as_ref(),
            self.subagent_context_window,
            self.collapsed_output_hint.as_deref(),
        ));
        let style = match self.phase {
            ToolPhase::Queued => Style::default().fg(self.muted),
            ToolPhase::Running => Style::default().fg(self.warning),
            ToolPhase::Success => Style::default().fg(self.success),
            ToolPhase::Failed => Style::default().fg(self.error),
            ToolPhase::Cancelled => Style::default().fg(self.muted),
        };
        // The collapsed-output button *is* this row's affordance when the card
        // draws no body, so it is drawn by the shared button widget rather than
        // as more phase-colored prose. The row text carries the same glyphs
        // (both come from `collapsed_action_text`), which is what keeps the
        // drawn button and `hits_collapsed_action` on the same columns.
        let button = self
            .collapsed_action
            .as_deref()
            .zip(self.collapsed_action_label)
            .and_then(|(action, label)| text.strip_suffix(action).map(|prefix| (prefix, label)));
        let Some((prefix, label)) = button else {
            return Line::from(Span::styled(text, style));
        };
        let mut spans = vec![Span::styled(prefix.to_string(), style)];
        spans.extend(
            Button::new(label, self.button_theme())
                .chrome(ButtonChrome::Brackets)
                .line()
                .spans,
        );
        Line::from(spans)
    }

    fn button_theme(&self) -> ButtonTheme {
        ButtonTheme {
            fg: self.fg,
            bg: self.bg,
            accent: self.accent,
            muted: self.muted,
            warning: self.warning,
            error: self.error,
            success: self.success,
        }
    }

    /// The live subagent card's cancel affordance, drawn by the shared button
    /// widget — same chrome and variant the host used to overlay onto the row.
    fn cancel_button(&self) -> Button<'_> {
        Button::new(self.subagent_cancel_label, self.button_theme())
            .variant(ButtonVariant::Danger)
            .chrome(ButtonChrome::Brackets)
            .modifier(Modifier::BOLD)
    }

    /// The cancel button's rect on a full-width meta row, when this card
    /// carries one: right-aligned with one blank column of margin at the row's
    /// end (the column the log scrollbar can overwrite) and one between the
    /// text and the button.
    fn cancel_button_area(&self, row_area: Rect) -> Option<Rect> {
        if !self.subagent_cancel {
            return None;
        }
        let width = self.cancel_button().preferred_size().0;
        (row_area.width > width + 1).then(|| {
            Rect::new(
                row_area.right() - width - 1,
                row_area.y,
                width,
                row_area.height.min(1),
            )
        })
    }

    /// Screen rect of a live subagent's meta-row `[Cancel]` button, measured
    /// from the same geometry [`Renderable::render_partial`] draws with, so the
    /// host's hit area is exactly the glyphs on screen. `None` when the card is
    /// not cancellable or the meta row is clipped by the viewport.
    pub fn subagent_cancel_button_rect(&self, area: Rect, skip_lines: usize) -> Option<Rect> {
        if self.card_only || !self.subagent_cancel || skip_lines > TOOL_META_ROW {
            return None;
        }
        let area =
            crate::render::util::indent_rect(area, crate::render::util::LOG_TOOL_BLOCK_INDENT);
        let vis_off = TOOL_META_ROW - skip_lines;
        if vis_off >= area.height as usize {
            return None;
        }
        let row_area = Rect::new(area.x, area.y + vis_off as u16, area.width, 1);
        self.cancel_button_area(row_area)
    }

    /// Draw the meta row (row 1 of the header) into `row_area`.
    ///
    /// The cell owns this row whole — text, blanks, and the cancel button's
    /// columns — so the row's background is painted across it first and the
    /// text is clipped to the columns left of the button. (`Paragraph` writes
    /// over every column it is given; handing it the full row would let a long
    /// meta line overwrite the button's glyphs, the exact failure the old
    /// render-loop overlay had.)
    fn render_meta_row(&self, row_area: Rect, buf: &mut Buffer) {
        buf.set_style(row_area, Style::default().bg(self.bg));
        let text_area = self.meta_text_area(row_area);
        if text_area.width > 0 {
            Paragraph::new(vec![self.meta_line()]).render(text_area, buf);
        }
        if let Some(button_area) = self.cancel_button_area(row_area) {
            self.cancel_button().render(button_area, buf);
        }
    }

    /// The columns of the meta row the text itself gets.
    ///
    /// Whole row when this card has no `[Cancel]`; otherwise everything left of
    /// the button, with one blank column between text and button. One helper for
    /// both the paint and the two hit rects on this row, because three
    /// measurements of the same row had already drifted: a rect for a glyph that
    /// the clip removed is worse than no rect at all — the click runs whatever
    /// owns those columns instead.
    fn meta_text_area(&self, row_area: Rect) -> Rect {
        let Some(button_area) = self.cancel_button_area(row_area) else {
            return row_area;
        };
        Rect::new(
            row_area.x,
            row_area.y,
            button_area.x.saturating_sub(row_area.x).saturating_sub(1),
            1,
        )
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

        let visible_line_count = self.detail_total_lines.min(self.detail_preview.len());
        let first_line_number = self
            .detail_total_lines
            .saturating_sub(visible_line_count)
            .saturating_add(1);
        let lines: Vec<Line<'static>> = self
            .detail_preview
            .iter()
            .enumerate()
            .map(|(i, line)| {
                let num = if i < visible_line_count {
                    format!("{:>nw$}", first_line_number + i, nw = num_width)
                } else {
                    " ".repeat(num_width)
                };
                let mut spans = Vec::with_capacity(line.spans.len() + 2);
                if self.use_diff_gutter {
                    spans.push(Span::styled(format!(" {} ", num), num_style));
                    spans.push(Span::styled("+ ", plus_style));
                } else {
                    spans.push(Span::styled(format!(" {} ", num), num_style));
                }
                let mut remaining = code_width;
                for output_span in &line.spans {
                    if remaining == 0 {
                        break;
                    }
                    let text: String = output_span.text.chars().take(remaining).collect();
                    remaining = remaining.saturating_sub(text.chars().count());
                    let style = if output_span.stream == ToolOutputStream::Stderr {
                        Style::default().fg(self.warning).bg(self.bg)
                    } else {
                        text_style
                    };
                    spans.push(Span::styled(text, style));
                }
                Line::from(spans)
            })
            .collect();

        lines
    }

    /// Screen rect of an expanded run head's collapse control: the glyph in the
    /// gutter plus the whole title row beside it.
    ///
    /// The glyph alone would be a two-cell target, and the row beside it is not
    /// text a reader selects — activating either puts the run back. That row is
    /// the one the folded summary occupied, so the control stays where the
    /// summary was rather than moving as the run expands.
    ///
    /// `None` when this card is not an expanded run head, or when a clip has
    /// scrolled the title row off the frame.
    pub fn run_toggle_rect(&self, area: Rect, skip_lines: usize) -> Option<Rect> {
        if !self.run_expanded_head || skip_lines > 0 {
            return None;
        }
        let row =
            crate::render::util::indent_rect(area, crate::render::util::LOG_TOOL_BLOCK_INDENT);
        let width = row.width.saturating_add(2);
        (width > 0 && row.height > 0).then(|| Rect::new(row.x.saturating_sub(2), row.y, width, 1))
    }

    /// Screen rect of a collapsed tool's `[Open]` meta-row button.
    ///
    /// Measured off the row the cell actually draws: the text is clipped to
    /// [`Self::meta_text_area`] (a live subagent's `[Cancel]` takes the row's
    /// right end), and the action is the tail *of that clipped text*. The two
    /// used to be measured independently, so a clipped hint kept a full-width
    /// rect over columns that were no longer the button — a live card's
    /// `[Cancel]` sat under it, and the click opened the transcript instead of
    /// cancelling.
    pub fn collapsed_open_button_rect(&self, area: Rect, skip_lines: usize) -> Option<Rect> {
        let action = self.collapsed_action.as_deref()?;
        if skip_lines >= self.height(area.width) as usize || skip_lines > TOOL_META_ROW {
            return None;
        }
        let row =
            crate::render::util::indent_rect(area, crate::render::util::LOG_TOOL_BLOCK_INDENT);
        let row = Rect::new(
            row.x,
            row.y + (TOOL_META_ROW - skip_lines) as u16,
            row.width,
            1,
        );
        let text_area = self.meta_text_area(row);
        let action_width = UnicodeWidthStr::width(action) as u16;
        // `Paragraph` truncates at the area it is given instead of wrapping, so
        // a row too long for its columns loses its *tail* — which is the action
        // itself. There is then no button on screen, and claiming one would put
        // a hit rect over whatever the row does show.
        let text_width = UnicodeWidthStr::width(self.meta_line().to_string().as_str()) as u16;
        (action_width > 0 && text_width <= text_area.width).then(|| {
            Rect::new(
                text_area.x + text_width - action_width,
                text_area.y,
                action_width,
                1,
            )
        })
    }

    /// Whether this card fronts a `spawn_subagent` call.
    ///
    /// The renderer needs it beside [`Self::subagent_open_button_rect`], because
    /// a *collapsed* subagent card draws its `[Open]` on the meta row instead of
    /// the footer, and that button has to reach the transcript rather than the
    /// tool's diff popup.
    pub fn is_subagent(&self) -> bool {
        self.is_subagent
    }

    /// The card footer is the subagent popup's click target in every phase:
    /// `Click for buffered output` while the child streams, `[Open]` once it
    /// finished, `Click for full error` when it failed. Returns `None` when the
    /// footer is clipped or the card is not a subagent.
    ///
    /// The rect is the footer *as drawn*, which costs two details that used to
    /// be missed:
    ///
    /// * the card is painted inside the block indent
    ///   ([`LOG_TOOL_BLOCK_INDENT`]), so the rect has to be indented too —
    ///   measuring it off the un-indented slice put it eight columns left of the
    ///   glyphs, which left the right-hand part of the label dead;
    /// * the drawn footer is [`Self::card_bottom_text`], progress prefix
    ///   included (`5/20 lines | [Open]`), not the bare `card_bottom` — with
    ///   the prefix the label starts later and runs longer, so measuring
    ///   `card_bottom` put most of it outside the hit rect. That is exactly the
    ///   live card, the one a reader clicks: it always has a prefix.
    pub fn subagent_open_button_rect(&self, area: Rect, skip_lines: usize) -> Option<Rect> {
        if self.card_only || !self.is_subagent || !self.has_detail_card {
            return None;
        }
        let card_total = 1 + self.card_inner_rows() + 1;
        let card_skip = skip_lines.saturating_sub(TOOL_HEADER_ROWS);
        // Case A of [`Renderable::render_partial`] draws the bottom border with
        // its title; a partially visible card (case B) draws a bare border, so
        // there is no label to click there.
        if card_skip != 0 || card_skip + (area.height as usize) < TOOL_HEADER_ROWS + card_total {
            return None;
        }
        let bottom_y = area.y + (TOOL_HEADER_ROWS + card_total - 1 - skip_lines) as u16;
        // Same indent as the paint, then the block's own left border: that is
        // where `Block::title_bottom` starts the footer text.
        let card =
            crate::render::util::indent_rect(area, crate::render::util::LOG_TOOL_BLOCK_INDENT);
        let width = UnicodeWidthStr::width(self.card_bottom_text().as_str())
            .min(card.width.saturating_sub(2) as usize) as u16;
        (width > 0).then(|| Rect::new(card.x + 1, bottom_y, width, 1))
    }
    fn card_bottom_text(&self) -> String {
        if self.detail_total_lines > self.detail_preview.len() {
            // The count prefix is chrome, so it is localized like the label it
            // introduces (the template comes from the frame's messages).
            self.card_progress_tmpl
                .replacen("{}", &self.detail_preview.len().to_string(), 1)
                .replacen("{}", &self.detail_total_lines.to_string(), 1)
                .replacen("{}", self.card_bottom.trim(), 1)
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
                    // Title text is a fact of the call; its color is not — style
                    // it here so a theme switch repaints blocks that already
                    // exist. The paragraph keeps the row's own fg so the
                    // untouched remainder of the row is painted exactly as the
                    // rest of the theme paints every other row.
                    Paragraph::new(vec![self.drawn_title_line()])
                        .style(Style::default().fg(self.fg).bg(self.bg))
                        .render(row_area, buf);
                    // The folded summary's `⏵` lives in this gutter column, so
                    // the expanded run's `⏷` does too: the two states differ by
                    // the glyph, not by where the run starts.
                    if row == 0
                        && let Some(glyph) = self.run_toggle_glyph_rect(row_area)
                    {
                        buf.set_string(
                            glyph.x,
                            glyph.y,
                            "⏷",
                            Style::default().fg(self.accent).bg(self.bg),
                        );
                    }
                } else {
                    self.render_meta_row(row_area, buf);
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
    use tact_protocol::ToolOutputSpan;

    use super::*;
    use crate::widgets::tool_widget::{
        TOOL_HEADER_ROWS, TOOL_META_ROW, ToolLayout, ToolPhase, tool_visual_rows,
    };

    /// Build a `ToolRenderOutput` for a hypothetical "Step 1: write_file".
    ///
    /// `has_card` controls whether the card is present; `preview_count` sets
    /// how many preview lines are stored; `total` is the real file line count.
    fn make_output(has_card: bool, preview_count: usize, total: usize) -> ToolRenderOutput {
        let preview: Vec<ToolOutputLine> = (1..=preview_count)
            .map(|i| ToolOutputLine {
                spans: vec![ToolOutputSpan {
                    stream: ToolOutputStream::Other,
                    text: format!("line-{i:02}"),
                }],
            })
            .collect();
        ToolRenderOutput {
            title_raw: "Write  src/main.rs".into(),
            phase: ToolPhase::Success,
            task_id: None,
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
                detail_collapsed: false,
            },
            live_detail: false,
            detail_preview: preview,
            detail_total_lines: total,
            detail_full: None,
            subagent_id: None,
            subagent_model: None,
            subagent_tokens: None,
            subagent_context_window: None,
            visual_kind: tact_protocol::ToolVisualKind::FileWrite,
        }
    }

    fn tool_cell(output: ToolRenderOutput) -> ToolCell {
        tool_cell_mode(output, false, None)
    }

    /// Plain text of the meta row (row 1 of the cell).
    fn meta_text(cell: &ToolCell) -> String {
        cell.meta_line()
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
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
            Color::Gray,
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
        assert!(bottom.contains("Click for full code"));
    }

    /// The overflow prefix is chrome, not a readout of the tool: it is drawn in
    /// the frame's language like the label it introduces.
    #[test]
    fn overflow_prefix_is_localized() {
        let msgs = crate::i18n::Messages::by_language(crate::i18n::Language::Chinese);
        let theme = crate::theme::Theme::from(crate::theme::ThemeName::Retro);
        let output = make_output(true, 3, 10);
        let cell = ToolCell::from_output(
            output,
            None,
            ' ',
            false,
            theme.accent,
            theme.bg,
            theme.fg,
            theme.muted,
            theme.success,
            theme.warning,
            theme.error,
            theme.block_border_type(),
            &msgs,
        );
        let bottom = cell.card_bottom_text();
        assert!(bottom.contains("3/10 行"), "{bottom:?}");
        assert!(bottom.contains("点击查看完整代码"), "{bottom:?}");
        assert!(!bottom.contains("lines"), "{bottom:?}");
    }

    /// The log hit test measures `ToolRenderOutput.meta_text`, while this cell
    /// draws the meta row itself: the two must be the same string, or clicks
    /// would land next to the text they are aimed at.
    #[test]
    fn widget_meta_text_matches_the_rendered_meta_row() {
        let msgs = crate::i18n::Messages::by_language(crate::i18n::Language::English);
        let result = tact_protocol::StepResult {
            tool: "bash".into(),
            arg_summary: "cargo build".into(),
            arg_full: Some("cargo build".into()),
            status: tact_protocol::StepStatus::Success,
            message: "ok".into(),
            detail: Some("Compiling\ndone\n".into()),
            duration_us: Some(4_820_000),
            permission_label: Some("Always allow this tool".into()),
            presentation: tact_protocol::ToolPresentationInfo::generic("bash"),
        };
        let output = crate::widgets::tool_widget::ToolWidget::from_step_result(&result).build();
        let measured = output
            .meta_text(&msgs)
            .expect("a finished block has a meta row");
        let cell = tool_cell(output);

        assert_eq!(meta_text(&cell), measured);
        assert!(measured.contains("4 lines · [󰜼 Open]"), "{measured}");
    }

    /// A collapsed command card draws no card, so the meta row must carry the
    /// line count and the gesture that opens it.
    #[test]
    fn collapsed_command_meta_row_reports_hidden_output() {
        let mut output = make_output(false, 0, 0);
        output.layout.detail_collapsed = true;
        output.detail_total_lines = 7;
        output.tool_name = "bash".into();
        output.visual_kind = tact_protocol::ToolVisualKind::Command;
        let text = meta_text(&tool_cell(output));

        assert!(text.contains("7 lines"), "meta row: {text}");
        assert!(text.contains("7 lines · [󰜼 Open]"), "meta row: {text}");
    }

    /// The tail is not more prose: it is the shared button widget's own spans
    /// (brackets plus label), styled by the button, while the sentence before it
    /// keeps the phase color.
    #[test]
    fn collapsed_command_meta_row_draws_its_action_as_a_button() {
        let mut output = make_output(false, 0, 0);
        output.layout.detail_collapsed = true;
        output.detail_total_lines = 7;
        output.tool_name = "bash".into();
        output.visual_kind = tact_protocol::ToolVisualKind::Command;
        let line = tool_cell(output).meta_line();

        assert_eq!(line.spans.len(), 4, "prose + the button's three spans");
        let tail: String = line.spans[1..]
            .iter()
            .map(|span| span.content.as_ref())
            .collect();
        assert_eq!(tail, "[󰜼 Open]");
        assert_eq!(
            line.spans[0].style.fg,
            Some(Color::Green),
            "prose keeps the phase color"
        );
        assert_eq!(
            line.spans[1].style.fg,
            Some(Color::Gray),
            "the tail is drawn by the button"
        );
        assert_eq!(
            line.spans[1].style.bg,
            Some(Color::Black),
            "the button paints its own bg"
        );
    }

    #[test]
    fn collapsed_command_meta_row_uses_the_singular_for_one_line() {
        let mut output = make_output(false, 0, 0);
        output.layout.detail_collapsed = true;
        output.detail_total_lines = 1;
        output.tool_name = "bash".into();
        output.visual_kind = tact_protocol::ToolVisualKind::Command;
        let text = meta_text(&tool_cell(output));

        assert!(text.contains("1 line ·"), "meta row: {text}");
        assert!(text.contains("1 line · [󰜼 Open]"), "meta row: {text}");
        assert!(!text.contains("1 lines"), "meta row: {text}");
    }

    #[test]
    fn open_card_meta_row_has_no_collapsed_hint() {
        let text = meta_text(&tool_cell(make_output(true, 3, 3)));
        assert!(!text.contains("[󰜼 Open]"), "meta row: {text}");
    }

    /// A subagent card names the run it spawned: the short id (the same handle
    /// the sticky Subagent overview lists), ahead of the model.
    #[test]
    fn subagent_meta_row_shows_the_short_child_id() {
        let mut output = make_output(false, 0, 0);
        output.subagent_id = Some("4d2a5f1c-9e8b-4c1a-bdf0-112233445566".into());
        output.subagent_model = Some("gpt-4o".into());
        let text = meta_text(&tool_cell(output));

        assert!(text.contains("#4d2a5f1c"), "meta row: {text}");
        assert!(
            !text.contains("4d2a5f1c-"),
            "the handle is the short form, not the uuid: {text}"
        );
        assert!(text.contains("🤖 gpt-4o"), "the model follows it: {text}");
    }

    /// Card and sticky must name a run the same way, and the widget-measured
    /// row (what the log hit test sees) is the row the cell draws.
    #[test]
    fn subagent_meta_row_matches_the_widgets_measured_row() {
        let msgs = crate::i18n::Messages::by_language(crate::i18n::Language::English);
        let widget = crate::widgets::tool_widget::ToolWidget::new()
            .with_tool("spawn_subagent")
            .with_arg_summary("audit the repo")
            .with_phase(ToolPhase::Success)
            .with_detail("child summary")
            .with_subagent_id(Some("4d2a5f1c-9e8b-4c1a-bdf0-112233445566".into()));
        let measured = widget
            .build()
            .meta_text(&msgs)
            .expect("a finished block has a meta row");
        let drawn = meta_text(&tool_cell(widget.build()));

        assert_eq!(drawn, measured);
        assert!(measured.contains("#4d2a5f1c"), "{measured}");
    }

    /// The live async subagent card draws its `[Cancel]` button on the meta
    /// row — the row it belongs to. A meta line long enough to reach the
    /// row's end is clipped before the button instead of overwriting it (the
    /// failure the old render-loop overlay had on the title row).
    #[test]
    fn live_subagent_cancel_button_is_drawn_on_the_meta_row() {
        let mut output = make_output(false, 0, 0);
        output.phase = ToolPhase::Running;
        output.subagent_id = Some("child-123".into());
        output.subagent_model = Some("gpt-4o".into());
        let cell = tool_cell_mode(output, false, None).with_subagent_cancel(true);
        let area = Rect::new(0, 0, 60, 2);
        let mut buf = Buffer::empty(area);
        cell.render(area, &mut buf);

        let cells: Vec<String> = (0..area.width)
            .map(|x| buf[(x, TOOL_META_ROW as u16)].symbol().to_string())
            .collect();
        let needle: Vec<String> = "[Cancel]".chars().map(|c| c.to_string()).collect();
        let col = cells
            .windows(needle.len())
            .position(|w| w == needle.as_slice())
            .expect("the live card must draw [Cancel] on its meta row");
        let rect = cell
            .subagent_cancel_button_rect(area, 0)
            .expect("the drawn button has a hit rect");
        assert_eq!(
            (rect.x as usize, rect.y),
            (col, TOOL_META_ROW as u16),
            "the hit rect must be exactly the drawn glyphs"
        );

        // The row stays readable left of the button: its phase and its handle,
        // with one blank column between the text and the button and one at the
        // row's own end (the column a scrollbar can repaint).
        assert_eq!(buf[(col as u16 - 1, TOOL_META_ROW as u16)].symbol(), " ");
        assert_eq!(buf[(rect.right(), TOOL_META_ROW as u16)].symbol(), " ");
        let row_text: String = cells.join("");
        assert!(row_text.contains("#child-12"), "meta row: {row_text:?}");
        assert!(row_text.contains("Running"), "meta row: {row_text:?}");

        // Scrolled so the meta row leaves the slice: no glyphs, no hit rect.
        assert!(
            cell.subagent_cancel_button_rect(Rect::new(0, 0, 60, 1), TOOL_META_ROW + 1)
                .is_none()
        );
    }

    /// A meta line that would run under the button is cut at its left edge —
    /// the button patch is never overwritten by the text band next to it.
    #[test]
    fn long_meta_row_is_clipped_before_the_cancel_button() {
        let mut output = make_output(false, 0, 0);
        output.phase = ToolPhase::Running;
        output.subagent_id = Some("child-123".into());
        output.permission_label = Some("Always allow this tool for the whole workspace".into());
        let cell = tool_cell_mode(output, false, None).with_subagent_cancel(true);
        let area = Rect::new(0, 0, 60, 2);
        let mut buf = Buffer::empty(area);
        cell.render(area, &mut buf);

        let row_text: String = (0..area.width)
            .map(|x| buf[(x, TOOL_META_ROW as u16)].symbol().to_string())
            .collect();
        assert!(row_text.contains("Running"), "meta row: {row_text:?}");
        assert!(
            row_text.contains("[Cancel]"),
            "the button must survive the clip: {row_text:?}"
        );
        assert!(
            !row_text.contains("workspace"),
            "the overlong label must be cut, not flow under the button: {row_text:?}"
        );
    }

    /// The button is the row's affordance for a *live* card only: a card that
    /// is not marked cancellable draws none.
    #[test]
    fn no_cancel_button_without_a_cancellable_card() {
        let mut output = make_output(false, 0, 0);
        output.phase = ToolPhase::Running;
        let cell = tool_cell_mode(output, false, None);
        let area = Rect::new(0, 0, 60, 2);
        let mut buf = Buffer::empty(area);
        cell.render(area, &mut buf);

        let row: String = (0..area.width)
            .map(|x| buf[(x, TOOL_META_ROW as u16)].symbol().to_string())
            .collect();
        assert!(!row.contains("[Cancel]"), "meta row: {row:?}");
        assert!(cell.subagent_cancel_button_rect(area, 0).is_none());
    }

    /// The meta row can carry two controls at once: the `[Cancel]` a live
    /// subagent draws at its right end, and the `[Open]` hint a card with no
    /// body draws at the end of its *text*. The hint has to be measured against
    /// the columns the text actually gets, or it lands on top of the button and
    /// eats its clicks.
    #[test]
    fn the_open_hint_never_lands_on_the_cancel_button() {
        let mut output = make_output(false, 0, 0);
        output.phase = ToolPhase::Running;
        output.visual_kind = tact_protocol::ToolVisualKind::Subagent;
        output.permission_label = Some("Always allow this tool for the whole workspace".into());

        for width in [50u16, 60, 70, 100] {
            let cell = tool_cell_mode(output.clone(), false, None).with_subagent_cancel(true);
            let area = Rect::new(0, 0, width, 2);
            let mut buf = Buffer::empty(area);
            cell.render(area, &mut buf);

            let cancel = cell
                .subagent_cancel_button_rect(area, 0)
                .unwrap_or_else(|| panic!("width {width}: [Cancel] is drawn"));
            // The drawn glyphs are the button's own columns.
            let row: String = (0..width)
                .map(|x| buf[(x, TOOL_META_ROW as u16)].symbol().to_string())
                .collect();
            let drawn: String = row
                .chars()
                .skip(cancel.x as usize)
                .take(cancel.width as usize)
                .collect();
            assert_eq!(
                drawn, "[Cancel]",
                "width {width}: the rect must be the glyphs, row: {row:?}"
            );

            match cell.collapsed_open_button_rect(area, 0) {
                Some(hint) => {
                    assert!(
                        !hint.intersects(cancel),
                        "width {width}: the hint at {hint:?} covers [Cancel] at {cancel:?}"
                    );
                    // A rect that survived the clip must sit on its own glyphs:
                    // the row's text ends right where the hint begins.
                    let before: String = row
                        .chars()
                        .skip(hint.x as usize)
                        .take(hint.width as usize)
                        .collect();
                    assert!(
                        before.contains("Open"),
                        "width {width}: the hint must be over the hint, got {before:?} \
                         in {row:?}"
                    );
                }
                // Clipped away: `Paragraph` truncates the row, so the tail (the
                // hint) is simply not on screen. No rect is the honest answer.
                None => assert!(
                    !row.contains("Open"),
                    "width {width}: no rect, yet the hint is drawn, row: {row:?}"
                ),
            }
        }
    }

    /// The footer is the subagent popup's click target for a live card too —
    /// its label is literally "Click for buffered output", so the rect must not
    /// be limited to a finished card.
    #[test]
    fn live_subagent_card_footer_is_the_open_target() {
        let mut output = make_output(true, 2, 2);
        output.phase = ToolPhase::Running;
        output.live_detail = true;
        output.visual_kind = tact_protocol::ToolVisualKind::Subagent;
        let cell = tool_cell(output);
        let area = Rect::new(0, 0, 60, (TOOL_HEADER_ROWS + 4) as u16);
        let rect = cell
            .subagent_open_button_rect(area, 0)
            .expect("the live footer is clickable");
        assert_eq!(rect.y, (TOOL_HEADER_ROWS + 3) as u16);

        // A non-subagent card has no such target, live or finished.
        let mut plain = make_output(true, 2, 2);
        plain.phase = ToolPhase::Running;
        plain.live_detail = true;
        assert!(
            tool_cell(plain)
                .subagent_open_button_rect(area, 0)
                .is_none()
        );
    }

    /// The footer's hit rect has to be the *drawn* footer: inside the block
    /// indent, starting at the card's left border, and as wide as the label the
    /// border actually carries.
    ///
    /// Both halves were wrong in a way nothing caught, because the bug only
    /// showed up through the real renderer and a real click: the rect was
    /// measured off the un-indented slice (eight columns left of the glyphs) and
    /// off `card_bottom` instead of `card_bottom_text()` (missing the
    /// `5/20 lines | ` prefix, which is present on exactly the live card a
    /// reader clicks). Every column of the label past the rect's end was dead,
    /// and the longer the label, the deader the footer got.
    #[test]
    fn subagent_footer_hit_rect_matches_the_footer_it_draws() {
        use crate::render::util::LOG_TOOL_BLOCK_INDENT;

        /// The four footers a subagent card can carry: live with and without
        /// the progress prefix, finished, and failed.
        fn shapes() -> Vec<(&'static str, ToolRenderOutput)> {
            let mut live_short = make_output(true, 2, 2);
            live_short.phase = ToolPhase::Running;
            live_short.live_detail = true;
            let mut live_long = make_output(true, 5, 20);
            live_long.phase = ToolPhase::Running;
            live_long.live_detail = true;
            let finished = make_output(true, 2, 2);
            let mut failed = make_output(true, 2, 2);
            failed.phase = ToolPhase::Failed;
            failed.error_message = Some("child blew up".into());
            vec![
                ("live, no prefix", live_short),
                ("live, `5/20 lines | `", live_long),
                ("finished, `[Open]`", finished),
                ("failed", failed),
            ]
        }

        for (shape, mut output) in shapes() {
            output.visual_kind = tact_protocol::ToolVisualKind::Subagent;
            let cell = tool_cell(output);
            // The host hands the cell exactly its own height
            // (`LogColumnRenderer::cell_slice`); a taller area would let the
            // card's borders fill the slack and the geometry would be a fixture
            // artifact rather than the one on screen.
            let area = Rect::new(0, 0, 60, cell.height(60));
            let mut buf = Buffer::empty(area);
            cell.render(area, &mut buf);

            let rect = cell
                .subagent_open_button_rect(area, 0)
                .unwrap_or_else(|| panic!("{shape}: the footer is clickable"));
            // One entry per screen column: a border glyph is multi-byte, so a
            // byte slice would land mid-character.
            let cells: Vec<String> = (0..area.width)
                .map(|x| buf[(x, rect.y)].symbol().to_string())
                .collect();
            let row: String = cells.join("");
            assert_eq!(
                rect.x,
                LOG_TOOL_BLOCK_INDENT + 1,
                "{shape}: the rect must start at the card's left border, row: {row:?}"
            );
            let drawn: String = cells[rect.x as usize..(rect.x + rect.width) as usize].concat();
            assert_eq!(
                drawn,
                cell.card_bottom_text(),
                "{shape}: the rect must cover the drawn footer exactly"
            );
            // The rect ends with the label: the next cell is the border line
            // that continues to the card's right corner.
            assert_eq!(
                cells[rect.right() as usize],
                "─",
                "{shape}: the rect must end with the label, row: {row:?}"
            );
        }
    }

    #[test]
    fn stderr_preview_uses_warning_color() {
        let mut output = make_output(true, 2, 2);
        output.detail_preview = vec![
            ToolOutputLine {
                spans: vec![ToolOutputSpan {
                    stream: ToolOutputStream::Stdout,
                    text: "normal".into(),
                }],
            },
            ToolOutputLine {
                spans: vec![ToolOutputSpan {
                    stream: ToolOutputStream::Stderr,
                    text: "warning".into(),
                }],
            },
        ];
        let cell = tool_cell(output);
        let area = Rect::new(0, 0, 60, 10);
        let mut buf = Buffer::empty(area);

        cell.render(area, &mut buf);

        let warning_cell = (0..area.width)
            .flat_map(|x| (0..area.height).map(move |y| (x, y)))
            .map(|pos| &buf[pos])
            .find(|cell| cell.symbol() == "w")
            .expect("warning text should render");
        assert_eq!(warning_cell.fg, Color::Yellow);
    }
}
