//! Subagent popup: full conversation during run and after completion, organized
//! into three stacked sections — **🧠 Thinking**, **🔧 Tools**, **📄 Context**.
//!
//! The forwarder (`crates/tact/src/tool/subagent_ui.rs`) tags every
//! `ToolProgress` chunk with its [`SubagentSection`]; `ToolOutputBuffer`
//! accumulates the structured blocks alongside the flat card stream, and the
//! live→completed handoff stores them in `ToolRenderOutput::detail_sections`.
//! This popup groups those blocks in canonical order (Thinking → Tools →
//! Context), prepends the subagent prompt (the card's `arg_full`) to Context,
//! and renders one scrollable document: plain wrapped lines while live,
//! Markdown through the same width-aware pipeline as the main area and the
//! thinking popup (`render_markdown_with_tables`), with shared heading
//! markers / code-tail fills / ordered-list spacers from [`super::markdown_plan`]
//! (with a one-shot cache in the popup struct). Layout is always driven by the
//! text actually shown so styles and grapheme positions stay in sync.
//!
//! Transcripts that predate section capture (no `detail_sections`) fall back
//! to the flat `detail_full` as a single Context section and render exactly
//! like the old flat popup.
//!
//! `prepare_subagent_popup` rebuilds the layout cache (side effect on the
//! popup state); `render_subagent_popup` only reads it.

use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Scrollbar, ScrollbarState},
};

use super::{PopupMouseSurface, markdown_plan};
use crate::{
    protocol::{SubagentSection, SubagentSectionBlock, THINKING_SECTION_HEADER},
    render::{
        ctx::RenderCtx,
        render_md::render_markdown_with_tables,
        selectable_text::{MarkdownDisplayRow, PopupLayoutCache},
    },
    state::{SubagentPopup, ToolState},
    theme::Theme,
};

/// Canonical section header for the sectioned popup.
fn section_header(section: SubagentSection) -> &'static str {
    match section {
        SubagentSection::Thinking => "🧠 Thinking",
        SubagentSection::Tool => "🔧 Tools",
        SubagentSection::Context => "📄 Context",
    }
}

/// Left indent for the Thinking section body.
const THINKING_BODY_INDENT: usize = 4;

/// Indent every non-empty line of a thinking body by [`THINKING_BODY_INDENT`]
/// columns.
///
/// Live mode uses plain spaces. Completed mode uses non-breaking spaces
/// (`\u{00A0}`): four leading ASCII spaces would make CommonMark treat the
/// paragraph as an indented code block (verified against the kit's pulldown
/// renderer), while NBSP is not indentation per the spec and renders as a
/// normal-width space. Wrapped continuation rows start at column 0.
fn indent_thinking_lines(text: &str, markdown_safe: bool) -> String {
    let pad = if markdown_safe {
        "\u{00A0}".repeat(THINKING_BODY_INDENT)
    } else {
        " ".repeat(THINKING_BODY_INDENT)
    };
    text.lines()
        .map(|line| {
            if line.is_empty() {
                line.to_string()
            } else {
                format!("{pad}{line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Strip the shared `🧠 Thinking` marker the forwarder prefixes to each
/// thinking block in the flat card stream. The sectioned popup renders one
/// header per section instead, so the marker must not appear in the body.
fn strip_thinking_marker(text: &str) -> String {
    let text = text.trim_start_matches('\n');
    match text.strip_prefix(THINKING_SECTION_HEADER) {
        Some(rest) => rest.trim_start_matches('\n').to_string(),
        None => text.to_string(),
    }
}

/// Group raw section blocks in canonical order (Thinking → Tools → Context),
/// skipping empty sections and trimming outer newlines. Returns the grouped
/// bodies and whether section headers should be shown (only when at least two
/// sections are non-empty, so a run that only streamed text keeps today's
/// flat look).
fn group_sections(blocks: &[SubagentSectionBlock]) -> (Vec<(SubagentSection, Vec<String>)>, bool) {
    let mut grouped: Vec<(SubagentSection, Vec<String>)> = Vec::new();
    for section in SubagentSection::ORDERED {
        let mut texts: Vec<String> = Vec::new();
        for block in blocks.iter().filter(|b| b.section == section) {
            let text = if section == SubagentSection::Thinking {
                strip_thinking_marker(&block.text)
            } else {
                block.text.clone()
            };
            let text = text.trim().to_string();
            if !text.is_empty() {
                texts.push(text);
            }
        }
        if !texts.is_empty() {
            grouped.push((section, texts));
        }
    }
    let with_headers = grouped.len() >= 2;
    (grouped, with_headers)
}

/// Live document: plain wrapped lines with styled section headers and a bold
/// `Prompt:` label at the top of Context. Returns one styled line per display
/// line so the plan step can map styles 1:1.
fn build_live_document(
    grouped: &[(SubagentSection, Vec<String>)],
    prompt: Option<&str>,
    theme: &Theme,
) -> (Vec<Line<'static>>, String) {
    let with_headers = grouped.len() >= 2;
    let header_style = Style::default()
        .fg(theme.heading)
        .add_modifier(Modifier::BOLD);
    let label_style = Style::default().fg(theme.fg).add_modifier(Modifier::BOLD);
    let mut styled = Vec::new();
    let mut text = Vec::new();

    for (section, blocks) in grouped {
        if with_headers {
            styled.push(Line::from(Span::styled(
                section_header(*section),
                header_style,
            )));
            text.push(section_header(*section).to_string());
            styled.push(Line::from(""));
            text.push(String::new());
        }
        if *section == SubagentSection::Context
            && let Some(prompt) = prompt.filter(|p| !p.is_empty())
        {
            styled.push(Line::from(Span::styled("Prompt:", label_style)));
            text.push("Prompt:".to_string());
            for line in prompt.lines() {
                styled.push(Line::from(line.to_string()));
                text.push(line.to_string());
            }
            styled.push(Line::from(""));
            text.push(String::new());
        }
        for block in blocks {
            let block_text = if *section == SubagentSection::Thinking {
                indent_thinking_lines(block, false)
            } else {
                block.clone()
            };
            for line in block_text.lines() {
                styled.push(Line::from(line.to_string()));
                text.push(line.to_string());
            }
            styled.push(Line::from(""));
            text.push(String::new());
        }
    }
    (styled, text.join("\n"))
}

/// Completed document source: Markdown with `## <header>` per section and a
/// bold `Prompt:` label at the top of Context.
fn build_completed_markdown(
    grouped: &[(SubagentSection, Vec<String>)],
    prompt: Option<&str>,
) -> String {
    let with_headers = grouped.len() >= 2;
    let mut md = String::new();
    for (section, blocks) in grouped {
        if with_headers {
            md.push_str(&format!("## {}\n\n", section_header(*section)));
        }
        if *section == SubagentSection::Context
            && let Some(prompt) = prompt.filter(|p| !p.is_empty())
        {
            md.push_str(&format!("**Prompt:**\n\n{prompt}\n\n"));
        }
        for block in blocks {
            if *section == SubagentSection::Thinking {
                md.push_str(&indent_thinking_lines(block, true));
            } else {
                md.push_str(block);
            }
            md.push_str("\n\n");
        }
    }
    md
}

/// Rebuild the popup's layout cache when missing or stale (content grew,
/// width changed, or a live→completed transition). Side-effect phase.
pub fn prepare_subagent_popup(
    popup: &mut SubagentPopup,
    tools: &ToolState,
    theme: &Theme,
    body_width: u16,
) {
    let tool_id = &popup.tool_id;
    let is_live = tools.active.iter().any(|a| a.tool_id == *tool_id);
    // Cheap fingerprint (byte length) so we can skip the clone + full re-wrap
    // when neither the content nor the body width changed. Sections and the
    // flat text grow in lockstep, so the flat length doubles as the section
    // fingerprint.
    let (sections, prompt, content_len) = if is_live {
        let Some(active) = tools.active.iter().find(|a| a.tool_id == *tool_id) else {
            return;
        };
        (
            active.live_output.sections().to_vec(),
            Some(active.output.arg_full.clone()),
            active.live_output.full_detail_len(),
        )
    } else {
        let Some(block) = tools.blocks.iter().find(|b| b.tool_id == *tool_id) else {
            return;
        };
        // Legacy transcript (predates section capture): one flat Context block
        // with no prompt label, so the popup renders exactly like before.
        let (sections, prompt, content_len) = match block.output.detail_sections.as_deref() {
            Some(sections) if !sections.is_empty() => {
                let content_len = sections.iter().map(|b| b.text.len()).sum();
                (
                    sections.to_vec(),
                    Some(block.output.arg_full.clone()),
                    content_len,
                )
            }
            _ => {
                let flat = block
                    .output
                    .detail_full
                    .as_ref()
                    .cloned()
                    .unwrap_or_default();
                let content_len = flat.len();
                let sections = if flat.is_empty() {
                    Vec::new()
                } else {
                    vec![SubagentSectionBlock {
                        section: SubagentSection::Context,
                        text: flat,
                    }]
                };
                (sections, None, content_len)
            }
        };
        (sections, prompt, content_len)
    };
    if content_len == 0 {
        return;
    }

    let cache_valid = popup
        .layout_cache
        .as_ref()
        .is_some_and(|c| c.is_valid(is_live, content_len, body_width));
    if cache_valid {
        return;
    }

    let (grouped, _with_headers) = group_sections(&sections);
    if grouped.is_empty() {
        return;
    }
    let prompt = prompt.as_deref();

    // Live: plain lines. Completed: width-aware markdown render + shared
    // popup decoration (heading `#` markers). Layout must use the rendered
    // line text (not the markdown source), matching thinking_popup, otherwise
    // style spans and grapheme positions drift apart.
    let (styled_lines, display_text) = if is_live {
        build_live_document(&grouped, prompt, theme)
    } else if let Some(cached) = popup.cached_markdown.clone() {
        let display_text = cached
            .iter()
            .map(markdown_plan::line_text)
            .collect::<Vec<_>>()
            .join("\n");
        (cached, display_text)
    } else {
        // Re-render at the actual popup body width: the width-aware pipeline
        // shrinks pipe-table columns, wraps long cells inside the table and
        // routes Mermaid at this width (same as the main area / thinking
        // popup), instead of the card's fixed 80-column render.
        let source = build_completed_markdown(&grouped, prompt);
        let (mut styled, _raw) =
            render_markdown_with_tables(&source, theme, Some(body_width as usize));
        markdown_plan::decorate_headings(&mut styled, theme);
        let display_text = styled
            .iter()
            .map(markdown_plan::line_text)
            .collect::<Vec<_>>()
            .join("\n");
        popup.cached_markdown = Some(styled.clone());
        (styled, display_text)
    };

    if display_text.is_empty() {
        return;
    }

    let fallback = Style::default().fg(theme.fg).bg(theme.bg);
    let code_bg = theme.code_block_bg();
    let display_rows = markdown_plan::plan_markdown_display(
        &styled_lines,
        &display_text,
        fallback,
        code_bg,
        body_width as usize,
    );
    let line_count = display_text.lines().count();

    popup.layout_cache = Some(PopupLayoutCache {
        is_live,
        content_len,
        width: body_width,
        raw_text: display_text,
        display_rows,
        line_count,
    });
}

pub fn render_subagent_popup(frame: &mut Frame, area: Rect, ctx: &RenderCtx) -> PopupMouseSurface {
    let mut surface = PopupMouseSurface::default();
    let Some((scroll, selection, title)) = ctx
        .subagent_popup
        .map(|p| (p.scroll, p.selection, p.title.clone()))
    else {
        return surface;
    };

    let popup = match ctx.subagent_popup {
        Some(p) => p,
        None => return surface,
    };
    let is_live = ctx.tools.active.iter().any(|a| a.tool_id == popup.tool_id);

    let popup_area = super::centered_popup_area(area);
    let body_area = super::popup_inner(popup_area);

    let Some(cache) = popup.layout_cache.as_ref() else {
        return surface;
    };
    if !cache.is_valid(is_live, cache.content_len, body_area.width) {
        return surface;
    }
    let display_rows = &cache.display_rows;
    let raw_text = &cache.raw_text;

    let total = display_rows.len();
    let content_height = body_area.height as usize;
    let max_scroll = total.saturating_sub(content_height);
    let scroll = (scroll as usize).min(max_scroll);

    let header = if is_live {
        format!(" {} (live, {} lines) ", title, cache.line_count)
    } else {
        format!(" {} ({} lines) ", title, cache.line_count)
    };

    let footer: &[super::FooterHint] = &[
        super::FooterHint {
            key: "y",
            label: " copy ",
        },
        super::FooterHint {
            key: "Esc",
            label: " close ",
        },
        super::FooterHint {
            key: "j/k",
            label: " scroll ",
        },
    ];
    let inner = super::render_popup_chrome(frame, popup_area, ctx.theme, &header, Some(footer));
    let body_area = inner;

    let selection_range = selection.and_then(|sel| sel.normalized_non_empty(raw_text));
    let code_bg = ctx.theme.code_block_bg();

    let mut hit_rows = Vec::new();
    for (visible_row, display) in display_rows
        .iter()
        .skip(scroll)
        .take(content_height)
        .enumerate()
    {
        let screen_y = body_area.y.saturating_add(visible_row as u16);
        let (line, hit_row) = match display {
            MarkdownDisplayRow::Code(display) => {
                let mut line = Line::from(display.spans(selection_range.as_ref()));
                markdown_plan::fill_code_row_tail(&mut line, display, body_area.width, code_bg);
                (line, display.hit_row(screen_y, body_area.x))
            }
            MarkdownDisplayRow::Content(display) => (
                Line::from(display.spans(selection_range.as_ref())),
                display.hit_row(screen_y, body_area.x),
            ),
            MarkdownDisplayRow::Spacer => continue,
        };
        frame.render_widget(
            Paragraph::new(line),
            Rect::new(body_area.x, screen_y, body_area.width, 1),
        );
        hit_rows.push(hit_row);
    }

    let scrollbar =
        Scrollbar::default().orientation(ratatui::widgets::ScrollbarOrientation::VerticalRight);
    let mut state = ScrollbarState::new(total)
        .viewport_content_length(content_height)
        .position(scroll);
    frame.render_stateful_widget(scrollbar, popup_area, &mut state);

    surface.subagent_popup_area = popup_area;
    surface.body_area = body_area;
    surface.hit_rows = hit_rows;
    surface
}
