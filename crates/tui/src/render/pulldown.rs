//! `pulldown-cmark` → ratatui `Line` renderer.
//!
//! Replaces the forked `ratatui-markdown` block renderer for the main log
//! area. Mermaid fences are still routed by [`super::render_md::render_md`]
//! before this renderer runs, so this module only ever sees prose, lists,
//! tables, code fences and blockquotes.

use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};

use super::render_md::format_table;
use crate::theme::Theme;

/// Width of the horizontal-rule line (matches the fork's `max_width.min(80)`
/// behaviour at the log panel's unlimited render width).
const HR_WIDTH: usize = 80;
const BULLET: &str = "\u{2022} ";
const LIST_INDENT: &str = "    ";

/// Render one non-Mermaid Markdown chunk into styled lines + raw text.
///
/// `available_width` is forwarded to the width-aware pipe-table layout
/// ([`format_table`]); `None` means unlimited (the log panel wraps itself).
pub(crate) fn render_markdown(
    text: &str,
    theme: &Theme,
    available_width: Option<usize>,
) -> (Vec<Line<'static>>, Vec<String>) {
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_TASKLISTS);
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_FOOTNOTES);

    let parser = Parser::new_ext(text, opts);
    let mut writer = Writer::new(theme, available_width);
    writer.run(parser);
    let raw = writer.lines.iter().map(Line::to_string).collect();
    (writer.lines, raw)
}

/// Ordered/unordered list context; `number` is the next item's ordinal.
struct ListCtx {
    ordered: bool,
    number: u64,
}

/// In-progress table collection (headers + body rows).
struct TableCtx {
    headers: Vec<String>,
    rows: Vec<Vec<String>>,
    current_cell: String,
    current_row: Vec<String>,
    in_head: bool,
}

struct Writer {
    theme: Theme,
    available_width: Option<usize>,
    lines: Vec<Line<'static>>,

    /// Accumulated inline styles; the top is the fully-merged current style.
    inline_styles: Vec<Style>,
    /// Spans of the logical line currently being built.
    pending: Vec<Span<'static>>,

    list_stack: Vec<ListCtx>,
    /// True between `Start(Item)` and the item's first text (marker pending).
    in_item_start: bool,
    /// Task-list checkbox recorded by `TaskListMarker`, if any.
    task_marker: Option<bool>,

    /// Blockquote nesting level (drives the `│ ` gutter prefix).
    bq_level: u8,

    heading: Option<HeadingLevel>,

    /// `Some` while inside a fenced/indented code block.
    code_lang: Option<String>,
    code_buf: String,

    table: Option<TableCtx>,

    /// Destination of the innermost link, for the ` (url)` suffix.
    link_url: Option<String>,
    /// `pending.len()` at the link's open, to detect empty link text.
    link_text_start: usize,
}

impl Writer {
    fn new(theme: &Theme, available_width: Option<usize>) -> Self {
        Self {
            theme: *theme,
            available_width,
            lines: Vec::new(),
            inline_styles: Vec::new(),
            pending: Vec::new(),
            list_stack: Vec::new(),
            in_item_start: false,
            task_marker: None,
            bq_level: 0,
            heading: None,
            code_lang: None,
            code_buf: String::new(),
            table: None,
            link_url: None,
            link_text_start: 0,
        }
    }

    fn run(&mut self, parser: Parser<'_>) {
        for event in parser {
            self.handle_event(event);
        }
        // A trailing inline run without a closing block event (e.g. a lone
        // paragraph) is still flushed.
        self.flush_line();
    }

    fn handle_event(&mut self, event: Event<'_>) {
        match event {
            Event::Start(tag) => self.start_tag(tag),
            Event::End(tag_end) => self.end_tag(tag_end),
            Event::Text(text) => {
                if self.code_lang.is_some() {
                    self.code_buf.push_str(&text);
                } else if let Some(table) = self.table.as_mut() {
                    table.current_cell.push_str(&text);
                } else {
                    self.push_text(&text);
                }
            }
            Event::Code(code) => {
                if let Some(table) = self.table.as_mut() {
                    table.current_cell.push_str(&code);
                } else {
                    self.push_inline_code(&code);
                }
            }
            Event::SoftBreak => self.push_span(Span::styled(" ", self.current_style())),
            Event::HardBreak => self.flush_line(),
            Event::Rule => {
                self.ensure_separation();
                self.flush_line();
                self.lines.push(Line::from(Span::styled(
                    "─".repeat(HR_WIDTH),
                    Style::default().fg(self.theme.muted_fg()),
                )));
            }
            Event::TaskListMarker(checked) => self.task_marker = Some(checked),
            // HTML is out of scope for the log area; drop it rather than leak.
            Event::Html(_) | Event::InlineHtml(_) => {}
            Event::FootnoteReference(_) | Event::InlineMath(_) | Event::DisplayMath(_) => {}
        }
    }

    fn start_tag(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph => {
                if self.top_level() {
                    self.ensure_separation();
                }
            }
            Tag::Heading { level, .. } => {
                if self.top_level() {
                    self.ensure_separation();
                }
                self.heading = Some(level);
            }
            Tag::BlockQuote(_) => {
                if self.top_level() {
                    self.ensure_separation();
                }
                self.bq_level += 1;
            }
            Tag::CodeBlock(kind) => {
                if self.top_level() {
                    self.ensure_separation();
                }
                self.code_lang = Some(match kind {
                    CodeBlockKind::Fenced(lang) => lang.to_string(),
                    CodeBlockKind::Indented => String::new(),
                });
                self.code_buf.clear();
            }
            Tag::List(start) => {
                if self.top_level() {
                    self.ensure_separation();
                }
                self.list_stack.push(ListCtx {
                    ordered: start.is_some(),
                    number: start.unwrap_or(1),
                });
            }
            Tag::Item => {
                self.in_item_start = true;
                self.task_marker = None;
            }
            Tag::Table(_) => {
                if self.top_level() {
                    self.ensure_separation();
                }
                self.table = Some(TableCtx {
                    headers: Vec::new(),
                    rows: Vec::new(),
                    current_cell: String::new(),
                    current_row: Vec::new(),
                    in_head: false,
                });
            }
            Tag::TableHead => {
                if let Some(t) = self.table.as_mut() {
                    t.in_head = true;
                }
            }
            Tag::TableRow => {
                if let Some(t) = self.table.as_mut() {
                    t.current_row = Vec::new();
                }
            }
            Tag::TableCell => {
                if let Some(t) = self.table.as_mut() {
                    t.current_cell = String::new();
                }
            }
            Tag::Emphasis => self.push_inline(Style::default().add_modifier(Modifier::ITALIC)),
            Tag::Strong => self.push_inline(Style::default().add_modifier(Modifier::BOLD)),
            Tag::Strikethrough => {
                self.push_inline(Style::default().add_modifier(Modifier::CROSSED_OUT));
            }
            Tag::Link { dest_url, .. } => {
                self.link_text_start = self.pending.len();
                self.link_url = Some(dest_url.to_string());
                self.push_inline(
                    Style::default()
                        .fg(self.theme.accent)
                        .add_modifier(Modifier::UNDERLINED),
                );
            }
            // Footnotes, definition lists, images and metadata are out of
            // scope; ignore them (their text still flows through as events).
            Tag::FootnoteDefinition(_)
            | Tag::DefinitionList
            | Tag::DefinitionListTitle
            | Tag::DefinitionListDefinition
            | Tag::Image { .. }
            | Tag::HtmlBlock
            | Tag::MetadataBlock(_)
            | Tag::Superscript
            | Tag::Subscript => {}
        }
    }

    fn end_tag(&mut self, tag_end: TagEnd) {
        match tag_end {
            TagEnd::Paragraph => self.flush_line(),
            TagEnd::Heading(level) => self.flush_heading(level),
            TagEnd::BlockQuote(_) => {
                self.flush_line();
                self.bq_level = self.bq_level.saturating_sub(1);
            }
            TagEnd::CodeBlock => {
                self.flush_code_block();
                self.code_lang = None;
            }
            TagEnd::List(_) => {
                self.flush_line();
                self.list_stack.pop();
            }
            TagEnd::Item => self.flush_line(),
            TagEnd::Table => {
                self.flush_table();
                self.table = None;
            }
            TagEnd::TableHead => {
                if let Some(t) = self.table.as_mut() {
                    t.in_head = false;
                }
            }
            TagEnd::TableRow => {
                if let Some(t) = self.table.as_mut() {
                    let row = std::mem::take(&mut t.current_row);
                    t.rows.push(row);
                }
            }
            TagEnd::TableCell => {
                if let Some(t) = self.table.as_mut() {
                    let cell = std::mem::take(&mut t.current_cell);
                    if t.in_head {
                        t.headers.push(cell);
                    } else {
                        t.current_row.push(cell);
                    }
                }
            }
            TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough => {
                self.inline_styles.pop();
            }
            TagEnd::Link => {
                let link_style = self.inline_styles.pop().unwrap_or_default();
                if let Some(url) = self.link_url.take() {
                    if self.pending.len() > self.link_text_start {
                        self.pending
                            .push(Span::styled(format!(" ({url})"), link_style));
                    } else {
                        self.pending.push(Span::styled(url, link_style));
                    }
                }
            }
            TagEnd::FootnoteDefinition
            | TagEnd::DefinitionList
            | TagEnd::DefinitionListTitle
            | TagEnd::DefinitionListDefinition
            | TagEnd::Image
            | TagEnd::HtmlBlock
            | TagEnd::MetadataBlock(_)
            | TagEnd::Superscript
            | TagEnd::Subscript => {}
        }
    }

    /// True at the document top level (outside lists and blockquotes).
    fn top_level(&self) -> bool {
        self.bq_level == 0 && self.list_stack.is_empty()
    }

    /// Push a blank line before a block when the previous line is non-empty.
    fn ensure_separation(&mut self) {
        if self.lines.last().is_some_and(|l| !l.spans.is_empty()) {
            self.lines.push(Line::default());
        }
    }

    fn current_style(&self) -> Style {
        self.inline_styles.last().copied().unwrap_or_default()
    }

    fn push_inline(&mut self, style: Style) {
        let base = self.inline_styles.last().copied().unwrap_or_default();
        // `base.patch(style)` lets the new style win on fg/bg while unions
        // modifiers (ratatui 0.30 `patch` semantics).
        self.inline_styles.push(base.patch(style));
    }

    fn push_text(&mut self, text: &str) {
        if self.in_item_start {
            self.push_list_marker();
        }
        let mut style = self.current_style();
        if style.fg.is_none() {
            style = style.fg(self.theme.fg);
        }
        self.pending.push(Span::styled(text.to_string(), style));
    }

    fn push_inline_code(&mut self, code: &str) {
        if self.in_item_start {
            self.push_list_marker();
        }
        let style = self
            .current_style()
            .fg(self.theme.code_block_fg())
            .bg(self.theme.code_block_bg());
        self.pending.push(Span::styled(code.to_string(), style));
    }

    fn push_span(&mut self, span: Span<'static>) {
        if self.in_item_start {
            self.push_list_marker();
        }
        self.pending.push(span);
    }

    /// Emit the list item's leading indent + bullet/number/checkbox marker.
    fn push_list_marker(&mut self) {
        if !self.in_item_start {
            return;
        }
        self.in_item_start = false;
        let indent = LIST_INDENT.repeat(self.list_stack.len().saturating_sub(1));
        let marker = match self.task_marker {
            Some(checked) => {
                let checkbox = if checked { "☑ " } else { "☐ " };
                if let Some(top) = self.list_stack.last_mut()
                    && top.ordered
                {
                    let n = top.number;
                    top.number += 1;
                    format!("{n}. {checkbox}")
                } else {
                    checkbox.to_string()
                }
            }
            None => {
                let mut m = BULLET.to_string();
                if let Some(top) = self.list_stack.last_mut()
                    && top.ordered
                {
                    let n = top.number;
                    top.number += 1;
                    m = format!("{n}. ");
                }
                m
            }
        };
        self.pending.push(Span::raw(format!("{indent}{marker}")));
    }

    /// Flush `pending` as one line, prepending the blockquote gutter.
    fn flush_line(&mut self) {
        let spans = std::mem::take(&mut self.pending);
        if spans.is_empty() {
            return;
        }
        let mut line = Line::from(spans);
        if self.bq_level > 0 {
            let prefix = "│ ".repeat(self.bq_level as usize);
            line.spans.insert(
                0,
                Span::styled(prefix, Style::default().fg(self.theme.muted_fg())),
            );
        }
        self.lines.push(line);
    }

    fn heading_style(&self, level: HeadingLevel) -> Style {
        let (fg, modifier) = match level {
            HeadingLevel::H1 => (self.theme.heading, Modifier::BOLD | Modifier::UNDERLINED),
            HeadingLevel::H2 => (self.theme.heading, Modifier::BOLD),
            HeadingLevel::H3 => (self.theme.heading, Modifier::BOLD | Modifier::ITALIC),
            HeadingLevel::H4 => (self.theme.fg, Modifier::BOLD | Modifier::ITALIC),
            HeadingLevel::H5 => (self.theme.fg, Modifier::ITALIC),
            HeadingLevel::H6 => (self.theme.fg, Modifier::ITALIC),
        };
        Style::default().fg(fg).add_modifier(modifier)
    }

    fn flush_heading(&mut self, level: HeadingLevel) {
        let heading_style = self.heading_style(level);
        let spans = std::mem::take(&mut self.pending);
        if spans.is_empty() {
            return;
        }
        let styled: Vec<Span<'static>> = spans
            .into_iter()
            .map(|sp| {
                // `sp.patch(heading)` lets the heading fg/modifier win while
                // keeping inline emphasis's own modifiers (union).
                let mut s = sp;
                s.style = s.style.patch(heading_style);
                s
            })
            .collect();
        self.lines.push(Line::from(styled));
        self.heading = None;
    }

    fn flush_code_block(&mut self) {
        let code = std::mem::take(&mut self.code_buf);
        let code = code.trim_end_matches('\n');
        let code_style = Style::default()
            .fg(self.theme.code_block_fg())
            .bg(self.theme.code_block_bg());
        let mut lines = vec![Line::default()];
        lines.extend(
            code.split('\n')
                .map(|l| Line::from(Span::styled(l.to_string(), code_style))),
        );
        lines.push(Line::default());
        self.lines.extend(lines);
    }

    fn flush_table(&mut self) {
        let Some(table) = self.table.take() else {
            return;
        };
        if table.headers.is_empty() && table.rows.is_empty() {
            return;
        }
        let mut md_lines: Vec<String> = vec![format!("| {} |", table.headers.join(" | "))];
        md_lines.push(format!(
            "| {} |",
            vec!["---"; table.headers.len()].join(" | ")
        ));
        for row in &table.rows {
            md_lines.push(format!("| {} |", row.join(" | ")));
        }
        let (styled, _raw) = format_table(&md_lines, &self.theme, self.available_width);
        self.lines.extend(styled);
    }
}
