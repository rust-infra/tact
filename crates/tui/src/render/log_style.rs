use std::collections::HashSet;

use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

use super::slash_style::style_user_skill_line;
use crate::{
    theme::Theme,
    widgets::state::{LogItemKind, SystemMsgStyle},
};

/// Caller should build `skill_names` once per cache rebuild (`perf-` / `mem-reuse`).
/// `user_prefix_tmpl` / `user_cont_tmpl` are i18n templates like `"💬 {}"`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn restyle_log_line_with_skills(
    stored: &Line,
    raw: &str,
    theme: &Theme,
    kind: LogItemKind,
    skill_names: &HashSet<&str>,
    user_prefix_tmpl: &str,
    user_cont_tmpl: &str,
) -> Line<'static> {
    if raw.is_empty() {
        return Line::default();
    }

    if kind.is_user() {
        if let Some(line) =
            style_user_skill_line(raw, skill_names, theme, user_prefix_tmpl, user_cont_tmpl)
        {
            return line;
        }
        return single_span(raw, theme.success);
    }

    if let LogItemKind::SystemPlain(style) = kind
        && style != SystemMsgStyle::Default
    {
        return single_span(raw, style.color(theme));
    }

    if kind == LogItemKind::SystemTool {
        return single_span(raw, theme.accent);
    }

    // Only whole fenced-code lines — every span already carries the code
    // background — are restyled as a code block. A prose line that merely
    // contains inline code (e.g. a `- run `cargo build`` list item) must keep
    // its prose styling: restyling it here paints the whole line with the code
    // background, a full-width highlight block that `wrap_line` re-slices onto
    // every wrapped continuation row and reads as a shadow band.
    let is_code_line = stored
        .spans
        .iter()
        .all(|s| s.style.bg == Some(theme.code_block_bg()));
    if is_code_line {
        return restyle_code_line(stored, theme);
    }

    let line_style = stored.style;
    let spans: Vec<Span<'static>> = stored
        .spans
        .iter()
        .map(|span| {
            let mut style = line_style.patch(span.style);
            if style.bg == Some(theme.code_block_bg()) {
                // Inline code is part of prose, so distinguish it with the
                // accent foreground rather than a rectangular code-block patch.
                style.bg = None;
                style.fg = Some(theme.accent);
                return Span::styled(span.content.to_string(), style);
            }
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
        kind: LogItemKind,
    ) -> Line<'static> {
        let empty = HashSet::new();
        restyle_log_line_with_skills(stored, raw, theme, kind, &empty, "💬 {}", "  {}")
    }

    #[test]
    fn user_first_and_continuation_lines_use_success() {
        let theme = brutal();
        let first = restyle_log_line(
            &stored_plain("💬 hello", Color::Green),
            "💬 hello",
            &theme,
            LogItemKind::User,
        );
        let cont = restyle_log_line(
            &stored_plain("  continued", Color::Green),
            "  continued",
            &theme,
            LogItemKind::User,
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
            LogItemKind::SystemPlain(SystemMsgStyle::Success),
        );
        let err = restyle_log_line(
            &stored_plain("✗ failed", Color::Red),
            "✗ failed",
            &theme,
            LogItemKind::SystemPlain(SystemMsgStyle::Error),
        );
        let warn = restyle_log_line(
            &stored_plain("⚠ retry", Color::Yellow),
            "⚠ retry",
            &theme,
            LogItemKind::SystemPlain(SystemMsgStyle::Warning),
        );

        let err_x = restyle_log_line(
            &stored_plain("❌ boom", Color::Red),
            "❌ boom",
            &theme,
            LogItemKind::SystemPlain(SystemMsgStyle::Error),
        );
        let ok_badge = restyle_log_line(
            &stored_plain("✅ ok", Color::Green),
            "✅ ok",
            &theme,
            LogItemKind::SystemPlain(SystemMsgStyle::Success),
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
            LogItemKind::AssistantMarkdown,
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
    fn inline_code_uses_accent_without_background() {
        // Inline code is prose, not a code block: use the accent foreground
        // without painting a rectangular background patch.
        let theme = brutal();
        let stored = Line::from(vec![
            Span::styled("• ".to_string(), Style::default().fg(theme.fg)),
            Span::styled("run ".to_string(), Style::default().fg(theme.fg)),
            Span::styled(
                "cargo build".to_string(),
                Style::default()
                    .fg(theme.code_block_fg())
                    .bg(theme.code_block_bg()),
            ),
            Span::styled(" now".to_string(), Style::default().fg(theme.fg)),
        ]);
        let line = restyle_log_line(
            &stored,
            "- run `cargo build` now",
            &theme,
            LogItemKind::AssistantMarkdown,
        );
        let bgs: Vec<Option<Color>> = line.spans.iter().map(|s| s.style.bg).collect();
        assert_eq!(
            bgs,
            vec![None, None, None, None],
            "inline code must not carry a code-block background"
        );
        assert_eq!(line.spans[2].style.fg, Some(theme.accent));
        assert_eq!(line.spans[0].style.fg, Some(theme.fg));
        assert_eq!(line.spans[3].style.fg, Some(theme.fg));
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
        let line = restyle_log_line(&stored, "# Title", &theme, LogItemKind::AssistantMarkdown);
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
            LogItemKind::AssistantMarkdown,
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
            LogItemKind::SystemTool,
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
            LogItemKind::AssistantMarkdown,
        );
        assert_eq!(
            line.spans.first().unwrap().style.fg,
            Some(theme.heading),
            "blue must remap to {heading:?}",
            heading = theme.heading
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
        let restyled = restyle_log_line(&lines[0], &raw[0], &theme, LogItemKind::AssistantMarkdown);
        let wrapped = wrap_line(&restyled, 80);
        assert_eq!(wrapped.len(), 1);
        let span = &wrapped[0].spans[0];
        assert_eq!(span.style.fg, Some(theme.accent));
        assert!(span.style.add_modifier.contains(Modifier::BOLD));
        assert!(span.style.add_modifier.contains(Modifier::ITALIC));
    }
}
