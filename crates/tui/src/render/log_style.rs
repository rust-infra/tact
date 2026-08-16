use std::collections::HashSet;

use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

use super::slash_style::style_user_skill_line;
use crate::{theme::Theme, widgets::state::RawMessageType};

/// Whether `phys_idx` belongs to a user message block (first line or continuation).
pub(crate) fn is_user_message_line(raw_messages: &[String], phys_idx: usize) -> bool {
    let Some(raw) = raw_messages.get(phys_idx) else {
        return false;
    };
    if raw.trim_start().starts_with('💬') {
        return true;
    }
    if !raw.starts_with("  ") || raw.trim().is_empty() {
        return false;
    }
    let mut i = phys_idx;
    while i > 0 {
        i -= 1;
        let prev = raw_messages[i].as_str();
        if prev.is_empty() {
            return false;
        }
        if prev.trim_start().starts_with('💬') {
            return true;
        }
        if prev.starts_with("  ") {
            continue;
        }
        return false;
    }
    false
}

/// Precompute user-line membership for every physical row in one O(n) pass.
///
/// `is_user_message_line` walks back to the block start per row, which is
/// quadratic for a long pasted user block. This mask follows the exact same
/// rules (whitespace-only rows neither are user lines nor break the run) so
/// hot render paths can index it instead of walking.
pub(crate) fn user_line_mask(raw_messages: &[String]) -> Vec<bool> {
    let mut mask = vec![false; raw_messages.len()];
    let mut in_user_block = false;
    for (i, raw) in raw_messages.iter().enumerate() {
        if raw.trim_start().starts_with('💬') {
            in_user_block = true;
            mask[i] = true;
        } else if raw.is_empty() {
            in_user_block = false;
        } else if raw.starts_with("  ") {
            // Continuation line: part of the block only when it has content
            // (whitespace-only lines pass through without ending the block).
            mask[i] = in_user_block && !raw.trim().is_empty();
        } else {
            in_user_block = false;
        }
    }
    mask
}

/// Caller should build `skill_names` once per cache rebuild (`perf-` / `mem-reuse`).
/// `user_prefix_tmpl` / `user_cont_tmpl` are i18n templates like `"💬 {}"`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn restyle_log_line_with_skills(
    stored: &Line,
    raw: &str,
    theme: &Theme,
    msg_type: RawMessageType,
    is_user_line: bool,
    skill_names: &HashSet<&str>,
    user_prefix_tmpl: &str,
    user_cont_tmpl: &str,
) -> Line<'static> {
    if raw.is_empty() {
        return Line::default();
    }

    if is_user_line {
        if let Some(line) =
            style_user_skill_line(raw, skill_names, theme, user_prefix_tmpl, user_cont_tmpl)
        {
            return line;
        }
        return single_span(raw, theme.success);
    }

    if let Some(style) = crate::widgets::state::log_messages::SystemMsgStyle::from_marker(raw) {
        return single_span(raw, style.color(theme));
    }

    if msg_type == RawMessageType::SysTool {
        return single_span(raw, theme.accent);
    }

    // Only fenced-code lines carry the code background; a heading's
    // highlight background must keep its own style instead of being
    // restyled as code.
    if stored
        .spans
        .iter()
        .any(|s| s.style.bg == Some(theme.code_block_bg()))
    {
        return restyle_code_line(stored, theme);
    }

    let line_style = stored.style;
    let spans: Vec<Span<'static>> = stored
        .spans
        .iter()
        .map(|span| {
            let mut style = line_style.patch(span.style);
            // H1 headings used to carry the theme.highlight background here
            // (a leftover from tui-markdown). The pulldown pipeline emits
            // headings without a background and the MarkdownCell path never
            // paints one either; keeping the band only in this path made
            // wrapped headings render as a shadow-like highlight block, so
            // headings are left backgroundless (bold/underlined heading
            // color only).
            style.fg = if style.add_modifier.contains(Modifier::BOLD) {
                // Table headers / emphasis: keep accent so bold stays visible.
                Some(theme.accent)
            } else {
                match style.fg {
                    // Links / headings: theme-aware color, not the hardcoded
                    // palette Blue tui-markdown used to emit.
                    Some(c) if c == theme.heading => Some(theme.heading),
                    Some(Color::Blue) | Some(Color::LightBlue) => Some(theme.heading),
                    Some(Color::Green) => Some(theme.success),
                    Some(Color::Cyan) => Some(theme.accent),
                    _ => Some(theme.fg),
                }
            };
            Span::styled(span.content.to_string(), style)
        })
        .collect();
    Line::from(spans)
}

fn single_span(text: &str, fg: Color) -> Line<'static> {
    Line::from(Span::styled(text.to_string(), Style::default().fg(fg)))
}

fn restyle_code_line(stored: &Line, theme: &Theme) -> Line<'static> {
    let spans: Vec<Span<'static>> = stored
        .spans
        .iter()
        .map(|s| {
            let mut style = s.style;
            style = style.bg(theme.code_block_bg());
            if style.fg.is_none() || style.fg == Some(Color::Rgb(200, 200, 210)) {
                style = style.fg(theme.code_block_fg());
            }
            Span::styled(s.content.to_string(), style)
        })
        .collect();
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::ThemeName;

    fn brutal() -> Theme {
        Theme::from(ThemeName::Brutal)
    }

    fn retro() -> Theme {
        Theme::from(ThemeName::Retro)
    }

    fn stored_plain(text: &str, fg: Color) -> Line<'static> {
        Line::from(Span::styled(text.to_string(), Style::default().fg(fg)))
    }

    fn stored_code(text: &str, theme: &Theme) -> Line<'static> {
        Line::from(Span::styled(
            text.to_string(),
            Style::default()
                .fg(Color::Rgb(200, 200, 210))
                .bg(theme.code_block_bg()),
        ))
    }

    fn restyle_log_line(
        stored: &Line,
        raw: &str,
        theme: &Theme,
        msg_type: RawMessageType,
        is_user_line: bool,
    ) -> Line<'static> {
        let empty = HashSet::new();
        restyle_log_line_with_skills(
            stored,
            raw,
            theme,
            msg_type,
            is_user_line,
            &empty,
            "💬 {}",
            "  {}",
        )
    }

    #[test]
    fn user_first_and_continuation_lines_use_success() {
        let theme = brutal();
        let raw_messages = vec![
            String::new(),
            "💬 hello".to_string(),
            "  continued".to_string(),
        ];

        assert!(is_user_message_line(&raw_messages, 1));
        assert!(is_user_message_line(&raw_messages, 2));

        let first = restyle_log_line(
            &stored_plain("💬 hello", Color::Green),
            "💬 hello",
            &theme,
            RawMessageType::LLM,
            true,
        );
        let cont = restyle_log_line(
            &stored_plain("  continued", Color::Green),
            "  continued",
            &theme,
            RawMessageType::LLM,
            true,
        );
        assert_eq!(first.spans.first().unwrap().style.fg, Some(theme.success));
        assert_eq!(cont.spans.first().unwrap().style.fg, Some(theme.success));
    }

    #[test]
    fn system_prefixes_map_to_semantic_colors() {
        let theme = brutal();

        let ok = restyle_log_line(
            &stored_plain("✓ done", Color::Green),
            "✓ done",
            &theme,
            RawMessageType::LLM,
            false,
        );
        let err = restyle_log_line(
            &stored_plain("✗ failed", Color::Red),
            "✗ failed",
            &theme,
            RawMessageType::LLM,
            false,
        );
        let warn = restyle_log_line(
            &stored_plain("⚠ retry", Color::Yellow),
            "⚠ retry",
            &theme,
            RawMessageType::LLM,
            false,
        );

        let err_x = restyle_log_line(
            &stored_plain("❌ boom", Color::Red),
            "❌ boom",
            &theme,
            RawMessageType::LLM,
            false,
        );
        let ok_badge = restyle_log_line(
            &stored_plain("✅ ok", Color::Green),
            "✅ ok",
            &theme,
            RawMessageType::LLM,
            false,
        );

        assert_eq!(ok.spans.first().unwrap().style.fg, Some(theme.success));
        assert_eq!(err.spans.first().unwrap().style.fg, Some(theme.error));
        assert_eq!(warn.spans.first().unwrap().style.fg, Some(theme.warning));
        assert_eq!(err_x.spans.first().unwrap().style.fg, Some(theme.error));
        assert_eq!(
            ok_badge.spans.first().unwrap().style.fg,
            Some(theme.success)
        );
    }

    #[test]
    fn code_block_restyles_for_light_theme() {
        let theme = brutal();
        let line = restyle_log_line(
            &stored_code("fn main() {}", &theme),
            "fn main() {}",
            &theme,
            RawMessageType::LLM,
            false,
        );
        assert_eq!(
            line.spans.first().unwrap().style.bg,
            Some(theme.code_block_bg())
        );
        assert_eq!(
            line.spans.first().unwrap().style.fg,
            Some(theme.code_block_fg())
        );
    }

    #[test]
    fn heading_keeps_no_background() {
        // The pulldown renderer emits H1 headings without a background; the
        // restyle pass must not paint one (the tui-markdown-era highlight
        // band rendered wrapped headings as a shadow-like block) and must
        // never swap the line for the code-block background either.
        let theme = brutal();
        let stored = Line::from(Span::styled(
            "# Title".to_string(),
            Style::default()
                .fg(theme.heading)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
        ));
        let line = restyle_log_line(&stored, "# Title", &theme, RawMessageType::LLM, false);
        let span = line.spans.first().unwrap();
        assert_eq!(span.style.bg, None, "heading must not gain a background");
        assert_eq!(span.style.fg, Some(theme.accent));
        assert!(
            span.style
                .add_modifier
                .contains(Modifier::BOLD | Modifier::UNDERLINED),
            "heading modifiers must survive restyle"
        );
    }

    #[test]
    fn plain_assistant_text_uses_theme_fg() {
        let theme = retro();
        let line = restyle_log_line(
            &stored_plain("Hello assistant", Color::White),
            "Hello assistant",
            &theme,
            RawMessageType::LLM,
            false,
        );
        assert_eq!(line.spans.first().unwrap().style.fg, Some(theme.fg));
    }

    #[test]
    fn sys_tool_lines_use_accent() {
        let theme = brutal();
        let line = restyle_log_line(
            &stored_plain("  1. bash", Color::Cyan),
            "  1. bash",
            &theme,
            RawMessageType::SysTool,
            false,
        );
        assert_eq!(line.spans.first().unwrap().style.fg, Some(theme.accent));
    }

    #[test]
    fn hardcoded_blue_links_remap_to_theme_heading() {
        // tui-markdown used to emit hardcoded palette Blue for links; the
        // restyle pass must remap it to the theme-aware heading color so
        // links stay visible on every theme.
        let theme = Theme::from(ThemeName::Ink);
        let line = restyle_log_line(
            &stored_plain("a link", Color::Blue),
            "a link",
            &theme,
            RawMessageType::LLM,
            false,
        );
        assert_eq!(
            line.spans.first().unwrap().style.fg,
            Some(theme.heading),
            "blue must remap to {heading:?}",
            heading = theme.heading
        );
    }

    #[test]
    fn unrelated_continuation_is_not_user_line() {
        let raw_messages = vec!["🤖 assistant".to_string(), "  still assistant".to_string()];
        assert!(!is_user_message_line(&raw_messages, 1));
    }

    #[test]
    fn user_line_mask_matches_the_per_row_walk() {
        let raw_messages = vec![
            String::new(),
            "💬 paste start".to_string(),
            "  line one".to_string(),
            "  ".to_string(), // whitespace-only: passes through, not a user line
            "  line two".to_string(),
            "end of block".to_string(),
            String::new(),
            "🤖 assistant reply".to_string(),
            "  indented continuation".to_string(),
        ];
        let mask = user_line_mask(&raw_messages);
        for (i, expected) in mask.iter().enumerate() {
            assert_eq!(
                *expected,
                is_user_message_line(&raw_messages, i),
                "mask mismatch at row {i}"
            );
        }
        assert_eq!(
            mask,
            vec![false, true, true, false, true, false, false, false, false]
        );
    }

    #[test]
    fn heading_style_after_wrap() {
        use ratatui::style::Modifier;

        use crate::{
            render::{render_md::render_markdown_tui, util::wrap_line},
            theme::ThemeName,
        };

        let theme = Theme::from(ThemeName::Dark);
        let (lines, raw) = render_markdown_tui("### Popular exchanges in HK", &theme);
        assert_eq!(lines.len(), 1);
        let restyled = restyle_log_line(&lines[0], &raw[0], &theme, RawMessageType::LLM, false);
        let wrapped = wrap_line(&restyled, 80);
        assert_eq!(wrapped.len(), 1);
        let span = &wrapped[0].spans[0];
        assert_eq!(span.style.fg, Some(theme.accent));
        assert!(span.style.add_modifier.contains(Modifier::BOLD));
        assert!(span.style.add_modifier.contains(Modifier::ITALIC));
    }
}
