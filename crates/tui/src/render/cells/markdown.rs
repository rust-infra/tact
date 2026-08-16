//! Renderable cell for native Markdown text.
//!
//! `MarkdownCell` is the render half for raw Markdown strings: it accepts one
//! string (`new`) or several fragments (`from_strings`) and renders them
//! through the shared width-aware pipeline (`render_markdown_with_tables`):
//! pipe-table rows are extracted and laid out against the panel width via
//! `format_table` (columns shrink, long cells wrap inside the table, pipes
//! stay aligned), complete top-level ` ```mermaid ` fences are rendered as
//! terminal diagrams at the same content width, and everything else goes to
//! tui-markdown directly (`render_markdown_tui`, the themed `from_str`
//! pipeline), so headings / lists / blockquotes / fenced code keep their
//! theme styling.
//!
//! Rendering is lazy and cached per content width: the cell stores only the
//! source text until the log renderer asks for its height, and re-layouts
//! when the panel resizes (both the table layout and the `wrap_line`
//! fallback are width-dependent, so the cache key is the content width).

use std::cell::RefCell;

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::Style,
    text::Line,
    widgets::{Paragraph, Widget},
};

use crate::{
    render::{render_md::render_markdown_with_tables, renderable::Renderable, util::wrap_line},
    theme::Theme,
};

/// A rendered Markdown document at one specific content width.
#[allow(dead_code)] // used by `MarkdownCell::cached_lines` (dead until a caller wires the cell in)
struct RenderedMarkdown {
    width: u16,
    lines: Vec<Line<'static>>,
}

/// Rendering unit for a block of Markdown text.
///
/// Input is native Markdown — one string or several strings that are joined
/// with newlines. The cell never mutates the source; it only renders it.
#[allow(dead_code)] // component cell; wired in by callers (log rows / popups), exercised by unit tests
pub(crate) struct MarkdownCell {
    /// Raw Markdown source (multiple strings joined with `\n`).
    source: String,
    /// Theme snapshot for styling (theme is `Copy`).
    theme: Theme,
    /// Left gutter columns applied before rendering (log-panel indents).
    indent_cols: u16,
    /// Width-keyed render cache; `None` until the first `height`/render call.
    cache: RefCell<Option<RenderedMarkdown>>,
}

#[allow(dead_code)] // component cell; wired in by callers (log rows / popups), exercised by unit tests
impl MarkdownCell {
    /// Create a cell from a single Markdown string.
    ///
    /// The string may be a full document containing newlines: paragraphs
    /// separated by blank lines, fenced code blocks, lists and tables keep
    /// their structure. Per CommonMark, a single newline *inside* a
    /// paragraph is a soft break and renders as a space; use two trailing
    /// spaces before the newline (`  \n`) for a hard line break.
    pub(crate) fn new(text: impl Into<String>, theme: &Theme) -> Self {
        Self::from_strings([text], theme)
    }

    /// Create a cell from multiple Markdown fragments joined by newlines.
    ///
    /// ```text
    /// MarkdownCell::from_strings(["# Title", "- a", "- b"], &theme)
    /// ```
    /// is equivalent to `MarkdownCell::new("# Title\n- a\n- b", &theme)`.
    /// Joining never alters the fragments themselves: newlines already
    /// inside a fragment are preserved as-is.
    pub(crate) fn from_strings<I, S>(parts: I, theme: &Theme) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let mut source = String::new();
        for (i, part) in parts.into_iter().enumerate() {
            if i > 0 {
                source.push('\n');
            }
            source.push_str(&part.into());
        }
        Self {
            source,
            theme: *theme,
            indent_cols: 0,
            cache: RefCell::new(None),
        }
    }

    /// Indent the rendered content by `cols` gutter columns.
    ///
    /// Mirrors `TextCell`'s `indent_cols`: the indent narrows the content
    /// width used for layout, so wrapped lines and tables still fit.
    pub(crate) fn with_indent(mut self, cols: u16) -> Self {
        self.indent_cols = cols;
        self
    }

    /// Render (and cache) the Markdown at the given content width.
    ///
    /// The pipeline is width-aware: pipe tables are laid out against
    /// `content_width` (`format_table` wraps long cells inside the table),
    /// and every other line that still exceeds the width is wrapped with
    /// `wrap_line`, so the returned line count is the exact visual height.
    fn render_if_needed(&self, width: u16) {
        let content_width = width.saturating_sub(self.indent_cols).max(1);
        if self
            .cache
            .borrow()
            .as_ref()
            .is_some_and(|c| c.width == content_width)
        {
            return;
        }
        let (styled, _raw) =
            render_markdown_with_tables(&self.source, &self.theme, Some(content_width as usize));
        let lines = styled
            .into_iter()
            .flat_map(|line| wrap_line(&line, content_width as usize))
            .collect::<Vec<_>>();
        *self.cache.borrow_mut() = Some(RenderedMarkdown {
            width: content_width,
            lines,
        });
    }

    /// Borrow the rendered lines at `width` (rendering first if needed).
    ///
    /// Borrowing avoids the per-frame full clone: `height` only reads the
    /// length, and `render_partial` clones just the visible slice.
    fn cached_lines(&self, width: u16) -> std::cell::Ref<'_, Vec<Line<'static>>> {
        self.render_if_needed(width);
        std::cell::Ref::map(self.cache.borrow(), |c| {
            &c.as_ref()
                .expect("render_if_needed just populated the cache")
                .lines
        })
    }
}

impl Renderable for MarkdownCell {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        self.render_partial(area, buf, 0);
    }

    fn height(&self, width: u16) -> u16 {
        self.cached_lines(width).len() as u16
    }

    fn render_partial(&self, area: Rect, buf: &mut Buffer, skip_lines: usize) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        // `render_if_needed` subtracts the indent itself, so the layout width
        // must come from the full area width — not from an already-indented
        // rect.
        let lines = self.cached_lines(area.width);
        let start = skip_lines.min(lines.len());
        if start >= lines.len() {
            return;
        }
        let content = crate::render::util::indent_rect(area, self.indent_cols);
        if content.width == 0 || content.height == 0 {
            return;
        }
        // Clone only the visible slice (bounded by the viewport height), not
        // the whole document, on every frame.
        let end = (start + content.height as usize).min(lines.len());
        Paragraph::new(lines[start..end].to_vec())
            .style(Style::default().bg(self.theme.bg))
            .render(content, buf);
    }
}

impl Renderable for &MarkdownCell {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        (*self).render(area, buf);
    }

    fn height(&self, width: u16) -> u16 {
        (*self).height(width)
    }

    fn render_partial(&self, area: Rect, buf: &mut Buffer, skip_lines: usize) {
        (*self).render_partial(area, buf, skip_lines);
    }
}

#[cfg(test)]
mod tests {
    use ratatui::{Terminal, backend::TestBackend, style::Modifier};

    use super::*;
    use crate::render::test_harness::buffer_text;
    use crate::theme::{Theme, ThemeName};

    fn dark() -> Theme {
        Theme::from(ThemeName::Dark)
    }

    fn render_text(cell: &MarkdownCell, width: u16) -> String {
        let height = cell.height(width).max(1);
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| cell.render(frame.area(), frame.buffer_mut()))
            .expect("draw");
        buffer_text(terminal.backend().buffer())
    }

    fn render_viewport(cell: &MarkdownCell, width: u16, height: u16) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| cell.render(frame.area(), frame.buffer_mut()))
            .expect("draw");
        buffer_text(terminal.backend().buffer())
    }

    #[test]
    fn renders_heading_and_list() {
        let cell = MarkdownCell::new("# Title\n\n- item one\n- item two", &dark());
        let text = render_text(&cell, 80);
        assert!(text.contains("Title"), "{text}");
        assert!(text.contains("item one"), "{text}");
        assert!(text.contains("item two"), "{text}");
    }

    #[test]
    fn markdown_cell_renders_mermaid_at_the_requested_width() {
        let cell = MarkdownCell::new(
            "```mermaid\nsequenceDiagram\n  Alice->>Bob: Hello\n```",
            &dark(),
        );
        let text = render_text(&cell, 60);

        assert!(
            text.contains("Alice") && text.contains("Bob"),
            "diagram missing: {text}"
        );
        assert!(
            !text.contains("sequenceDiagram"),
            "raw Mermaid leaked: {text}"
        );
    }

    #[test]
    fn heading_line_is_styled_bold() {
        let cell = MarkdownCell::new("# Title\nplain body", &dark());
        let backend = TestBackend::new(80, cell.height(80));
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| cell.render(frame.area(), frame.buffer_mut()))
            .expect("draw");
        let buf = terminal.backend().buffer();
        let y = (0..buf.area.height)
            .find(|&y| (0..buf.area.width).any(|x| buf[(x, y)].symbol() == "T"))
            .expect("heading text row");
        let bold = (0..buf.area.width).any(|x| buf[(x, y)].modifier.contains(Modifier::BOLD));
        assert!(bold, "heading row should be bold");
    }

    #[test]
    fn indented_cell_shifts_content_right() {
        // Whole-Markdown log messages align with assistant replies
        // (LOG_THINKING_INDENT + 1 = 3) instead of hugging the left border.
        let cell = MarkdownCell::new("# Title", &dark()).with_indent(3);
        let backend = TestBackend::new(20, 2);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| cell.render(frame.area(), frame.buffer_mut()))
            .expect("draw");
        let buf = terminal.backend().buffer();
        assert_eq!(buf[(0, 0)].symbol(), " ", "left gutter must stay blank");
        assert_eq!(buf[(2, 0)].symbol(), " ", "col 2 must stay blank");
        // The heading marker is stripped at render time; the heading text
        // itself starts at col 3.
        assert_eq!(buf[(3, 0)].symbol(), "T", "content starts at col 3");
    }

    #[test]
    fn single_string_with_newlines_renders_full_document() {
        // 一整段 markdown 字符串，包含结构换行（空行分隔段落）、段落内软换行、
        // fenced 代码块、列表和管道表格。
        let md = "\
# Title

First paragraph
continues on the next line (soft break).

Second paragraph.

```rust
fn whole_doc() {}
```

- item one
- item two

| A | B |
| --- | --- |
| 1 | 2 |
";
        let cell = MarkdownCell::new(md, &dark());
        let text = render_text(&cell, 80);

        assert!(text.contains("Title"), "{text}");
        // 空行分隔的段落各自独立成行。
        assert!(text.contains("Second paragraph."), "{text}");
        // 段落内单个换行是软换行，按 CommonMark 合并为空格。
        assert!(
            text.contains("First paragraph continues on the next line (soft break)."),
            "soft break should collapse into a space:\n{text}"
        );
        // 结构元素全部保留。宽度感知路径下表格是管道风格（| A | B |）。
        assert!(text.contains("fn whole_doc() {}"), "{text}");
        assert!(
            text.contains("item one") && text.contains("item two"),
            "{text}"
        );
        assert!(
            text.contains("| A | B |") && text.contains("| 1 | 2 |"),
            "table rows must survive:\n{text}"
        );
    }

    #[test]
    fn hard_break_keeps_line_break() {
        // 行尾两个空格 + 换行是硬换行，必须保留换行而不是合并成空格。
        let cell = MarkdownCell::new("line one  \nline two", &dark());
        let text = render_text(&cell, 80);
        // buffer_text 每行会补齐宽度空格，所以按 trim 后的行内容断言。
        let rows: Vec<&str> = text.lines().map(|l| l.trim()).collect();
        let one = rows.iter().position(|l| *l == "line one");
        let two = rows.iter().position(|l| *l == "line two");
        assert!(one.is_some(), "{text}");
        assert!(two.is_some(), "{text}");
        assert!(
            two > one,
            "hard break must keep both lines separate:\n{text}"
        );
    }

    #[test]
    fn from_strings_joins_fragments() {
        let cell = MarkdownCell::from_strings(["# A", "body one", "body two"], &dark());
        let text = render_text(&cell, 80);
        assert!(text.contains("A"), "{text}");
        assert!(text.contains("body one"), "{text}");
        assert!(text.contains("body two"), "{text}");
    }

    #[test]
    fn empty_source_renders_nothing() {
        let cell = MarkdownCell::new("", &dark());
        assert_eq!(cell.height(80), 0);
        let text = render_viewport(&cell, 80, 3);
        assert!(text.trim().is_empty(), "{text}");
    }

    #[test]
    fn height_grows_when_width_shrinks() {
        let cell = MarkdownCell::new(
            "a long paragraph that definitely wraps at narrow widths but stays on one line at wide widths",
            &dark(),
        );
        assert_eq!(cell.height(120), 1, "fits on one line when wide");
        assert!(
            cell.height(20) > cell.height(120),
            "narrow width should wrap into more visual lines"
        );
    }

    #[test]
    fn table_rows_fit_width_and_stay_aligned() {
        // 宽度感知路径：长单元格在列内换行，管道列位置在所有行（含折行
        // 子行）保持一致，且不超出面板宽度。
        let md = "| Name | Description |\n| --- | --- |\n| alpha | a very long description that exceeds the width and must wrap inside the table |";
        let cell = MarkdownCell::new(md, &dark());
        let width = 40u16;
        let height = cell.height(width);
        assert!(height > 3, "long cell should wrap: {height}");

        let text = render_text(&cell, width);

        let pipe_cols = |row: &str| -> Vec<usize> {
            let mut cols = Vec::new();
            let mut col = 0;
            for ch in row.chars() {
                if ch == '|' {
                    cols.push(col);
                }
                col += unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
            }
            cols
        };
        let rows: Vec<&str> = text.lines().filter(|l| l.contains('|')).collect();
        assert!(!rows.is_empty(), "expected table rows:\n{text}");
        let cols: Vec<Vec<usize>> = rows.iter().map(|r| pipe_cols(r)).collect();
        assert!(
            cols.windows(2).all(|w| w[0] == w[1]),
            "pipe columns misaligned:\n{text}"
        );
        assert!(
            rows.iter()
                .all(|r| unicode_width::UnicodeWidthStr::width(*r) <= width as usize),
            "no row may exceed the panel width:\n{text}"
        );
    }

    #[test]
    fn render_partial_skips_leading_lines() {
        let cell = MarkdownCell::new("# Head\n\nline two\n\nline three", &dark());
        let height = cell.height(80);
        assert!(height >= 3, "{height}");

        let backend = TestBackend::new(80, height.saturating_sub(2));
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| cell.render_partial(frame.area(), frame.buffer_mut(), 2))
            .expect("draw");
        let text = buffer_text(terminal.backend().buffer());
        assert!(text.contains("line three"), "{text}");
        assert!(!text.contains("Head"), "{text}");
    }

    #[test]
    fn indent_narrows_the_layout_width() {
        let long = "word ".repeat(60);
        let plain = MarkdownCell::new(long.clone(), &dark());
        let indented = MarkdownCell::new(long, &dark()).with_indent(4);
        // Content width is 4 columns narrower, so the indented cell wraps to
        // strictly more visual lines at the same panel width.
        assert!(
            indented.height(40) > plain.height(40),
            "indent must shrink the layout width"
        );
    }

    #[test]
    fn indent_height_matches_rendered_rows() {
        // Regression: `render_partial` used to indent the area and then pass
        // the already-narrowed width into the width-aware renderer, shrinking
        // the layout width twice. The drawn rows then exceeded `height()`
        // and got clipped by the column renderer.
        let long = "word ".repeat(60);
        let cell = MarkdownCell::new(long, &dark()).with_indent(4);
        let width = 40u16;
        let height = cell.height(width);
        assert!(height >= 5, "expected a multi-line layout, got {height}");

        let text = render_viewport(&cell, width, height);
        let non_empty = text.lines().filter(|l| !l.trim().is_empty()).count();
        assert_eq!(
            non_empty, height as usize,
            "every reported row must be drawn:\n{text}"
        );
    }

    #[test]
    fn re_layouts_when_width_changes() {
        let cell = MarkdownCell::new(
            "| A | B |\n| --- | --- |\n| short | https://example.com/a/very/long/url/that/must/wrap |",
            &dark(),
        );
        let narrow = cell.height(30);
        let wide = cell.height(60);
        assert!(narrow >= wide, "narrower panel needs more rows");
        // Rendering at the new width must not reuse the stale cache.
        let text = render_text(&cell, 60);
        assert!(
            text.lines()
                .filter(|l| l.contains('|'))
                .all(|l| unicode_width::UnicodeWidthStr::width(l) <= 60),
            "{text}"
        );
    }

    #[test]
    fn table_cell_with_pipe_stays_aligned_end_to_end() {
        // 回归：转义管道 `\|` 在整条管线（pulldown → format_table）中保持为
        // 单元格数据，不再被二次拆列。
        // 注意：行内代码里的管道（| x | `a|b` |）是 pulldown-cmark 的上游
        // 限制——它在单元格层面就按未转义管道切分表格行，本仓库无法修复；
        // `format_table_keeps_pipe_inside_cell` 保证的是 format_table 层收到
        // 结构化单元格后不再拆列。
        let md = "| A | B |\n| --- | --- |\n| x | `plain` |\n| y | esc\\|pipe |";
        let cell = MarkdownCell::new(md, &dark());
        let width = 60u16;
        let text = render_text(&cell, width);

        let rows: Vec<&str> = text.lines().filter(|l| l.contains('|')).collect();
        assert!(!rows.is_empty(), "expected table rows:\n{text}");
        // The header row keeps exactly 3 structural pipes.
        assert_eq!(rows[0].matches('|').count(), 3, "header pipes: {}", rows[0]);
        // The escaped pipe survives intact inside column B; inline code in a
        // cell renders without backticks.
        assert!(text.contains("esc|pipe"), "{text}");
        assert!(text.contains("plain"), "{text}");
        // Uniform display width across rows, with the structural pipes (the
        // A/B boundary and the trailing edge) aligned. Pipes inside cell
        // data are allowed between them.
        let widths: Vec<usize> = rows
            .iter()
            .map(|r| unicode_width::UnicodeWidthStr::width(*r))
            .collect();
        assert!(
            widths.windows(2).all(|w| w[0] == w[1]),
            "row display widths differ: {widths:?}\n{text}"
        );
        let pipe_cols = |row: &str| -> Vec<usize> {
            let mut cols = Vec::new();
            let mut col = 0;
            for ch in row.chars() {
                if ch == '|' {
                    cols.push(col);
                }
                col += unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
            }
            cols
        };
        let sep: Vec<Vec<usize>> = rows.iter().map(|r| pipe_cols(r)).collect();
        assert!(
            sep.iter()
                .all(|c| c.first() == sep[0].first() && c.last() == sep[0].last()),
            "structural pipes must align:\n{text}"
        );
    }

    #[test]
    fn wide_table_chunks_into_fitting_blocks() {
        // 回归：10 列表格在 40 列面板里，旧实现 shrink 到 MIN_COL_WIDTH=8
        // 地板后整行仍 111 列宽，外层 wrap_line 把表格行拦腰折断导致管道错乱。
        // 现在列会被拆成多个能放下的块（每块重复表头），所有行都不超宽。
        let md = "| c1 | c2 | c3 | c4 | c5 | c6 | c7 | c8 | c9 | c10 |\n| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |\n| a1 | b1 | c1 | d1 | e1 | f1 | g1 | h1 | i1 | j1 |";
        let cell = MarkdownCell::new(md, &dark());
        let width = 40u16;
        let text = render_text(&cell, width);

        let rows: Vec<&str> = text.lines().filter(|l| l.contains('|')).collect();
        assert!(!rows.is_empty(), "expected table rows:\n{text}");
        for r in &rows {
            assert!(
                unicode_width::UnicodeWidthStr::width(*r) <= width as usize,
                "row must never exceed the panel width (was shredded): {r}\n{text}"
            );
        }
        // The header repeats per chunk: chunk 1 carries c1..c7, chunk 2 c8..c10.
        assert!(
            text.contains("| c1 | c2 | c3") && text.contains("| c8 | c9 | c10"),
            "each chunk must repeat the header:\n{text}"
        );
        assert!(text.contains("a1") && text.contains("j1"), "{text}");
    }

    /// 肉眼查看渲染效果的 demo：跑
    /// `cargo test -p tui --lib cells::markdown::tests::demo_render_output -- --nocapture`
    #[test]
    fn demo_render_output() {
        let md = "\
# Title

一段包含 **bold**、*italic*、`inline code` 和 [链接](https://example.com) 的段落。

## 列表

- item one
- item two
  1. nested ordered

## 表格

| Name | Description |
| --- | --- |
| alpha | a very long description that exceeds the width and must wrap |
| beta | short |

## 代码块

```rust
fn main() {
    println!(\"hi\");
}
```

> blockquote 引用
";
        let cell = MarkdownCell::new(md, &dark());
        let width = 60u16;
        let text = render_text(&cell, width);
        println!(
            "=== MarkdownCell render @ {width} cols (height {}) ===",
            cell.height(width)
        );
        for (i, line) in text.lines().enumerate() {
            println!("{:>3}|{}|", i, line.trim_end());
        }
    }
}

#[cfg(test)]
mod integration_tests {
    use tact_protocol::AgentUpdate;

    use crate::{
        render::{
            render_md::format_table_lines,
            test_harness::{make_app, render_log_panel_text},
        },
        widgets::state::RawMessageType,
    };

    #[test]
    fn assistant_history_mermaid_uses_width_aware_cell() {
        let mut app = make_app();
        let md = "```mermaid\nflowchart TD\n  A[This is a long start node] --> B[This is a long end node]\n```";
        app.load_history(vec![tact_llm::Message::new_text(
            tact_llm::Role::Assistant,
            md,
        )]);

        assert_eq!(app.messages.len(), 1);
        assert!(
            app.markdown_cells[0].is_some(),
            "assistant history should use the width-aware MarkdownCell"
        );

        let text = render_log_panel_text(&mut app, 40, 20);
        assert!(text.contains('─') || text.contains('│'), "{text}");
        assert!(!text.contains("flowchart TD"), "raw Mermaid leaked: {text}");
    }

    #[test]
    fn md_info_renders_as_one_markdown_cell() {
        let mut app = make_app();
        let md = "# Title\n\n- item one\n- item two\n\n```rust\nfn hi() {}\n```\n";
        app.handle_agent_update(AgentUpdate::MdInfo(md.into()));

        let text = render_log_panel_text(&mut app, 80, 20);
        assert!(text.contains("Title"), "{text}");
        assert!(
            text.contains("item one") && text.contains("item two"),
            "{text}"
        );
        assert!(text.contains("fn hi() {}"), "{text}");
        // One physical message: the markdown is a single cell.
        assert_eq!(
            app.messages.len(),
            1,
            "MdInfo must append exactly one message"
        );
        assert!(app.markdown_cells[0].is_some());
        assert_eq!(app.raw_messages[0], md);
    }

    #[test]
    fn md_info_cell_is_followed_by_normal_message_without_layout_shift() {
        let mut app = make_app();
        app.handle_agent_update(AgentUpdate::MdInfo("# Head\n\nline two\n".into()));
        // A normal message after the markdown cell.
        app.append_msg(
            ratatui::text::Line::from("after markdown"),
            "after markdown".into(),
            RawMessageType::LLM,
        );

        let text = render_log_panel_text(&mut app, 40, 20);
        assert!(text.contains("after markdown"), "{text}");
        assert!(text.contains("Head"), "{text}");
    }

    #[test]
    fn md_info_does_not_break_scroll_positions_of_following_rows() {
        let mut app = make_app();
        // Markdown block whose height is stable across renders.
        app.handle_agent_update(AgentUpdate::MdInfo("# A\n\npara\n".into()));
        app.append_msg(
            ratatui::text::Line::from("tail line"),
            "tail line".into(),
            RawMessageType::LLM,
        );

        // Render twice: the prefix-sum cache must stay consistent so the
        // tail line remains at the same visual offset.
        let t1 = render_log_panel_text(&mut app, 40, 20);
        let t2 = render_log_panel_text(&mut app, 40, 20);
        assert!(t1.contains("tail line"), "{t1}");
        assert!(t2.contains("tail line"), "{t2}");
        let pos1 = t1.lines().position(|l| l.contains("tail line"));
        let pos2 = t2.lines().position(|l| l.contains("tail line"));
        assert_eq!(pos1, pos2, "tail line must not drift: {t1:?} vs {t2:?}");
    }

    #[test]
    fn md_info_skips_mouse_selection() {
        let mut app = make_app();
        app.handle_agent_update(AgentUpdate::MdInfo("# Title\n".into()));
        // Force a selection range over the markdown row.
        app.mouse.log_selection = Some(crate::widgets::state::LogSelection::new(
            crate::widgets::state::TextPosition {
                phys_idx: 0,
                byte_offset: 0,
            },
            crate::widgets::state::TextPosition {
                phys_idx: 0,
                byte_offset: 10,
            },
        ));

        let text = render_log_panel_text(&mut app, 80, 10);
        assert!(
            !text.contains('\u{7f}'),
            "markdown cell must not draw selection overlay (reversed), got:\n{text}"
        );
    }

    #[test]
    fn streamed_table_rows_stay_aligned_after_reply_indent() {
        // 回归：流式表格按 log_scroll.width（含缩进的全内容宽度）布局，
        // 但渲染时 assistant 行缩进 LOG_THINKING_INDENT + 1 = 3 列，实际
        // 可用宽度少 3 —— 长表格行尾 pipe 被裁掉、列看起来错位。
        // `table_layout_width` 在布局时扣掉缩进，表格行永不超渲染宽度。
        use crate::render::test_harness::render_log_panel_terminal;
        use crate::widgets::state::RawMessageType;

        let md = "| 编号 | 问题描述 | 影响范围 | 处理建议 |\n|-----:|:---------|:---------|:---------|\n| 1 | 当用户连续快速点击「保存」按钮超过五次时，系统会偶发出现重复提交，导致数据库中产生两条内容完全一致但主键不同的记录 | 涉及所有使用表单保存功能的页面，包括用户管理、订单管理、商品管理、配置管理四个模块 | 在前端增加防抖与提交锁，后端在事务中增加唯一性约束校验，并对历史重复数据执行清理脚本 |";

        for width in [40u16, 60, 80] {
            let mut app = make_app();
            // First render sets log_scroll.width, as in real usage before
            // streaming table rows arrive.
            let _first = render_log_panel_terminal(&mut app, width, 5);
            let (styled, raw) = format_table_lines(
                &md.lines().map(|s| s.to_string()).collect::<Vec<_>>(),
                &app.theme,
                Some(app.table_layout_width()),
            );
            for (s, r) in styled.into_iter().zip(raw) {
                app.append_msg(s, r, RawMessageType::LLM);
            }
            let height = app.messages.len() as u16 + 2;
            let terminal = render_log_panel_terminal(&mut app, width, height);
            let buf = terminal.backend().buffer();

            // Group pipe cells by row using real buffer coordinates.
            let mut rows: Vec<Vec<u16>> = Vec::new();
            for y in 0..buf.area.height {
                let xs: Vec<u16> = (0..buf.area.width)
                    .filter(|&x| buf[(x, y)].symbol() == "|")
                    .collect();
                if !xs.is_empty() {
                    rows.push(xs);
                }
            }
            assert!(!rows.is_empty(), "expected table rows at width {width}");
            // Rows sharing a pipe pattern (same block / column count) must
            // have identical pipe columns.
            for w in rows.windows(2) {
                if w[0].len() == w[1].len() {
                    assert_eq!(w[0], w[1], "same-block pipes misaligned at width {width}");
                }
            }
            // Every table row keeps its trailing pipe (no right clipping).
            assert!(
                rows.iter().all(|xs| xs.len() >= 2),
                "trailing pipe clipped at width {width}: {rows:?}"
            );
            // Every pipe column must sit inside the rendered content area
            // (panel width minus right border).
            for xs in &rows {
                for &x in xs {
                    assert!(x < width, "pipe beyond panel at width {width}: {rows:?}");
                }
            }
        }
    }
    // appended below integration_tests
}
