use crate::theme::Theme;
use crate::widgets::state::RawMessageType;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

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

/// Re-apply the active theme to a cached log line (raw text + prior styled line).
pub(crate) fn restyle_log_line(
    stored: &Line,
    raw: &str,
    theme: &Theme,
    msg_type: RawMessageType,
    is_user_line: bool,
) -> Line<'static> {
    if raw.is_empty() {
        return Line::default();
    }

    if is_user_line {
        return single_span(raw, theme.success);
    }

    let trimmed = raw.trim_start();
    if trimmed.starts_with('✓') || trimmed.starts_with('✔') {
        return single_span(raw, theme.success);
    }
    if trimmed.starts_with('✗') {
        return single_span(raw, theme.error);
    }
    if trimmed.starts_with('⚠') {
        return single_span(raw, theme.warning);
    }

    if msg_type == RawMessageType::SysTool {
        return single_span(raw, theme.accent);
    }

    if stored.spans.iter().any(|s| s.style.bg.is_some()) {
        return restyle_code_line(stored, theme);
    }

    let line_style = stored.style;
    let spans: Vec<Span<'static>> = stored
        .spans
        .iter()
        .map(|span| {
            let mut style = line_style.patch(span.style);
            if style.bg == Some(Color::Rgb(70, 90, 140)) {
                style.bg = Some(theme.highlight);
            }
            style.fg = match style.fg {
                Some(Color::Blue) | Some(Color::LightBlue) => style.fg,
                Some(Color::Green) => Some(theme.success),
                Some(Color::Cyan) => Some(theme.accent),
                _ => Some(theme.fg),
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

    fn brutal() -> Theme {
        Theme::by_name(ThemeName::Brutal)
    }

    fn retro() -> Theme {
        Theme::by_name(ThemeName::Retro)
    }

    fn stored_plain(text: &str, fg: Color) -> Line<'static> {
        Line::from(Span::styled(text.to_string(), Style::default().fg(fg)))
    }

    fn stored_code(text: &str) -> Line<'static> {
        Line::from(Span::styled(
            text.to_string(),
            Style::default()
                .fg(Color::Rgb(200, 200, 210))
                .bg(Color::Rgb(30, 35, 50)),
        ))
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

        assert_eq!(ok.spans.first().unwrap().style.fg, Some(theme.success));
        assert_eq!(err.spans.first().unwrap().style.fg, Some(theme.error));
        assert_eq!(warn.spans.first().unwrap().style.fg, Some(theme.warning));
    }

    #[test]
    fn code_block_restyles_for_light_theme() {
        let theme = brutal();
        let line = restyle_log_line(
            &stored_code("fn main() {}"),
            "fn main() {}",
            &theme,
            RawMessageType::LLM,
            false,
        );
        assert_eq!(line.spans.first().unwrap().style.bg, Some(theme.code_block_bg()));
        assert_eq!(line.spans.first().unwrap().style.fg, Some(theme.code_block_fg()));
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
    fn unrelated_continuation_is_not_user_line() {
        let raw_messages = vec![
            "🤖 assistant".to_string(),
            "  still assistant".to_string(),
        ];
        assert!(!is_user_message_line(&raw_messages, 1));
    }

    #[test]
    fn heading_style_after_wrap() {
        use crate::render::render_md::render_markdown_tui;
        use crate::render::util::wrap_line;
        use crate::theme::ThemeName;
        use ratatui::style::Modifier;

        let theme = Theme::by_name(ThemeName::Dark);
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
