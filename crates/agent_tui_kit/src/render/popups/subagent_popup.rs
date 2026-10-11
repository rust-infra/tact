//! Subagent popup: the child's conversation, rendered as one readable column.
//!
//! The body is a sequence of **segments** — a meta row, the prompt, then one
//! segment per transcript entry (assistant Markdown, a thinking block, a tool
//! block, info / error text). Segments are laid out once per append and drawn
//! through the log's [`LogColumnRenderer`], so a child conversation scrolls and
//! clips like the main log does.
//!
//! Three consequences worth stating, because they are the point of the rewrite:
//!
//! * **Live and completed render identically.** There is no plain-text branch:
//!   assistant text is Markdown-rendered while it streams, so finishing a run
//!   no longer re-flows the body under the reader's cursor.
//! * **The body is driven by structure, not by text.** A tool call's name,
//!   arguments, status and result come from the child's own updates, so a
//!   `read_file` draws a read card and a `bash` draws a command card — a
//!   flattened `→ name args` line could not say that.
//! * **Nothing here opens another popup.** A tool call is drawn as a
//!   self-contained *block* (title, meta, output, overflow row) rather than
//!   the log's interactive card: that card's `[ Open ]` and `Click for full
//!   code` footers promise a second overlay, and a popup on top of a popup is
//!   the one thing this surface cannot offer. Everything a reader needs is in
//!   this column, reachable by scrolling it — and a capped block's overflow row
//!   is itself the affordance: it states the count and, clicked, raises *that
//!   block's* cap in place ([`SubagentPopup::expanded_steps`]). Revealing more
//!   of the same column is not a second overlay; it is the column, longer.
//!
//! `prepare_subagent_popup` builds the layout (side effect on the popup state);
//! `render_subagent_body` only reads it and reports the toggle rows it drew.

use std::collections::HashSet;
use std::ops::Range;

use ratatui::{
    Frame,
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Widget},
};

use super::PopupMouseSurface;
use crate::{
    i18n::Messages,
    protocol::{
        StepResult, StepStatus, SubagentStep, SubagentStepStatus, SubagentTranscriptEntry,
        ToolVisualKind,
    },
    render::{
        bar::format_tokens_compact,
        ctx::RenderCtx,
        log_column::LogColumnRenderer,
        render_md::render_markdown_tui,
        renderable::Renderable,
        selectable_text::{
            DisplayRow, display_row_from_line, layout_display_rows, scalar_styles, source_lines,
        },
        util::wrap_line,
    },
    state::{SubagentPopup, ToolState},
    theme::{Theme, ThemeName},
    widgets::tool_widget::{
        TOOL_RUNNING_SPINNER, ToolPhase, ToolRenderOutput, ToolWidget, format_bytes,
        format_duration_us,
    },
};

/// Output rows a tool block shows inside the popup.
///
/// The popup is a transcript, not a stream: a 500-line `grep` dump would bury
/// the conversation, and there is no second popup to open (that is exactly what
/// this rendering exists to avoid). So the remainder is **counted** — and the
/// counting row is the toggle that reveals it (see the module header).
const POPUP_TOOL_PREVIEW_LINES: usize = 8;

/// Columns the body is indented by, and where a block's rail sits.
const BLOCK_INDENT: usize = 2;
/// Columns a block's title occupies before its text (`"  ▎ "`).
const BLOCK_TITLE_COLS: usize = BLOCK_INDENT + 2;
/// Columns an output row's rail occupies before its text (`"    │ "`).
const BLOCK_OUTPUT_COLS: usize = BLOCK_INDENT + 4;

/// One row of a transcript block.
///
/// Selectable, like a text row: the graphemes carry byte ranges into the popup's
/// copy text, and the block's rail is *cells without bytes* (see
/// [`display_row_from_line`]) — so a drag across a tool call selects the command
/// and its output, and nothing else.
#[derive(Debug, Clone)]
pub struct BlockRow {
    pub row: DisplayRow,
    /// Row surface. A tool's output sits on the code band, so it reads as data
    /// rather than as more prose.
    pub bg: Color,
    /// Where this row's text starts (after the rail).
    pub content_col: u16,
    /// The content column of the **running** glyph this row draws, when it is a
    /// running block's phase row.
    ///
    /// The layout bakes a `●` in — the copy text stays semantic and the byte
    /// ranges stay exact — and the draw pass swaps *that one cell* for the
    /// current animation frame. The body is cached: rebuilding it per frame to
    /// move one glyph would re-wrap the whole child conversation, every frame.
    pub spinner: Option<u16>,
}

/// One drawn unit of the popup body, with its visual start row and height.
#[derive(Debug, Clone)]
pub enum PopupSegment {
    /// The run's meta row (model / tokens / elapsed / status). Not selectable.
    Meta {
        vis_start: usize,
        height: usize,
        rows: Vec<DisplayRow>,
    },
    /// Selectable text rows. `bytes` is this segment's range inside
    /// [`SubagentPopupLayout::selection_text`], which is what maps the popup's
    /// one selection onto the segment's own byte space.
    Text {
        vis_start: usize,
        height: usize,
        rows: Vec<DisplayRow>,
        bytes: Range<usize>,
    },
    /// A thinking run or a tool call: pre-wrapped rows, no selectable text.
    Block {
        vis_start: usize,
        height: usize,
        rows: Vec<BlockRow>,
        /// `(row index inside this segment, child step id)` for the rows that
        /// toggle a block's expansion — the host turns a click on one into
        /// `App::toggle_subagent_step_expanded`.
        toggles: Vec<(usize, String)>,
    },
}

impl PopupSegment {
    pub fn vis_start(&self) -> usize {
        match self {
            Self::Meta { vis_start, .. }
            | Self::Text { vis_start, .. }
            | Self::Block { vis_start, .. } => *vis_start,
        }
    }

    pub fn height(&self) -> usize {
        match self {
            Self::Meta { height, .. } | Self::Text { height, .. } | Self::Block { height, .. } => {
                *height
            }
        }
    }

    fn shift(&mut self, rows: usize) {
        match self {
            Self::Meta { vis_start, .. }
            | Self::Text { vis_start, .. }
            | Self::Block { vis_start, .. } => *vis_start += rows,
        }
    }
}

/// The built popup body, reused while nothing that affects layout changed.
#[derive(Debug, Clone)]
pub struct SubagentPopupLayout {
    /// The transcript's append counter this layout was built from.
    pub revision: u64,
    pub width: u16,
    pub theme: ThemeName,
    /// Live-ness is part of the key because it is part of the meta row.
    pub is_live: bool,
    /// The expansion counter the blocks were built with (see
    /// [`SubagentPopup::expanded_rev`]).
    pub expanded_rev: u64,
    /// Total visual rows across every segment (the scroll clamp).
    pub total_rows: usize,
    /// What drag-selection and `y` copy address; segments hold their own slice.
    pub selection_text: String,
    pub segments: Vec<PopupSegment>,
}

impl SubagentPopupLayout {
    pub fn is_valid(
        &self,
        revision: u64,
        width: u16,
        theme: ThemeName,
        is_live: bool,
        expanded_rev: u64,
    ) -> bool {
        self.revision == revision
            && self.width == width
            && self.theme == theme
            && self.is_live == is_live
            && self.expanded_rev == expanded_rev
    }
}

/// Where a `spawn_subagent` card's identity and prompt live, for the popup
/// header. Read off whichever side of the active → block transition it is on.
struct PopupSource {
    phase: ToolPhase,
    /// Live cards tick; finished ones carry the measured duration.
    started_at: Option<std::time::Instant>,
    elapsed_us: Option<u64>,
    model: Option<String>,
    tokens: Option<u32>,
    prompt: String,
}

fn popup_source(tools: &ToolState, tool_id: &str) -> Option<PopupSource> {
    if let Some(active) = tools.active.iter().find(|a| a.tool_id == tool_id) {
        return Some(PopupSource {
            phase: ToolPhase::Running,
            started_at: Some(active.started_at),
            elapsed_us: None,
            model: active.output.subagent_model.clone(),
            tokens: active.output.subagent_tokens.as_ref().map(|t| t.total),
            prompt: active.output.arg_full.clone(),
        });
    }
    tools
        .blocks
        .iter()
        .find(|b| b.tool_id == tool_id)
        .map(|block| PopupSource {
            phase: block.output.phase,
            started_at: None,
            elapsed_us: block.output.duration_us,
            model: block.output.subagent_model.clone(),
            tokens: block.output.subagent_tokens.as_ref().map(|t| t.total),
            prompt: block.output.arg_full.clone(),
        })
}

/// Rebuild the popup's layout when missing or stale. Side-effect phase.
pub fn prepare_subagent_popup(
    popup: &mut SubagentPopup,
    tools: &ToolState,
    theme: &Theme,
    msgs: &Messages,
    body_width: u16,
) {
    let Some(source) = popup_source(tools, &popup.tool_id) else {
        return;
    };
    let is_live = matches!(source.phase, ToolPhase::Running);
    let Some(transcript) = tools.transcripts.get(&popup.tool_id) else {
        return;
    };
    if transcript.entries.is_empty() {
        return;
    }
    let expanded_rev = popup.expanded_rev;
    if popup.layout.as_ref().is_some_and(|l| {
        l.is_valid(
            transcript.revision,
            body_width,
            theme.name,
            is_live,
            expanded_rev,
        )
    }) {
        return;
    }

    let width = body_width.max(1) as usize;
    let mut builder = BodyBuilder::new(theme, msgs, width, &popup.expanded_steps);

    if !source.prompt.trim().is_empty() {
        builder.push_prompt(source.prompt.trim());
    }
    for entry in &transcript.entries {
        builder.push_entry(entry);
    }

    // The meta row comes first, but its trailing "N lines" is the body's own
    // row count — so the body is built first and shifted down by the header.
    let body_rows = builder.rows;
    let meta = builder.meta_rows(&source);
    // The status span is unconditional, so the header always has a row; the
    // segment's height has to be the *cell's* height or `LogColumnRenderer`
    // would clip it against a height the cell does not have.
    debug_assert!(!meta.is_empty(), "the meta row is never empty");
    let meta_height = meta.len().max(1);
    let mut segments = builder.segments;
    for segment in &mut segments {
        segment.shift(meta_height);
    }
    let mut all = Vec::with_capacity(segments.len() + 1);
    all.push(PopupSegment::Meta {
        vis_start: 0,
        height: meta_height,
        rows: meta,
    });
    all.extend(segments);

    let total_rows = meta_height + body_rows;
    popup.layout = Some(SubagentPopupLayout {
        revision: transcript.revision,
        width: body_width,
        theme: theme.name,
        is_live,
        expanded_rev,
        total_rows,
        selection_text: builder.selection_text,
        segments: all,
    });
}

/// What a block row's left rail looks like.
#[derive(Debug, Clone, Copy)]
enum BlockKind {
    /// The block's first row: a heavy rail in the block's own colour.
    Title(Color),
    /// A row under the title with no rail (the phase row, the overflow count).
    Plain,
    /// An output row: a light rail in `color`.
    Output(Color),
}

impl BlockKind {
    /// The row's leading spans and how many columns they occupy.
    fn prefix(self) -> (Vec<Span<'static>>, usize) {
        match self {
            Self::Title(color) => (
                vec![
                    Span::raw(" ".repeat(BLOCK_INDENT)),
                    Span::styled("▎", Style::default().fg(color)),
                    Span::raw(" "),
                ],
                BLOCK_TITLE_COLS,
            ),
            Self::Plain => (
                vec![Span::raw(" ".repeat(BLOCK_TITLE_COLS))],
                BLOCK_TITLE_COLS,
            ),
            Self::Output(color) => (
                vec![
                    Span::raw(" ".repeat(BLOCK_TITLE_COLS)),
                    Span::styled("│", Style::default().fg(color)),
                    Span::raw(" "),
                ],
                BLOCK_OUTPUT_COLS,
            ),
        }
    }
}

/// Accumulates the body's segments, rows and selectable text.
struct BodyBuilder<'a> {
    theme: &'a Theme,
    msgs: &'a Messages,
    width: usize,
    rows: usize,
    /// Tool calls seen so far — the block's own step number.
    ///
    /// Not the entry index: thinking and assistant runs are entries too, and
    /// numbering by those would make the child's first tool call `4.`.
    steps: usize,
    segments: Vec<PopupSegment>,
    /// Rows of the block being built; flushed into a segment when it ends.
    block_rows: Vec<BlockRow>,
    /// `(row index, step id)` of the toggle rows in the block being built.
    block_toggles: Vec<(usize, String)>,
    /// Whether the block being built has started its copy text — the next copy
    /// row then needs a line separator, not the between-blocks blank line.
    block_open: bool,
    selection_text: String,
    /// Child step ids whose blocks show the raised cap (see
    /// [`SubagentPopup::expanded_steps`]).
    expanded: &'a HashSet<String>,
}

impl<'a> BodyBuilder<'a> {
    fn new(
        theme: &'a Theme,
        msgs: &'a Messages,
        width: usize,
        expanded: &'a HashSet<String>,
    ) -> Self {
        Self {
            theme,
            msgs,
            width,
            rows: 0,
            steps: 0,
            segments: Vec::new(),
            block_rows: Vec::new(),
            block_toggles: Vec::new(),
            block_open: false,
            selection_text: String::new(),
            expanded,
        }
    }

    fn fallback(&self) -> Style {
        Style::default().fg(self.theme.fg).bg(self.theme.bg)
    }

    /// The prompt is the child's own user message: the brief the parent handed
    /// it. Rendered as **Markdown**, not raw text.
    ///
    /// The main log draws a human's typed message plainly, but a subagent
    /// prompt is a generated brief — headings, numbered checklists, bold
    /// leads — and the popup is where it is read. Showing `##` and `**`
    /// verbatim made the one part of the popup a reader always sees look
    /// broken.
    ///
    /// The `💬` marker is the one the main log gives user rows. It goes on the
    /// first *rendered* line and into the selection text by the same bytes, so
    /// a selection offset computed from the display rows still indexes the
    /// string the copy path slices.
    fn push_prompt(&mut self, prompt: &str) {
        let (mut styled, raw_lines) = render_markdown_tui(prompt, self.theme);
        let mut text = raw_lines.join("\n");
        if text.is_empty() {
            return;
        }
        const MARKER: &str = "💬 ";
        text.insert_str(0, MARKER);
        let marker = Span::styled(MARKER, Style::default().fg(self.theme.muted));
        match styled.first_mut() {
            Some(first) => first.spans.insert(0, marker),
            None => styled.push(Line::from(marker)),
        }
        self.push_text(&text, &styled);
    }

    fn push_entry(&mut self, entry: &SubagentTranscriptEntry) {
        match entry {
            SubagentTranscriptEntry::Assistant(text) => {
                let (styled, raw_lines) = render_markdown_tui(text, self.theme);
                self.push_text(&raw_lines.join("\n"), &styled);
            }
            SubagentTranscriptEntry::Thinking { text, duration_us } => {
                self.push_thinking(text, *duration_us);
            }
            SubagentTranscriptEntry::Step(step) => {
                self.push_step(step);
            }
            SubagentTranscriptEntry::Info(text) => {
                let fg = self.theme.muted;
                self.push_text(text, &plain_lines(text, fg));
            }
            SubagentTranscriptEntry::Error(text) => {
                let fg = self.theme.error;
                self.push_text(text, &plain_lines(text, fg));
            }
        }
    }

    /// A thinking run: a labelled block, shown in full.
    ///
    /// Not capped like a tool's output: the child's reasoning is only ever
    /// visible here (the log's thinking card belongs to the *parent*), so
    /// hiding it behind a count would hide it entirely. A tool's output, by
    /// contrast, is still reachable from its card in the log.
    fn push_thinking(&mut self, text: &str, duration_us: Option<u64>) {
        let accent = self.theme.thinking_card_border();
        let title = Line::from(vec![
            Span::styled(
                self.msgs.thinking_card_title_done.trim().to_string(),
                Style::default().fg(accent).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(" · ⏱ {}", format_duration_us(duration_us.unwrap_or(0))),
                Style::default().fg(self.theme.muted),
            ),
        ]);
        // The label is chrome: the copy buffer carries the reasoning, not the
        // "🧠 Thought · ⏱ 1.2s" heading a reader did not write.
        self.push_block_row(title, BlockKind::Title(accent), self.theme.bg, false);

        let fg = self.theme.thinking_preview_fg();
        for line in text.lines() {
            self.push_block_row(
                Line::from(Span::styled(line.to_string(), Style::default().fg(fg))),
                BlockKind::Output(accent),
                self.theme.bg,
                true,
            );
        }
        self.flush_block();
    }

    /// A tool call: title, phase row, a capped output preview, and the row that
    /// toggles the cap. No border and no `[ Open ]` — see the module header.
    fn push_step(&mut self, step: &SubagentStep) {
        let step_index = self.steps;
        self.steps += 1;
        let output = build_step_output(step, step_index);
        let expanded = self.expanded.contains(&step.tool_id);
        let (preview, total) = step_output(&output, expanded);

        let title = Line::from(Span::styled(
            output.title_raw.clone(),
            Style::default()
                .fg(self.theme.fg)
                .add_modifier(Modifier::BOLD),
        ));
        self.push_block_row(
            title,
            BlockKind::Title(self.theme.accent),
            self.theme.bg,
            true,
        );

        let (meta, meta_fg) = self.step_meta(&output, total);
        let meta_row = self.push_block_row(
            Line::from(Span::styled(meta.clone(), Style::default().fg(meta_fg))),
            BlockKind::Plain,
            self.theme.bg,
            true,
        );
        // A running child is still working: the phase row's glyph animates (the
        // layout keeps the `●`, the draw pass swaps the cell).
        if matches!(output.phase, ToolPhase::Running) {
            self.mark_spinner(meta_row);
        }

        let bg = self.theme.code_block_bg();
        let fg = self.theme.code_block_fg();
        for line in &preview {
            self.push_block_row(
                Line::from(Span::styled(line.clone(), Style::default().fg(fg))),
                BlockKind::Output(self.theme.border),
                bg,
                true,
            );
        }

        if let Some(row) = self.push_toggle_row(total, preview.len(), expanded) {
            self.block_toggles.push((row, step.tool_id.clone()));
        }

        self.flush_block();
    }

    /// The row that states what the cap hid and toggles it: `⋯ 12 more lines ·
    /// ▾ expand` while capped, `▴ collapse` (with the count, when even the
    /// raised cap hid something) once expanded.
    ///
    /// Returns the row it drew, so the host can route a click on it. `None`
    /// when there is nothing to reveal and nothing to put back.
    fn push_toggle_row(&mut self, total: usize, shown: usize, expanded: bool) -> Option<usize> {
        let msgs = self.msgs;
        let hidden = total.saturating_sub(shown);
        let more = || {
            msgs.subagent_popup_more_lines
                .replacen("{}", &hidden.to_string(), 1)
        };
        let text = if expanded {
            // Offering to collapse is only honest if collapsing would hide
            // something — a short output has nothing to put back.
            (total > POPUP_TOOL_PREVIEW_LINES).then(|| {
                if hidden > 0 {
                    format!(
                        "{}{}{}",
                        msgs.subagent_popup_collapse_action,
                        msgs.tool_meta_sep,
                        more()
                    )
                } else {
                    msgs.subagent_popup_collapse_action.to_string()
                }
            })
        } else {
            (hidden > 0).then(|| {
                format!(
                    "{}{}{}",
                    more(),
                    msgs.tool_meta_sep,
                    msgs.subagent_popup_expand_action
                )
            })
        };
        let text = text?;
        Some(self.push_block_row(
            Line::from(Span::styled(text, Style::default().fg(self.theme.muted))),
            BlockKind::Plain,
            self.theme.bg,
            false,
        ))
    }

    /// The phase row of a tool block, and the colour it is drawn in.
    fn step_meta(&self, output: &ToolRenderOutput, total: usize) -> (String, Color) {
        let msgs = self.msgs;
        let (glyph, label, color) = match output.phase {
            ToolPhase::Running => ("●", msgs.tool_phase_running, self.theme.warning),
            ToolPhase::Success => (
                msgs.step_success_prefix,
                msgs.tool_phase_success,
                self.theme.success,
            ),
            ToolPhase::Failed => (
                msgs.step_fail_prefix,
                msgs.tool_phase_failed,
                self.theme.error,
            ),
        };
        let mut parts = vec![format!("{glyph} {label}")];
        if let Some(size) = output.size_bytes {
            parts.push(format_bytes(size));
        }
        if let Some(us) = output.duration_us {
            parts.push(format_duration_us(us));
        }
        if total > 0 {
            parts.push(
                msgs.subagent_popup_lines
                    .replacen("{}", &total.to_string(), 1),
            );
        }
        (parts.join(msgs.tool_meta_sep), color)
    }

    /// Wrap one row's content and put the block's rail on every visual row it
    /// produces.
    ///
    /// Wrapping happens here rather than at draw time, which is what lets the
    /// rail be a span on every row: a rail typed into text that a later
    /// `wrap_line` slices would mark only the first row of a wrapped line.
    fn push_block_row(
        &mut self,
        line: Line<'static>,
        kind: BlockKind,
        bg: Color,
        copy: bool,
    ) -> usize {
        let (prefix, cols) = kind.prefix();
        let available = self.width.saturating_sub(cols).max(1);
        let text: String = line
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect();

        // The copy buffer is appended here, row by row, rather than assembled at
        // the end of the block: that makes every row's byte range absolute and
        // exact by construction instead of re-derived from the glyphs drawn.
        // One buffer line per *logical* row — the wrap below splits a row
        // visually, never in the buffer.
        let mut cursor = self.selection_text.len();
        if copy {
            if self.block_open {
                self.selection_text.push('\n');
            } else {
                if !self.selection_text.is_empty() {
                    self.selection_text.push_str("\n\n");
                }
                self.block_open = true;
            }
            cursor = self.selection_text.len();
            self.selection_text.push_str(&text);
        }

        let first = self.block_rows.len();
        for mut wrapped in wrap_line(&line, available) {
            let row_len: usize = wrapped.spans.iter().map(|span| span.content.len()).sum();
            let range = if copy {
                let range = cursor..cursor + row_len;
                cursor = range.end;
                range
            } else {
                // Chrome (a thinking title, the overflow row): columns, no bytes.
                cursor..cursor
            };
            let mut spans = prefix.clone();
            spans.append(&mut wrapped.spans);
            self.block_rows.push(BlockRow {
                row: display_row_from_line(&Line::from(spans), cols, range),
                bg,
                content_col: cols as u16,
                spinner: None,
            });
        }
        first
    }

    /// Mark `row` as the running spinner's home: the draw pass replaces its first
    /// content cell with the current animation frame.
    fn mark_spinner(&mut self, row: usize) {
        if let Some(block_row) = self.block_rows.get_mut(row) {
            block_row.spinner = Some(block_row.content_col);
        }
    }

    /// Flush the accumulated block rows into a segment.
    fn flush_block(&mut self) {
        self.block_open = false;
        if self.block_rows.is_empty() {
            return;
        }
        let rows = std::mem::take(&mut self.block_rows);
        let toggles = std::mem::take(&mut self.block_toggles);
        let height = rows.len();
        self.segments.push(PopupSegment::Block {
            vis_start: self.rows,
            height,
            rows,
            toggles,
        });
        self.rows += height;
    }

    /// Append a run of text to the popup's copy buffer and return where it
    /// starts.
    ///
    /// Text segments come through here; a block's rows append themselves (see
    /// `push_block_row`). Either way a block's text belongs in the buffer even
    /// where a row is chrome: `y` should copy the commands the child ran and
    /// what they printed, not just the prose around them.
    fn push_block_selection_text(&mut self, text: &str) -> usize {
        // A text segment ends any block that was open, so the next block gets
        // the blank-line separator rather than a single newline.
        self.block_open = false;
        if !self.selection_text.is_empty() {
            self.selection_text.push_str("\n\n");
        }
        let start = self.selection_text.len();
        self.selection_text.push_str(text);
        start
    }

    /// Push a selectable run of text, wrapped for `width` and styled from
    /// `styled` (one entry per source line; missing lines fall back to the
    /// theme's foreground).
    fn push_text(&mut self, text: &str, styled: &[Line<'static>]) {
        if text.is_empty() {
            return;
        }
        let start = self.push_block_selection_text(text);
        let bytes = start..self.selection_text.len();

        let rows = layout_rows(text, start, styled, self.fallback(), self.width);
        if rows.is_empty() {
            return;
        }
        let height = rows.len();
        self.segments.push(PopupSegment::Text {
            vis_start: self.rows,
            height,
            rows,
            bytes,
        });
        self.rows += height;
    }

    /// The header row: what this run is, in one line.
    ///
    /// No line count here — the popup's title already carries one, and two
    /// numbers that differ by the header row itself read as a bug.
    fn meta_rows(&self, source: &PopupSource) -> Vec<DisplayRow> {
        let muted = Style::default().fg(self.theme.muted);
        let mut spans: Vec<Span<'static>> = Vec::new();
        let part = |spans: &mut Vec<Span<'static>>, text: String| {
            if !spans.is_empty() {
                spans.push(Span::styled(" · ", muted));
            }
            spans.push(Span::styled(text, muted));
        };
        if let Some(model) = &source.model {
            part(&mut spans, format!("🤖 {model}"));
        }
        if let Some(total) = source.tokens {
            part(
                &mut spans,
                format!("⚡ {} tok", format_tokens_compact(total as u64)),
            );
        }
        let elapsed = match source.started_at {
            Some(started) => Some(started.elapsed().as_micros() as u64),
            None => source.elapsed_us,
        };
        if let Some(us) = elapsed {
            part(&mut spans, format!("⏱ {}", format_duration_us(us)));
        }
        let (status, style) = match source.phase {
            ToolPhase::Running => ("● running", Style::default().fg(self.theme.warning)),
            ToolPhase::Success => ("✓ Success", Style::default().fg(self.theme.success)),
            ToolPhase::Failed => ("✗ Failed", Style::default().fg(self.theme.error)),
        };
        if !spans.is_empty() {
            spans.push(Span::styled(" · ", muted));
        }
        spans.push(Span::styled(status, style));

        let text: String = spans.iter().map(|span| span.content.as_ref()).collect();
        layout_rows(&text, 0, &[Line::from(spans)], self.fallback(), self.width)
    }
}

/// Wrap one text run into display rows with **global** byte offsets.
///
/// `base` is the run's start inside the popup's selection text, so a row's
/// `hit_row` reports an offset the popup's single selection range can be
/// compared against without translating on every frame.
fn layout_rows(
    text: &str,
    base: usize,
    styled: &[Line<'static>],
    fallback: Style,
    width: usize,
) -> Vec<DisplayRow> {
    let mut rows = Vec::new();
    for (index, line) in source_lines(text).iter().enumerate() {
        let styles = scalar_styles(styled.get(index), fallback, line.text.chars().count());
        rows.extend(layout_display_rows(
            line.text,
            base + line.start,
            &styles,
            width,
            true,
        ));
    }
    rows
}

fn plain_lines(text: &str, fg: Color) -> Vec<Line<'static>> {
    text.lines()
        .map(|line| Line::from(Span::styled(line.to_string(), Style::default().fg(fg))))
        .collect()
}

/// Build the card spec for one child tool call.
fn build_step_output(step: &SubagentStep, index: usize) -> ToolRenderOutput {
    match step.status {
        SubagentStepStatus::Running => ToolWidget::new()
            .with_tool(step.tool_name.clone())
            .with_arg_summary(step.arg_summary.clone())
            .with_arg_full(step.arg_full.clone())
            .with_step_index(index)
            .with_phase(ToolPhase::Running)
            .build(),
        SubagentStepStatus::Success | SubagentStepStatus::Failed => {
            let result = StepResult {
                tool: step.tool_name.clone(),
                arg_summary: step.arg_summary.clone(),
                arg_full: Some(step.arg_full.clone()),
                status: if matches!(step.status, SubagentStepStatus::Failed) {
                    StepStatus::Failed
                } else {
                    StepStatus::Success
                },
                message: step.message.clone(),
                detail: step.detail.clone(),
                duration_us: step.duration_us,
                permission_label: None,
                presentation: step.presentation.clone(),
            };
            ToolWidget::from_step_result(&result)
                .with_step_index(index)
                .build()
        }
    }
}

/// Maximum lines shown when a step block is expanded (safety cap).
///
/// Not "all of it": a `grep -r` dump can be tens of thousands of lines, and the
/// layout is rebuilt as one unit, so the toggle raises the preview to a bound a
/// reader can scroll rather than removing the bound.
const POPUP_TOOL_EXPANDED_LINES: usize = 50;

/// The output a step block shows, and how many lines the call produced in
/// total.
///
/// Read straight off `detail_full` rather than reusing the card's
/// `detail_preview`: the card's slice is tuned for a two-row block in a stream
/// (one line, or five for an error), while a block here has room for a few
/// more. What is *not* shown is counted, not offered — see the module header.
///
/// `expanded` is the host's toggle (the view binds a key to it): it raises the
/// cap, it does not change which end of the output is kept.
fn step_output(output: &ToolRenderOutput, expanded: bool) -> (Vec<String>, usize) {
    let detail = output.detail_full.clone().unwrap_or_default();
    let mut lines: Vec<&str> = detail.lines().collect();
    // A command's detail opens with `$ <command>`, which the block's own title
    // already carries.
    if matches!(output.visual_kind, ToolVisualKind::Command)
        && lines.first().is_some_and(|line| line.starts_with("$ "))
    {
        lines.remove(0);
        if lines.first().is_some_and(|line| line.is_empty()) {
            lines.remove(0);
        }
    }
    let total = lines.len();
    let cap = if expanded {
        POPUP_TOOL_EXPANDED_LINES
    } else {
        POPUP_TOOL_PREVIEW_LINES
    };
    let preview: Vec<String> = if matches!(output.visual_kind, ToolVisualKind::Command) {
        // A command's interesting end is its tail, matching `ToolWidget::build`.
        lines
            .iter()
            .skip(total.saturating_sub(cap))
            .map(|line| (*line).to_string())
            .collect()
    } else {
        lines
            .iter()
            .take(cap)
            .map(|line| (*line).to_string())
            .collect()
    };
    (preview, total)
}

/// A block's pre-wrapped rows, drawn into the log column renderer.
///
/// Selectable, unlike a log tool card: the view is a reading surface, and the
/// command a child ran plus what it printed is exactly what a reader wants to
/// copy. `selection` is the popup's own (global) range — the rows' grapheme hits
/// live in the same byte space, so no per-row mapping is needed.
struct BlockCell<'a> {
    rows: &'a [BlockRow],
    selection: Option<Range<usize>>,
    /// Monotonic animation tick. The running glyph is picked here, per frame,
    /// because the layout that holds the row is cached — the same reason the log
    /// passes its spinner into the cell it builds each frame.
    spinner_frame: u32,
}

impl Renderable for BlockCell<'_> {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        self.render_partial(area, buf, 0);
    }

    fn height(&self, _width: u16) -> u16 {
        self.rows.len() as u16
    }

    fn render_partial(&self, area: Rect, buf: &mut Buffer, skip_lines: usize) {
        for (index, row) in self.rows.iter().enumerate().skip(skip_lines) {
            let y = area.y + (index - skip_lines) as u16;
            if y >= area.y.saturating_add(area.height) {
                break;
            }
            let rect = Rect::new(area.x, y, area.width, 1);
            // Own the row surface: an output row sits on the code band, and a
            // `Paragraph` only writes the glyphs it has, so the tail would keep
            // whatever the previous frame left there.
            buf.set_style(rect, Style::default().bg(row.bg));
            Paragraph::new(Line::from(row.row.spans(self.selection.as_ref()))).render(rect, buf);

            // One cell, swapped in place: the baked `●` is the semantic marker
            // (and what the copy buffer carries), the frame is the animation.
            if let Some(column) = row.spinner
                && let Some(cell) = buf.cell_mut((rect.x + column, y))
            {
                let glyph = TOOL_RUNNING_SPINNER
                    [(self.spinner_frame as usize) % TOOL_RUNNING_SPINNER.len()];
                let mut encoded = [0u8; 4];
                cell.set_symbol(glyph.encode_utf8(&mut encoded));
            }
        }
    }
}

/// A run of pre-wrapped rows, drawn into the log column renderer.
struct TextRowsCell<'a> {
    rows: &'a [DisplayRow],
    /// Selection in this cell's own byte space (already mapped from the
    /// popup's global range by the caller).
    selection: Option<Range<usize>>,
    bg: Color,
}

impl Renderable for TextRowsCell<'_> {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        self.render_partial(area, buf, 0);
    }

    fn height(&self, _width: u16) -> u16 {
        self.rows.len() as u16
    }

    fn render_partial(&self, area: Rect, buf: &mut Buffer, skip_lines: usize) {
        for (index, row) in self.rows.iter().enumerate().skip(skip_lines) {
            let y = area.y + (index - skip_lines) as u16;
            if y >= area.y.saturating_add(area.height) {
                break;
            }
            let rect = Rect::new(area.x, y, area.width, 1);
            // Every unit paints its own background over its full row: a
            // `Paragraph` writes only the glyphs it has, so the tail would
            // otherwise keep whatever the previous frame left there.
            //
            // The one cell this cannot own is the second half of a wide
            // grapheme (emoji / CJK): ratatui marks it skipped in the diff so
            // the terminal draws both halves from the glyph itself. Nothing to
            // do about it here, and nothing should be done — see the popup's
            // blank-cell test, which skips exactly those cells.
            buf.set_style(rect, Style::default().bg(self.bg));
            Paragraph::new(Line::from(row.spans(self.selection.as_ref()))).render(rect, buf);
        }
    }
}

/// Intersect the popup's global selection with one segment's byte range.
///
/// The result stays in the popup's own byte space: a row's grapheme hits are
/// global (`layout_rows` bakes the segment's base in) and the hit rows feed
/// `PopupTextSelection` directly, so subtracting the segment's start here — which
/// this used to do — made the highlight vanish for every segment but the first,
/// where the base happens to be 0.
fn segment_selection(global: &Range<usize>, bytes: &Range<usize>) -> Option<Range<usize>> {
    let start = global.start.max(bytes.start);
    let end = global.end.min(bytes.end);
    (start < end).then_some(start..end)
}

/// Render only the scrollable body of a subagent popup (no chrome, no
/// scrollbar). Used by view-mode hosts that draw their own header and chrome.
pub fn render_subagent_body(
    frame: &mut Frame,
    body_area: Rect,
    ctx: &RenderCtx,
) -> PopupMouseSurface {
    let mut surface = PopupMouseSurface::default();
    let Some(popup) = ctx.subagent_popup else {
        return surface;
    };
    let is_live = ctx.tools.active.iter().any(|a| a.tool_id == popup.tool_id);

    let Some(transcript) = ctx.tools.transcripts.get(&popup.tool_id) else {
        return surface;
    };
    let Some(layout) = popup.layout.as_ref() else {
        return surface;
    };
    if !layout.is_valid(
        transcript.revision,
        body_area.width,
        ctx.theme.name,
        is_live,
        popup.expanded_rev,
    ) {
        return surface;
    }

    let total = layout.total_rows;
    let content_height = body_area.height as usize;
    let max_scroll = total.saturating_sub(content_height);
    let scroll = (popup.scroll as usize).min(max_scroll);

    let selection = popup
        .selection
        .and_then(|sel| sel.normalized_non_empty(&layout.selection_text));

    let mut renderer = LogColumnRenderer::new().with_viewport(scroll, content_height);
    let mut hit_rows = Vec::new();

    for segment in &layout.segments {
        match segment {
            PopupSegment::Meta {
                vis_start, rows, ..
            } => {
                renderer.push(
                    *vis_start,
                    TextRowsCell {
                        rows,
                        selection: None,
                        bg: ctx.theme.bg,
                    },
                );
            }
            PopupSegment::Text {
                vis_start,
                rows,
                bytes,
                ..
            } => {
                let selected = selection
                    .as_ref()
                    .and_then(|global| segment_selection(global, bytes));
                renderer.push(
                    *vis_start,
                    TextRowsCell {
                        rows,
                        selection: selected,
                        bg: ctx.theme.bg,
                    },
                );
                for (index, row) in rows.iter().enumerate() {
                    let vis = vis_start + index;
                    if vis < scroll || vis >= scroll + content_height {
                        continue;
                    }
                    hit_rows.push(row.hit_row(body_area.y + (vis - scroll) as u16, body_area.x));
                }
            }
            PopupSegment::Block {
                vis_start,
                rows,
                toggles,
                ..
            } => {
                renderer.push(
                    *vis_start,
                    BlockCell {
                        rows,
                        selection: selection.clone(),
                        spinner_frame: ctx.spinner_frame,
                    },
                );
                // Block rows are selectable too, so they contribute hit rows
                // exactly like a text segment — same viewport clip, or a drag
                // could resolve to a row that is not on screen.
                for (index, row) in rows.iter().enumerate() {
                    let vis = vis_start + index;
                    if vis < scroll || vis >= scroll + content_height {
                        continue;
                    }
                    hit_rows.push(
                        row.row
                            .hit_row(body_area.y + (vis - scroll) as u16, body_area.x),
                    );
                }
                // The toggle rows are clickable, so their screen rects have to
                // come back with the surface — clipped by the same viewport the
                // rows themselves were clipped by, or a click would hit a row
                // that is not on screen.
                for (index, step_key) in toggles {
                    let vis = vis_start + index;
                    if vis < scroll || vis >= scroll + content_height {
                        continue;
                    }
                    surface.block_toggles.push((
                        step_key.clone(),
                        Rect::new(
                            body_area.x,
                            body_area.y + (vis - scroll) as u16,
                            body_area.width,
                            1,
                        ),
                    ));
                }
            }
        }
    }

    frame.render_widget(renderer, body_area);
    surface.body_area = body_area;
    surface.hit_rows = hit_rows;
    surface
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        i18n::Language,
        protocol::ToolPresentationInfo,
        state::{ActiveToolBlock, SubagentTranscript, ToolBlock},
    };

    fn theme() -> Theme {
        Theme::from(ThemeName::Ink)
    }

    fn step(tool_id: &str, status: SubagentStepStatus) -> SubagentStep {
        SubagentStep {
            tool_id: tool_id.into(),
            tool_name: "read_file".into(),
            arg_summary: "src/main.rs".into(),
            arg_full: "src/main.rs".into(),
            status,
            message: "fn main() {}".into(),
            detail: Some("fn main() {}".into()),
            duration_us: Some(1_400),
            presentation: ToolPresentationInfo::generic("read_file"),
        }
    }

    fn subagent_card(live: bool) -> ToolRenderOutput {
        let mut output = ToolWidget::new()
            .with_tool(String::from("spawn_subagent"))
            .with_arg_summary(String::from("research"))
            .with_arg_full(String::from("Research bitcoin."))
            .with_phase(if live {
                ToolPhase::Running
            } else {
                ToolPhase::Success
            })
            .with_duration_us(41_000_000)
            .build();
        output.visual_kind = ToolVisualKind::Subagent;
        output
    }

    fn tools_with(entries: Vec<SubagentTranscriptEntry>, live: bool) -> ToolState {
        let mut tools = ToolState::default();
        let mut transcript = SubagentTranscript::default();
        for entry in entries {
            transcript.push(entry);
        }
        tools.transcripts.insert("sa-1".into(), transcript);
        if live {
            tools.active.push(ActiveToolBlock {
                phys_idx: 0,
                tool_id: "sa-1".into(),
                output: subagent_card(true),
                live_output: crate::protocol::ToolOutputBuffer::new_full(1024),
                started_at: std::time::Instant::now(),
                subagent_child_id: None,
            });
        } else {
            tools.blocks.push(ToolBlock {
                phys_idx: 0,
                tool_id: "sa-1".into(),
                output: subagent_card(false),
            });
        }
        tools
    }

    fn prepare(entries: Vec<SubagentTranscriptEntry>, live: bool, width: u16) -> SubagentPopup {
        prepare_with(entries, live, width, &[])
    }

    /// `expanded` is the set of child step ids whose blocks show the raised
    /// cap — the state the host's click/key toggle builds.
    fn prepare_with(
        entries: Vec<SubagentTranscriptEntry>,
        live: bool,
        width: u16,
        expanded: &[&str],
    ) -> SubagentPopup {
        let tools = tools_with(entries, live);
        let theme = theme();
        let msgs = Messages::by_language(Language::English);
        let mut popup = SubagentPopup {
            title: "53. Subagent".into(),
            scroll: 0,
            tool_id: "sa-1".into(),
            selection: None,
            layout: None,
            expanded_steps: expanded.iter().map(|key| (*key).to_string()).collect(),
            expanded_rev: u64::from(!expanded.is_empty()),
        };
        prepare_subagent_popup(&mut popup, &tools, &theme, &msgs, width);
        popup
    }

    /// The toggle rows the layout reports, as `(step key, row index)`.
    fn block_toggles(layout: &SubagentPopupLayout) -> Vec<(String, usize)> {
        layout
            .segments
            .iter()
            .filter_map(|segment| match segment {
                PopupSegment::Block { toggles, .. } => Some(toggles),
                _ => None,
            })
            .flatten()
            .map(|(row, key)| (key.clone(), *row))
            .collect()
    }

    /// A block row's text, as drawn.
    fn row_text(row: &BlockRow) -> String {
        row.row
            .spans(None)
            .into_iter()
            .map(|span| span.content)
            .collect()
    }

    /// The plain text of every row of the layout's block segments.
    fn block_rows(layout: &SubagentPopupLayout) -> Vec<String> {
        layout
            .segments
            .iter()
            .filter_map(|segment| match segment {
                PopupSegment::Block { rows, .. } => Some(rows),
                _ => None,
            })
            .flatten()
            .map(row_text)
            .collect()
    }

    fn kinds(layout: &SubagentPopupLayout) -> Vec<&'static str> {
        layout
            .segments
            .iter()
            .map(|segment| match segment {
                PopupSegment::Meta { .. } => "meta",
                PopupSegment::Text { .. } => "text",
                PopupSegment::Block { .. } => "block",
            })
            .collect()
    }

    fn thinking_and_answer() -> Vec<SubagentTranscriptEntry> {
        vec![
            SubagentTranscriptEntry::Thinking {
                text: "why".into(),
                duration_us: Some(1_200),
            },
            SubagentTranscriptEntry::Step(step("t1", SubagentStepStatus::Success)),
            SubagentTranscriptEntry::Assistant("answer".into()),
        ]
    }

    #[test]
    fn layout_puts_the_meta_row_first_and_the_prompt_next() {
        let popup = prepare(
            vec![SubagentTranscriptEntry::Assistant("hello".into())],
            true,
            60,
        );
        let layout = popup.layout.expect("layout");
        assert_eq!(kinds(&layout), vec!["meta", "text", "text"]);
        assert!(layout.selection_text.contains("Research bitcoin."));
        assert!(layout.selection_text.contains("hello"));
    }

    /// The prompt is a generated brief, not typed text: its Markdown has to be
    /// rendered, or the one part of the popup a reader always sees looks broken.
    #[test]
    fn the_prompt_is_rendered_as_markdown() {
        let mut tools = tools_with(vec![SubagentTranscriptEntry::Assistant("ok".into())], true);
        // `arg_full` is the prompt; give the card a Markdown brief.
        let prompt = "## Review checklist\n\n1. **Spec conformance** — cite `file:line`";
        if let Some(active) = tools.active.first_mut() {
            active.output.arg_full = prompt.into();
        }
        let theme = theme();
        let msgs = Messages::by_language(Language::English);
        let mut popup = SubagentPopup {
            title: "4. Subagent".into(),
            scroll: 0,
            tool_id: "sa-1".into(),
            selection: None,
            layout: None,
            expanded_steps: HashSet::new(),
            expanded_rev: 0,
        };
        prepare_subagent_popup(&mut popup, &tools, &theme, &msgs, 60);
        let layout = popup.layout.expect("layout");

        assert!(
            layout.selection_text.contains("Review checklist"),
            "the heading text must survive:\n{}",
            layout.selection_text
        );
        assert!(
            !layout.selection_text.contains("##"),
            "the heading marker must not reach the screen:\n{}",
            layout.selection_text
        );
        assert!(
            !layout.selection_text.contains("**"),
            "bold markers must not reach the screen:\n{}",
            layout.selection_text
        );
        assert!(
            layout.selection_text.starts_with("💬 "),
            "the user marker still leads the prompt:\n{}",
            layout.selection_text
        );
    }

    /// The marker is prepended to the selection text and to the first display
    /// row by the same byte count, so a row's global offsets still index the
    /// string the copy path slices.
    #[test]
    fn the_prompt_marker_keeps_display_offsets_aligned() {
        let mut tools = tools_with(vec![SubagentTranscriptEntry::Assistant("ok".into())], true);
        if let Some(active) = tools.active.first_mut() {
            active.output.arg_full = "## Title\n\nbody text".into();
        }
        let theme = theme();
        let msgs = Messages::by_language(Language::English);
        let mut popup = SubagentPopup {
            title: "4. Subagent".into(),
            scroll: 0,
            tool_id: "sa-1".into(),
            selection: None,
            layout: None,
            expanded_steps: HashSet::new(),
            expanded_rev: 0,
        };
        prepare_subagent_popup(&mut popup, &tools, &theme, &msgs, 60);
        let layout = popup.layout.expect("layout");
        let (bytes, rows) = layout
            .segments
            .iter()
            .find_map(|segment| match segment {
                PopupSegment::Text { bytes, rows, .. } => Some((bytes, rows)),
                _ => None,
            })
            .expect("the prompt segment");

        assert_eq!(bytes.start, 0);
        assert_eq!(
            &layout.selection_text[rows[0].line_start..rows[0].line_end],
            "💬 Title",
            "the first row's byte range must cover the marker and the heading"
        );
    }

    #[test]
    fn segments_are_contiguous_and_total_rows_add_up() {
        let popup = prepare(thinking_and_answer(), false, 60);
        let layout = popup.layout.expect("layout");
        let mut next = 0;
        for segment in &layout.segments {
            assert_eq!(segment.vis_start(), next, "segments must be contiguous");
            assert!(segment.height() > 0, "no empty segment");
            next += segment.height();
        }
        assert_eq!(layout.total_rows, next);
    }

    /// The whole point of the rewrite: finishing a run must not re-flow the
    /// body. Same entries, same width ⇒ same segments, same row count, same
    /// selectable text.
    #[test]
    fn live_and_completed_build_the_same_segments() {
        let live = prepare(thinking_and_answer(), true, 60)
            .layout
            .expect("live layout");
        let done = prepare(thinking_and_answer(), false, 60)
            .layout
            .expect("done layout");
        assert_eq!(kinds(&live), kinds(&done));
        assert_eq!(live.total_rows, done.total_rows);
        assert_eq!(live.selection_text, done.selection_text);
    }

    #[test]
    fn a_step_started_then_finished_is_one_card() {
        let popup = prepare(
            vec![
                SubagentTranscriptEntry::Step(step("t1", SubagentStepStatus::Running)),
                SubagentTranscriptEntry::Step(step("t1", SubagentStepStatus::Success)),
            ],
            true,
            60,
        );
        let layout = popup.layout.expect("layout");
        assert_eq!(
            kinds(&layout).iter().filter(|k| **k == "block").count(),
            1,
            "the terminal half must rewrite the running block"
        );
    }

    #[test]
    fn stale_layout_is_rebuilt_when_the_transcript_grows() {
        let theme = theme();
        let msgs = Messages::by_language(Language::English);
        let mut tools = tools_with(vec![SubagentTranscriptEntry::Assistant("one".into())], true);
        let mut popup = SubagentPopup {
            title: "53. Subagent".into(),
            scroll: 0,
            tool_id: "sa-1".into(),
            selection: None,
            layout: None,
            expanded_steps: HashSet::new(),
            expanded_rev: 0,
        };
        prepare_subagent_popup(&mut popup, &tools, &theme, &msgs, 60);
        let first = popup.layout.as_ref().expect("layout").revision;

        // A second append bumps the revision, so the cache must be rebuilt.
        tools
            .transcripts
            .get_mut("sa-1")
            .expect("transcript")
            .push(SubagentTranscriptEntry::Assistant("two".into()));
        prepare_subagent_popup(&mut popup, &tools, &theme, &msgs, 60);
        let layout = popup.layout.as_ref().expect("layout");
        assert!(layout.revision > first);
        assert!(layout.selection_text.contains("two"));
    }

    #[test]
    fn a_collapsed_tool_card_is_expanded_in_the_popup() {
        // A finished `read_file` collapses to two rows in the log; the popup
        // has room and no click routing yet, so it draws the output card.
        let mut read = step("t1", SubagentStepStatus::Success);
        read.presentation = ToolPresentationInfo {
            visual_kind: ToolVisualKind::FileRead,
            display_name: "📖 Read".into(),
            keep_full_live_output: false,
            detail: crate::protocol::ToolDetailKind::Result,
            popup: crate::protocol::ToolPopupKind::None,
            compact_result_to_meta: false,
            keep_live: false,
        };
        read.detail = Some("line one\nline two\nline three".into());
        let popup = prepare(vec![SubagentTranscriptEntry::Step(read)], false, 60);
        let layout = popup.layout.expect("layout");
        let rows = block_rows(&layout);
        assert_eq!(rows.len(), 5, "title + phase + 3 output rows: {rows:?}");
        assert!(
            rows.iter().any(|row| row.contains("line three")),
            "the output is inline: {rows:?}"
        );
        assert!(
            !rows.iter().any(|row| row.contains("more lines")),
            "nothing was left out: {rows:?}"
        );
    }

    /// A tool block says how much it left out *and* offers to show it — in
    /// place, not in a second overlay: the row is the toggle, and no bracketed
    /// chrome is drawn (a popup inside a popup is what this surface cannot
    /// offer).
    #[test]
    fn a_long_tool_output_is_capped_and_counted() {
        let mut read = step("t1", SubagentStepStatus::Success);
        read.presentation = ToolPresentationInfo {
            visual_kind: ToolVisualKind::FileRead,
            display_name: "📖 Read".into(),
            keep_full_live_output: false,
            detail: crate::protocol::ToolDetailKind::Result,
            popup: crate::protocol::ToolPopupKind::None,
            compact_result_to_meta: false,
            keep_live: false,
        };
        let lines: Vec<String> = (0..20).map(|i| format!("line {i}")).collect();
        read.detail = Some(lines.join("\n"));
        let popup = prepare(vec![SubagentTranscriptEntry::Step(read)], false, 60);
        let layout = popup.layout.expect("layout");
        let rows = block_rows(&layout);

        assert_eq!(rows.len(), 2 + POPUP_TOOL_PREVIEW_LINES + 1);
        assert!(
            rows.iter().any(|row| row.contains("12 more lines")),
            "{rows:?}"
        );
        assert!(
            !rows.iter().any(|row| row.contains("[")),
            "no bracketed chrome is drawn: {rows:?}"
        );
        assert_eq!(
            block_toggles(&layout),
            vec![("t1".to_string(), 2 + POPUP_TOOL_PREVIEW_LINES)],
            "the counting row is the click target"
        );
    }

    /// A read step with `lines` lines of output, so the cap can be exercised
    /// from either side of the toggle.
    fn read_step(key: &str, lines: usize) -> SubagentStep {
        let mut read = step(key, SubagentStepStatus::Success);
        read.presentation = ToolPresentationInfo {
            visual_kind: ToolVisualKind::FileRead,
            display_name: "📖 Read".into(),
            keep_full_live_output: false,
            detail: crate::protocol::ToolDetailKind::Result,
            popup: crate::protocol::ToolPopupKind::None,
            compact_result_to_meta: false,
            keep_live: false,
        };
        read.detail = Some(
            (0..lines)
                .map(|i| format!("line {i}"))
                .collect::<Vec<_>>()
                .join("\n"),
        );
        read
    }

    /// Each block segment's rows, so a test can tell one block's shape from
    /// another's.
    fn blocks(layout: &SubagentPopupLayout) -> Vec<Vec<String>> {
        layout
            .segments
            .iter()
            .filter_map(|segment| match segment {
                PopupSegment::Block { rows, .. } => Some(rows),
                _ => None,
            })
            .map(|rows| rows.iter().map(row_text).collect())
            .collect()
    }

    /// The rows of the layout's first block segment.
    fn block_segment(layout: &SubagentPopupLayout) -> &[BlockRow] {
        layout
            .segments
            .iter()
            .find_map(|segment| match segment {
                PopupSegment::Block { rows, .. } => Some(rows.as_slice()),
                _ => None,
            })
            .expect("a block segment")
    }

    /// A running block's phase row is where the animation lives: the layout bakes
    /// the semantic `●` (the copy buffer keeps it) and marks the cell the draw
    /// pass swaps each frame.
    #[test]
    fn a_running_block_marks_the_glyph_cell_for_the_draw_pass() {
        let running = prepare(
            vec![SubagentTranscriptEntry::Step(step(
                "t1",
                SubagentStepStatus::Running,
            ))],
            true,
            60,
        );
        let layout = running.layout.expect("layout");
        let segment = block_segment(&layout);
        assert!(
            row_text(&segment[1]).contains("● Running"),
            "the baked text keeps the semantic marker: {:?}",
            row_text(&segment[1])
        );
        assert_eq!(
            segment[1].spinner,
            Some(BLOCK_TITLE_COLS as u16),
            "the phase row owns the animated cell"
        );
        assert!(segment[0].spinner.is_none(), "the title row does not");

        let done = prepare(
            vec![SubagentTranscriptEntry::Step(step(
                "t1",
                SubagentStepStatus::Success,
            ))],
            false,
            60,
        );
        let done_layout = done.layout.expect("layout");
        assert!(
            block_segment(&done_layout)
                .iter()
                .all(|row| row.spinner.is_none()),
            "a finished block animates nothing"
        );
    }

    /// A block's rows are selectable, and they point at the *content* in the copy
    /// buffer: a thinking block's label is chrome, its reasoning is not.
    #[test]
    fn a_thinking_block_row_covers_the_reasoning_not_the_label() {
        let reasoning = "check two sources";
        let popup = prepare(
            vec![SubagentTranscriptEntry::Thinking {
                text: reasoning.into(),
                duration_us: Some(1_200),
            }],
            false,
            60,
        );
        let layout = popup.layout.expect("layout");
        let at = layout
            .selection_text
            .find(reasoning)
            .expect("the reasoning is copied");
        let rows = blocks(&layout);
        assert_eq!(rows.len(), 1, "one block segment: {rows:?}");
        assert_eq!(rows[0].len(), 2, "label + reasoning: {rows:?}");

        let segment = layout
            .segments
            .iter()
            .find_map(|segment| match segment {
                PopupSegment::Block { rows, .. } => Some(rows),
                _ => None,
            })
            .expect("a block segment");
        let label = &segment[0].row;
        assert_eq!(
            label.line_start, label.line_end,
            "the `🧠 Thought · ⏱ …` label owns no bytes"
        );
        let text = &segment[1].row;
        assert_eq!(
            (text.line_start, text.line_end),
            (at, at + reasoning.len()),
            "the reasoning row covers exactly the reasoning"
        );
    }

    /// The cap is per block, and the row that states it is the row that lifts
    /// it: `⋯ 12 more lines · ▾ expand` while capped, `▴ collapse` once raised
    /// (count still appended when even the raised cap hid something).
    #[test]
    fn the_overflow_row_is_the_toggle() {
        let collapsed = prepare(
            vec![SubagentTranscriptEntry::Step(read_step("t1", 20))],
            false,
            60,
        );
        let layout = collapsed.layout.expect("layout");
        let rows = block_rows(&layout);
        assert_eq!(rows.len(), 2 + POPUP_TOOL_PREVIEW_LINES + 1);
        assert!(
            rows.iter().any(|row| row.contains("12 more lines")),
            "{rows:?}"
        );
        assert!(
            rows.last().expect("a row").contains("expand"),
            "the counting row offers the gesture: {rows:?}"
        );

        // 20 lines under the raised cap: everything fits, and the row offers to
        // put the cap back.
        let expanded = prepare_with(
            vec![SubagentTranscriptEntry::Step(read_step("t1", 20))],
            false,
            60,
            &["t1"],
        );
        let rows = block_rows(&expanded.layout.expect("layout"));
        assert_eq!(
            rows.len(),
            2 + 20 + 1,
            "every line fits, plus the collapse row: {rows:?}"
        );
        assert!(
            !rows.iter().any(|row| row.contains("more lines")),
            "{rows:?}"
        );
        assert!(rows.last().expect("a row").contains("collapse"), "{rows:?}");

        // 60 lines: the raised cap is still a cap, and the row says so.
        let expanded = prepare_with(
            vec![SubagentTranscriptEntry::Step(read_step("t1", 60))],
            false,
            60,
            &["t1"],
        );
        let rows = block_rows(&expanded.layout.expect("layout"));
        assert_eq!(
            rows.len(),
            2 + POPUP_TOOL_EXPANDED_LINES + 1,
            "the safety cap still counts: {rows:?}"
        );
        let last = rows.last().expect("a row");
        assert!(
            last.contains("collapse") && last.contains("10 more lines"),
            "{last:?}"
        );
    }

    /// Expansion is per block: one block's click must not reflow the others.
    #[test]
    fn only_the_clicked_block_is_expanded() {
        let popup = prepare_with(
            vec![
                SubagentTranscriptEntry::Step(read_step("t1", 20)),
                SubagentTranscriptEntry::Step(read_step("t2", 20)),
            ],
            false,
            60,
            &["t1"],
        );
        let layout = popup.layout.expect("layout");
        let blocks = blocks(&layout);
        assert_eq!(blocks.len(), 2);
        assert_eq!(
            blocks[0].len(),
            2 + 20 + 1,
            "t1 is expanded: {:?}",
            blocks[0]
        );
        assert!(blocks[0].last().expect("a row").contains("collapse"));
        assert_eq!(
            blocks[1].len(),
            2 + POPUP_TOOL_PREVIEW_LINES + 1,
            "t2 is untouched: {:?}",
            blocks[1]
        );
        assert!(blocks[1].last().expect("a row").contains("expand"));
        assert_eq!(
            block_toggles(&layout).len(),
            2,
            "both blocks keep a row to click"
        );
    }

    /// The expansion counter is the layout cache key: the host must bump it
    /// along with the set, or the toggle would only take effect on the next
    /// append.
    #[test]
    fn flipping_expansion_rebuilds_the_layout() {
        let tools = tools_with(
            vec![SubagentTranscriptEntry::Step(read_step("t1", 20))],
            false,
        );
        let theme = theme();
        let msgs = Messages::by_language(Language::English);
        let mut popup = SubagentPopup {
            title: "53. Subagent".into(),
            scroll: 0,
            tool_id: "sa-1".into(),
            selection: None,
            layout: None,
            expanded_steps: HashSet::new(),
            expanded_rev: 0,
        };
        prepare_subagent_popup(&mut popup, &tools, &theme, &msgs, 60);
        let collapsed = popup.layout.as_ref().expect("layout").total_rows;

        // The set alone changes nothing: the counter is the key.
        popup.expanded_steps.insert("t1".into());
        prepare_subagent_popup(&mut popup, &tools, &theme, &msgs, 60);
        assert_eq!(
            popup.layout.as_ref().expect("layout").total_rows,
            collapsed,
            "the counter is the cache key"
        );

        popup.expanded_rev += 1;
        prepare_subagent_popup(&mut popup, &tools, &theme, &msgs, 60);
        let layout = popup.layout.as_ref().expect("layout");
        assert_eq!(
            layout.expanded_rev, 1,
            "the rebuilt layout must record the counter"
        );
        assert!(
            layout.total_rows > collapsed,
            "the raised cap must reach the layout: {collapsed} -> {}",
            layout.total_rows
        );
    }

    /// A command's block drops the `$ <command>` line its detail opens with —
    /// the title already carries the command — and shows the *tail*.
    #[test]
    fn a_command_block_drops_its_argument_line_and_shows_the_tail() {
        let mut bash = step("t1", SubagentStepStatus::Success);
        bash.tool_name = "bash".into();
        bash.arg_summary = "grep -rn x".into();
        bash.arg_full = "grep -rn x".into();
        bash.presentation = ToolPresentationInfo {
            visual_kind: ToolVisualKind::Command,
            display_name: "⚡ Bash".into(),
            keep_full_live_output: false,
            detail: crate::protocol::ToolDetailKind::Result,
            popup: crate::protocol::ToolPopupKind::None,
            compact_result_to_meta: false,
            keep_live: false,
        };
        bash.detail = Some("hit one\nhit two\nhit three".into());
        let popup = prepare(vec![SubagentTranscriptEntry::Step(bash)], false, 60);
        let layout = popup.layout.expect("layout");
        let rows = block_rows(&layout);

        assert!(
            !rows.iter().any(|row| row.contains("$ grep")),
            "the argument line is the title's job: {rows:?}"
        );
        assert!(rows.iter().any(|row| row.contains("hit three")), "{rows:?}");
    }

    /// A thinking run is shown in full: it is only ever visible here.
    #[test]
    fn a_thinking_block_is_not_capped() {
        let text: String = (0..40)
            .map(|i| format!("thought {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let popup = prepare(
            vec![SubagentTranscriptEntry::Thinking {
                text,
                duration_us: Some(1_200),
            }],
            false,
            60,
        );
        let layout = popup.layout.expect("layout");
        let rows = block_rows(&layout);
        assert_eq!(rows.len(), 1 + 40, "title + every line: {rows:?}");
        assert!(
            rows.iter().any(|row| row.contains("thought 39")),
            "{rows:?}"
        );
    }

    /// A wrapped block row carries the rail on every visual row, not just the
    /// first — the reason the wrapping happens at build time.
    #[test]
    fn a_wrapped_block_row_repeats_its_rail() {
        let mut read = step("t1", SubagentStepStatus::Success);
        read.detail = Some("x".repeat(200));
        let popup = prepare(vec![SubagentTranscriptEntry::Step(read)], false, 40);
        let layout = popup.layout.expect("layout");
        let rows = block_rows(&layout);
        let output_rows: Vec<&String> = rows.iter().filter(|row| row.contains('│')).collect();
        assert!(output_rows.len() > 1, "the long line must wrap: {rows:?}");
        assert!(
            output_rows.iter().all(|row| row.contains('│')),
            "every wrapped row keeps the rail: {rows:?}"
        );
    }

    #[test]
    /// The selection stays in the popup's own byte space: a row's grapheme hits
    /// are global, and the hit rows feed `PopupTextSelection` directly.
    fn segment_selection_intersects_a_global_range_with_one_segment() {
        let bytes = 10..20;
        assert_eq!(segment_selection(&(12..15), &bytes), Some(12..15));
        assert_eq!(segment_selection(&(0..100), &bytes), Some(10..20));
        assert_eq!(segment_selection(&(0..5), &bytes), None);
        assert_eq!(segment_selection(&(20..30), &bytes), None);
    }

    /// Byte offsets on the display rows are global, so the popup's one
    /// selection range can be compared against any segment without translating.
    #[test]
    fn display_row_offsets_are_global_across_segments() {
        let popup = prepare(
            vec![
                SubagentTranscriptEntry::Assistant("first".into()),
                SubagentTranscriptEntry::Assistant("second".into()),
            ],
            true,
            60,
        );
        let layout = popup.layout.expect("layout");
        let text_segments: Vec<(&Range<usize>, &Vec<DisplayRow>)> = layout
            .segments
            .iter()
            .filter_map(|segment| match segment {
                PopupSegment::Text { bytes, rows, .. } => Some((bytes, rows)),
                _ => None,
            })
            .collect();
        assert_eq!(text_segments.len(), 3, "prompt + two assistant runs");
        for (bytes, rows) in text_segments {
            assert!(
                rows.iter()
                    .all(|row| row.line_start >= bytes.start && row.line_start <= bytes.end),
                "a row's offset must fall inside its own segment's range"
            );
        }
    }
}
