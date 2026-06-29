use crate::theme::Theme;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

/// Re-apply the active theme to a cached log line (raw text + prior styled line).
pub(crate) fn restyle_log_line(stored: &Line, raw: &str, theme: &Theme) -> Line<'static> {
    if raw.is_empty() {
        return Line::default();
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
    if trimmed.starts_with('💬') {
        return single_span(raw, theme.success);
    }
    if trimmed.starts_with("  💬") {
        return single_span(raw, theme.success);
    }

    if stored.spans.iter().any(|s| s.style.bg.is_some()) {
        return restyle_code_line(stored, theme);
    }

    let spans: Vec<Span<'static>> = stored
        .spans
        .iter()
        .map(|span| {
            let mut style = span.style;
            if style.bg.is_some() {
                return Span::styled(span.content.to_string(), style);
            }
            if style.add_modifier.contains(Modifier::BOLD) && style.fg == Some(Color::Cyan) {
                style = style.fg(theme.accent);
            } else if style.fg == Some(Color::Blue) || style.fg == Some(Color::LightBlue) {
                // keep link color
            } else if style.fg == Some(Color::Green) {
                style = style.fg(theme.success);
            } else {
                style = style.fg(theme.fg);
            }
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
