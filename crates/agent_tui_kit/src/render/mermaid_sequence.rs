//! Terminal rendering for Mermaid `sequenceDiagram` blocks.
//!
//! Tact renders sequence diagrams with its own renderer instead of the
//! upstream `ratatui-markdown` one because the upstream sequence renderer
//! has three problems that produce visibly broken diagrams:
//!
//! 1. `participant A as 名称` aliases are not parsed — the whole declaration
//!    string ends up as the participant name in the header.
//! 2. The `+`/`-` activation shorthand on arrow endpoints (`A->>+B`) is not
//!    understood, so `+B`/`-B` are auto-added as phantom participant columns.
//! 3. Arrow labels are placed by display column but only the start cell is
//!    checked, so a 2-column CJK glyph can overwrite a lifeline (or be
//!    dropped when its start cell is a lifeline), making labelled arrows
//!    look misaligned.
//!
//! This renderer also clears the continuation cell of each width-2 glyph and
//! reflows label characters past lifelines, so long / CJK labels neither
//! inflate row display width nor silently drop letters on `│` columns.
//!
//! Other diagram types still go through `ratatui-markdown`; only
//! `sequenceDiagram` fences are routed here (see `render_mermaid_block`).

use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::theme::Theme;

const HLINE: char = '─';
const DLINE: char = '╌';
const VLINE: char = '│';
const CORNER_TL: char = '┌';
const CORNER_TR: char = '┐';
const CORNER_BL: char = '└';
const CORNER_BR: char = '┘';
/// Horizontal cells between the lifeline and the self-loop corner.
const SELF_LOOP_ARM: usize = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArrowKind {
    /// `->`
    Solid,
    /// `-->`
    Dotted,
    /// `->>`
    SolidOpen,
    /// `-->>`
    DottedOpen,
}

#[derive(Debug, Clone)]
struct Participant {
    id: String,
    label: String,
}

#[derive(Debug, Clone)]
struct Message {
    from: String,
    to: String,
    text: String,
    kind: ArrowKind,
}

#[derive(Debug)]
struct Diagram {
    participants: Vec<Participant>,
    messages: Vec<Message>,
}

/// Parse a `sequenceDiagram` block.
///
/// Returns `None` when no participant could be found so callers can fall back
/// to ordinary code rendering.
fn parse(source: &str) -> Option<Diagram> {
    let mut participants: Vec<Participant> = Vec::new();
    let mut messages: Vec<Message> = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for raw in source.lines() {
        let line = raw.trim();
        if line.is_empty() || line == "sequenceDiagram" {
            continue;
        }

        if let Some(rest) = line
            .strip_prefix("participant ")
            .or_else(|| line.strip_prefix("actor "))
        {
            // `participant A as 用户` → id `A`, header label `用户`.
            let (id, label) = match rest.split_once(" as ") {
                Some((id, label)) => (id.trim(), label.trim()),
                None => (rest.trim(), ""),
            };
            if !id.is_empty() {
                let label = if label.is_empty() { id } else { label };
                ensure_participant(&mut participants, &mut seen, id, label);
            }
            continue;
        }

        if let Some(msg) = parse_message(line) {
            ensure_participant(&mut participants, &mut seen, &msg.from, &msg.from);
            ensure_participant(&mut participants, &mut seen, &msg.to, &msg.to);
            messages.push(msg);
        }
    }

    if participants.is_empty() {
        return None;
    }
    Some(Diagram {
        participants,
        messages,
    })
}

fn ensure_participant(
    participants: &mut Vec<Participant>,
    seen: &mut std::collections::HashSet<String>,
    id: &str,
    label: &str,
) {
    if seen.insert(id.to_string()) {
        participants.push(Participant {
            id: id.to_string(),
            label: label.to_string(),
        });
    }
}

fn parse_message(line: &str) -> Option<Message> {
    for arrow in ["-->>", "->>", "-->", "->"] {
        if let Some(idx) = line.find(arrow) {
            let kind = match arrow {
                "-->>" => ArrowKind::DottedOpen,
                "->>" => ArrowKind::SolidOpen,
                "-->" => ArrowKind::Dotted,
                _ => ArrowKind::Solid,
            };
            let from = strip_activation(&line[..idx]);
            let to_and_text = line[idx + arrow.len()..].trim();
            let (to, text) = match to_and_text.find(':') {
                Some(p) => (
                    strip_activation(&to_and_text[..p]),
                    to_and_text[p + 1..].trim().to_string(),
                ),
                None => (strip_activation(to_and_text), String::new()),
            };
            if from.is_empty() || to.is_empty() {
                return None;
            }
            return Some(Message {
                from,
                to,
                text,
                kind,
            });
        }
    }
    None
}

/// Strip Mermaid's `+`/`-` activation shorthand from an arrow endpoint
/// (`A->>+B` targets participant `B`, not a participant named `+B`).
fn strip_activation(s: &str) -> String {
    let s = s.trim();
    let s = s
        .strip_prefix('+')
        .or_else(|| s.strip_prefix('-'))
        .unwrap_or(s);
    s.trim().to_string()
}

/// Render a `sequenceDiagram` block as terminal lines, or `None` when the
/// source does not parse into a diagram.
pub fn render_sequence_diagram(
    source: &str,
    max_width: usize,
    theme: &Theme,
) -> Option<Vec<Line<'static>>> {
    let diagram = parse(source)?;
    Some(render(&diagram, max_width, theme))
}

fn render(diagram: &Diagram, max_width: usize, theme: &Theme) -> Vec<Line<'static>> {
    let n = diagram.participants.len();
    let col_width = ((max_width.saturating_sub(2)) / n).clamp(6, 20);
    let line_width = n * col_width + (n - 1);

    let header_style = Style::default()
        .fg(theme.accent)
        .add_modifier(Modifier::BOLD);
    let line_style = Style::default().fg(theme.muted_fg());
    let label_style = Style::default()
        .fg(theme.heading)
        .add_modifier(Modifier::ITALIC);
    let arrow_style = Style::default()
        .fg(theme.accent)
        .add_modifier(Modifier::BOLD);

    let mut lines: Vec<Line<'static>> = Vec::new();

    // Header row: centered participant labels.
    {
        let mut spans: Vec<Span<'static>> = Vec::new();
        for (i, p) in diagram.participants.iter().enumerate() {
            let tw = p.label.width();
            let pad = col_width.saturating_sub(tw);
            let left_pad = pad / 2;
            let right_pad = pad - left_pad;
            spans.push(Span::styled(" ".repeat(left_pad), header_style));
            spans.push(Span::styled(p.label.clone(), header_style));
            spans.push(Span::styled(" ".repeat(right_pad), header_style));
            if i + 1 < n {
                spans.push(Span::styled(" ".to_string(), line_style));
            }
        }
        lines.push(Line::from(spans));
    }

    for msg in &diagram.messages {
        let from_idx = diagram
            .participants
            .iter()
            .position(|p| p.id == msg.from)
            .unwrap_or(0);
        let to_idx = diagram
            .participants
            .iter()
            .position(|p| p.id == msg.to)
            .unwrap_or(0);

        lines.push(Line::from(lifeline_row(n, col_width, line_style)));

        if from_idx == to_idx {
            for row in self_loop_rows(
                n,
                col_width,
                line_width,
                from_idx,
                &msg.text,
                msg.kind,
                line_style,
                label_style,
                arrow_style,
            ) {
                lines.push(Line::from(row));
            }
            continue;
        }

        if !msg.text.is_empty() {
            lines.push(Line::from(label_row(
                n,
                col_width,
                line_width,
                from_idx,
                to_idx,
                &msg.text,
                line_style,
                label_style,
            )));
        }

        lines.push(Line::from(arrow_row(
            n,
            col_width,
            from_idx,
            to_idx,
            msg.kind,
            line_style,
            arrow_style,
        )));
    }

    lines.push(Line::from(lifeline_row(n, col_width, line_style)));
    lines
}

/// Column index of participant `i`'s lifeline.
fn lifeline_x(i: usize, col_width: usize) -> usize {
    i * (col_width + 1) + col_width / 2
}

/// One row of lifelines (`│` at each participant's center column).
fn lifeline_row(n: usize, col_width: usize, style: Style) -> Vec<Span<'static>> {
    let mut spans = Vec::with_capacity(n * (col_width + 1));
    for i in 0..n {
        let center = col_width / 2;
        for c in 0..col_width {
            spans.push(Span::styled(
                if c == center {
                    VLINE.to_string()
                } else {
                    " ".to_string()
                },
                style,
            ));
        }
        if i + 1 < n {
            spans.push(Span::styled(" ".to_string(), style));
        }
    }
    spans
}

/// Row holding the arrow label, centered between the two lifelines.
///
/// Wide (CJK) glyphs are placed by display column. Continuation cells of a
/// width-2 glyph are cleared to empty spans so the row's display width stays
/// aligned with neighbouring lifeline/arrow rows. Glyphs that would land on a
/// lifeline reflow to the next free cell instead of being dropped.
#[allow(clippy::too_many_arguments)]
fn label_row(
    n: usize,
    col_width: usize,
    line_width: usize,
    from_idx: usize,
    to_idx: usize,
    text: &str,
    line_style: Style,
    label_style: Style,
) -> Vec<Span<'static>> {
    let mut spans = lifeline_row(n, col_width, line_style);

    let text_w = text.width();
    let from_x = lifeline_x(from_idx, col_width);
    let to_x = lifeline_x(to_idx, col_width);
    let center = (from_x + to_x) / 2;
    let mut cx = center.saturating_sub(text_w / 2);

    for ch in text.chars() {
        let cw = UnicodeWidthChar::width(ch).unwrap_or(1).max(1);
        let mut placed = false;
        while cx + cw <= line_width {
            let free = (cx..cx + cw).all(|p| spans[p].content == " ");
            if free {
                spans[cx] = Span::styled(ch.to_string(), label_style);
                // Consume the remaining display columns of a wide glyph so a
                // leftover space does not inflate the row.
                for span in spans.iter_mut().take(cx + cw).skip(cx + 1) {
                    *span = Span::styled(String::new(), label_style);
                }
                cx += cw;
                placed = true;
                break;
            }
            cx += 1;
        }
        if !placed {
            break;
        }
    }
    spans
}

/// Two-row U-shaped self-message loop beside the participant lifeline.
///
/// Right side (preferred when there is room):
/// ```text
/// │──┐ label
/// │◀─┘
/// ```
/// Left side (last column / no room on the right):
/// ```text
/// label ┌──│
///      └──▶│
/// ```
#[allow(clippy::too_many_arguments)]
fn self_loop_rows(
    n: usize,
    col_width: usize,
    line_width: usize,
    idx: usize,
    text: &str,
    kind: ArrowKind,
    line_style: Style,
    label_style: Style,
    arrow_style: Style,
) -> Vec<Vec<Span<'static>>> {
    let x = lifeline_x(idx, col_width);
    let arm = SELF_LOOP_ARM.min(col_width.saturating_sub(1).max(1));
    // Prefer the right side (Mermaid-style), but the last participant loops
    // left so the U-shape stays inside the diagram instead of clipping.
    let go_right = if idx + 1 >= n {
        x < arm + 1
    } else {
        x + arm + 1 < line_width
    };
    let line_ch = match kind {
        ArrowKind::Solid | ArrowKind::SolidOpen => HLINE,
        ArrowKind::Dotted | ArrowKind::DottedOpen => DLINE,
    };
    let head = match (go_right, kind) {
        (true, ArrowKind::Solid | ArrowKind::Dotted) => '◄',
        (true, ArrowKind::SolidOpen | ArrowKind::DottedOpen) => '◀',
        (false, ArrowKind::Solid | ArrowKind::Dotted) => '►',
        (false, ArrowKind::SolidOpen | ArrowKind::DottedOpen) => '▶',
    };

    let mut top = lifeline_row(n, col_width, line_style);
    let mut bottom = lifeline_row(n, col_width, line_style);

    if go_right {
        // │──┐
        // │◀─┘
        for i in 1..=arm {
            let cell = x + i;
            if cell < top.len() {
                top[cell] = Span::styled(line_ch.to_string(), arrow_style);
            }
        }
        let corner = x + arm + 1;
        if corner < top.len() {
            top[corner] = Span::styled(CORNER_TR.to_string(), arrow_style);
        }
        if x + 1 < bottom.len() {
            bottom[x + 1] = Span::styled(head.to_string(), arrow_style);
        }
        for i in 2..=arm {
            let cell = x + i;
            if cell < bottom.len() {
                bottom[cell] = Span::styled(line_ch.to_string(), arrow_style);
            }
        }
        if corner < bottom.len() {
            bottom[corner] = Span::styled(CORNER_BR.to_string(), arrow_style);
        }
        if !text.is_empty() {
            place_label_from(&mut top, corner + 2, text, line_width, label_style);
        }
    } else {
        // ┌──│
        // └──▶│
        let corner = x.saturating_sub(arm + 1);
        if corner < top.len() && top[corner].content == " " {
            top[corner] = Span::styled(CORNER_TL.to_string(), arrow_style);
        }
        for i in 1..=arm {
            let cell = x - i;
            if cell < top.len() && cell > corner {
                top[cell] = Span::styled(line_ch.to_string(), arrow_style);
            }
        }
        if corner < bottom.len() && bottom[corner].content == " " {
            bottom[corner] = Span::styled(CORNER_BL.to_string(), arrow_style);
        }
        for i in 2..=arm {
            let cell = x - i;
            if cell < bottom.len() && cell > corner {
                bottom[cell] = Span::styled(line_ch.to_string(), arrow_style);
            }
        }
        if x > 0 {
            bottom[x - 1] = Span::styled(head.to_string(), arrow_style);
        }
        if !text.is_empty() && corner > 1 {
            let label_end = corner.saturating_sub(1);
            place_label_end_at(&mut top, label_end, text, label_style);
        }
    }

    vec![top, bottom]
}

/// Place `text` starting at `start`, skipping occupied cells, without
/// overwriting lifelines or loop artwork.
fn place_label_from(
    spans: &mut [Span<'static>],
    start: usize,
    text: &str,
    line_width: usize,
    label_style: Style,
) {
    let mut cx = start;
    for ch in text.chars() {
        let cw = UnicodeWidthChar::width(ch).unwrap_or(1).max(1);
        let mut placed = false;
        while cx + cw <= line_width && cx < spans.len() {
            let free = (cx..cx + cw).all(|p| p < spans.len() && spans[p].content == " ");
            if free {
                spans[cx] = Span::styled(ch.to_string(), label_style);
                for span in spans.iter_mut().take(cx + cw).skip(cx + 1) {
                    *span = Span::styled(String::new(), label_style);
                }
                cx += cw;
                placed = true;
                break;
            }
            cx += 1;
        }
        if !placed {
            break;
        }
    }
}

/// Right-align `text` so its last display column ends at `end` (exclusive of
/// occupied cells), used for left-side self-loops.
fn place_label_end_at(spans: &mut [Span<'static>], end: usize, text: &str, label_style: Style) {
    let text_w = text.width();
    let start = end.saturating_sub(text_w);
    let mut cx = start;
    for ch in text.chars() {
        let cw = UnicodeWidthChar::width(ch).unwrap_or(1).max(1);
        if cx + cw > end || cx >= spans.len() {
            break;
        }
        let free = (cx..cx + cw).all(|p| p < spans.len() && spans[p].content == " ");
        if !free {
            break;
        }
        spans[cx] = Span::styled(ch.to_string(), label_style);
        for span in spans.iter_mut().take(cx + cw).skip(cx + 1) {
            *span = Span::styled(String::new(), label_style);
        }
        cx += cw;
    }
}

/// Row holding the arrow between two lifelines.
fn arrow_row(
    n: usize,
    col_width: usize,
    from_idx: usize,
    to_idx: usize,
    kind: ArrowKind,
    line_style: Style,
    arrow_style: Style,
) -> Vec<Span<'static>> {
    let mut spans = lifeline_row(n, col_width, line_style);

    let from_x = lifeline_x(from_idx, col_width);
    let to_x = lifeline_x(to_idx, col_width);

    // Self-messages are rendered by `self_loop_rows`; keep a tiny stub here
    // only as a defensive fallback if a caller still hits this path.
    if from_x == to_x {
        return spans;
    }

    let go_right = to_x > from_x;
    let left_x = from_x.min(to_x);
    let right_x = from_x.max(to_x);

    let line_ch = match kind {
        ArrowKind::Solid | ArrowKind::SolidOpen => HLINE,
        ArrowKind::Dotted | ArrowKind::DottedOpen => DLINE,
    };
    for cell in spans.iter_mut().take(right_x).skip(left_x + 1) {
        if cell.content == " " {
            *cell = Span::styled(line_ch.to_string(), arrow_style);
        }
    }

    let head_x = if go_right { right_x - 1 } else { left_x + 1 };
    let head = match kind {
        ArrowKind::Solid | ArrowKind::Dotted => {
            if go_right {
                '►'
            } else {
                '◄'
            }
        }
        ArrowKind::SolidOpen | ArrowKind::DottedOpen => {
            if go_right {
                '▶'
            } else {
                '◀'
            }
        }
    };
    spans[head_x] = Span::styled(head.to_string(), arrow_style);

    let tail_x = if go_right { left_x + 1 } else { right_x - 1 };
    if spans[tail_x].content == " " {
        spans[tail_x] = Span::styled(if go_right { '>' } else { '<' }.to_string(), arrow_style);
    }

    spans
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;
    use crate::theme::ThemeName;

    fn theme() -> Theme {
        Theme::from(ThemeName::from_str("ink").unwrap())
    }

    /// Compare labels after stripping whitespace and lifeline glyphs so
    /// punched-through `│` characters do not break substring checks.
    fn compact(text: &str) -> String {
        text.chars()
            .filter(|c| !c.is_whitespace() && *c != VLINE)
            .collect()
    }

    fn render_text(source: &str, width: usize) -> String {
        let lines = render_sequence_diagram(source, width, &theme()).expect("valid diagram");
        lines
            .iter()
            .map(Line::to_string)
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn aliases_and_activation_shorthand_render_three_columns() {
        let source = "sequenceDiagram
    participant A as 用户
    participant B as 客户端
    participant C as 服务端
    A->>+B: 发起请求
    B->>+C: 发送请求
    C-->>-B: 返回结果
    B-->>-A: 返回结果";
        let text = render_text(source, 80);

        assert!(text.contains("用户"), "alias missing: {text}");
        assert!(text.contains("客户端"), "alias missing: {text}");
        assert!(text.contains("服务端"), "alias missing: {text}");
        assert!(!text.contains("+B"), "phantom participant leaked: {text}");
        assert!(!text.contains("-B"), "phantom participant leaked: {text}");
        assert!(!text.contains("-A"), "phantom participant leaked: {text}");
        let labels = compact(&text);
        assert!(labels.contains("发起请求"), "label missing: {text}");
        assert!(labels.contains("发送请求"), "label missing: {text}");
        assert!(labels.contains("返回结果"), "label missing: {text}");
        assert!(
            text.contains('▶') || text.contains('►'),
            "arrow missing: {text}"
        );

        // Every non-header row keeps exactly one lifeline per participant.
        for line in text.lines().skip(1) {
            assert_eq!(
                line.matches('│').count(),
                3,
                "expected 3 lifelines, got: {line:?}"
            );
        }
    }

    #[test]
    fn cjk_label_never_overwrites_a_lifeline() {
        let text = render_text("sequenceDiagram\n  Alice->>Bob: 你好世界", 80);

        assert!(compact(&text).contains("你好世界"), "label missing: {text}");
        for line in text.lines().skip(1) {
            assert_eq!(
                line.matches('│').count(),
                2,
                "lifeline overwritten by label: {line:?}"
            );
        }
    }

    #[test]
    fn cjk_label_keeps_same_display_width_as_lifeline_row() {
        // Placing a width-2 glyph must clear the next cell; leaving a ghost
        // space inflates the row and shifts every lifeline to the right.
        let lines =
            render_sequence_diagram("sequenceDiagram\n  Alice->>Bob: 你好世界", 80, &theme())
                .expect("valid diagram");
        let label = lines
            .iter()
            .find(|l| l.to_string().contains('你'))
            .expect("label row");
        let life = lines
            .iter()
            .find(|l| {
                let s = l.to_string();
                s.matches('│').count() == 2 && !s.contains('你')
            })
            .expect("lifeline row");
        let label_w: usize = label.spans.iter().map(|s| s.content.width()).sum();
        let life_w: usize = life.spans.iter().map(|s| s.content.width()).sum();
        assert_eq!(
            label_w,
            life_w,
            "CJK ghost cells inflated label row (label={label_w} life={life_w}):\n{}",
            lines
                .iter()
                .map(Line::to_string)
                .collect::<Vec<_>>()
                .join("\n")
        );
    }

    #[test]
    fn long_ascii_label_is_not_eaten_by_lifelines() {
        // Narrow columns + a long label centered over an arrow will land
        // characters on │ cells; those glyphs must reflow past the lifeline
        // instead of being dropped (e.g. "submitTask" → "ubmitTask").
        let source = "sequenceDiagram
    participant A
    participant B
    participant C
    A->>C: submitTask";
        let text = render_text(source, 48);
        assert!(
            compact(&text).contains("submitTask"),
            "label chars dropped on lifeline: {text}"
        );
        for line in text.lines().skip(1) {
            assert_eq!(
                line.matches('│').count(),
                3,
                "lifeline overwritten: {line:?}"
            );
        }
    }

    #[test]
    fn ascii_label_is_centered_between_lifelines() {
        let text = render_text("sequenceDiagram\n  Alice->>Bob: Hello", 80);

        assert!(text.contains("Hello"), "label missing: {text}");
        for line in text.lines().skip(1) {
            assert_eq!(
                line.matches('│').count(),
                2,
                "lifeline overwritten: {line:?}"
            );
        }
    }

    #[test]
    fn undeclared_message_participants_are_auto_added() {
        let text = render_text("sequenceDiagram\n  Alice->>Bob: Hi", 80);

        assert!(text.contains("Alice"), "participant missing: {text}");
        assert!(text.contains("Bob"), "participant missing: {text}");
        assert!(text.contains("Hi"), "label missing: {text}");
    }

    #[test]
    fn self_message_draws_u_shaped_loop() {
        let text = render_text("sequenceDiagram\n  Alice->>Alice: Loop", 80);

        assert!(
            text.contains("│──┐") || text.contains("┌──│"),
            "missing self-loop top: {text}"
        );
        assert!(
            text.contains("│◀─┘")
                || text.contains("│◄─┘")
                || text.contains("└─▶│")
                || text.contains("└─►│"),
            "missing self-loop return: {text}"
        );
        assert!(compact(&text).contains("Loop"), "label missing: {text}");
        // Label sits beside the loop, not punched through the lifeline.
        assert!(
            !text.contains("Lo│op"),
            "label should not straddle lifeline: {text}"
        );
        for line in text.lines().skip(1) {
            assert_eq!(
                line.matches('│').count(),
                1,
                "lifeline overwritten: {line:?}"
            );
        }
    }

    #[test]
    fn self_message_on_last_participant_loops_left() {
        let source = "sequenceDiagram
    participant A
    participant B
    B->>B: Refresh";
        let text = render_text(source, 40);
        assert!(
            text.contains("┌──│") || text.contains("┌─│"),
            "last column should loop left: {text}"
        );
        assert!(
            text.contains("└─▶│")
                || text.contains("└─►│")
                || text.contains("└──▶│")
                || text.contains("└──►│"),
            "missing left-side return arrow: {text}"
        );
        assert!(compact(&text).contains("Refresh"), "label missing: {text}");
    }

    #[test]
    fn unparseable_source_returns_none() {
        assert!(render_sequence_diagram("sequenceDiagram\n", 80, &theme()).is_none());
        assert!(
            render_sequence_diagram("sequenceDiagram\n  just some prose", 80, &theme()).is_none()
        );
    }
}
