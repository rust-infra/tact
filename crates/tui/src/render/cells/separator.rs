use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Paragraph, Widget},
};
use unicode_width::UnicodeWidthStr;

use super::super::renderable::Renderable;

/// Sentinel prefix stored in `LogItem::raw` for task-end rules.
/// Optional payload: `\x07tact-task-end\x1f{secs}` encodes elapsed seconds.
pub(crate) const TASK_END_SEPARATOR: &str = "\x07tact-task-end";
const TASK_END_ELAPSED_SEP: char = '\x1f';

pub(crate) fn is_task_end_separator(raw: &str) -> bool {
    raw.starts_with(TASK_END_SEPARATOR)
}

/// Build raw sentinel with frozen elapsed seconds.
pub(crate) fn task_end_separator_raw(elapsed_secs: i64) -> String {
    format!(
        "{TASK_END_SEPARATOR}{TASK_END_ELAPSED_SEP}{}",
        elapsed_secs.max(0)
    )
}

/// Parse elapsed seconds from a task-end sentinel, if present.
pub(crate) fn task_end_elapsed_secs(raw: &str) -> Option<i64> {
    let prefix = format!("{TASK_END_SEPARATOR}{TASK_END_ELAPSED_SEP}");
    raw.strip_prefix(&prefix)?.parse().ok()
}

fn format_mm_ss(total_secs: i64) -> String {
    let secs = total_secs.max(0);
    format!("{:02}:{:02}", secs / 60, secs % 60)
}

/// Full-width accent-colored rule appended after a completed task response.
/// When `elapsed_label` is set (e.g. `"Elapsed 00:03"`), it is centered in the rule.
pub(crate) struct TaskEndSeparator {
    fg: Color,
    elapsed_label: Option<String>,
}

impl TaskEndSeparator {
    pub(crate) fn new(fg: Color) -> Self {
        Self {
            fg,
            elapsed_label: None,
        }
    }

    pub(crate) fn with_elapsed(fg: Color, label: &str, elapsed_secs: i64) -> Self {
        Self {
            fg,
            elapsed_label: Some(format!("{label} {}", format_mm_ss(elapsed_secs))),
        }
    }

    fn solid_line(width: u16) -> String {
        "─".repeat(width as usize)
    }

    /// `──── Elapsed 00:03 ────` filling `width` columns.
    fn ruled_with_label(width: u16, label: &str) -> String {
        let padded = format!(" {label} ");
        let label_w = UnicodeWidthStr::width(padded.as_str()) as u16;
        if width == 0 {
            return String::new();
        }
        if label_w >= width {
            // Prefer showing the label; truncate by chars if needed.
            let mut out = String::new();
            for ch in padded.chars() {
                let next = UnicodeWidthStr::width(out.as_str()) as u16
                    + UnicodeWidthStr::width(ch.to_string().as_str()) as u16;
                if next > width {
                    break;
                }
                out.push(ch);
            }
            while (UnicodeWidthStr::width(out.as_str()) as u16) < width {
                out.push(' ');
            }
            return out;
        }
        let remaining = width - label_w;
        let left = remaining / 2;
        let right = remaining - left;
        format!(
            "{}{}{}",
            "─".repeat(left as usize),
            padded,
            "─".repeat(right as usize)
        )
    }
}

impl Renderable for TaskEndSeparator {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        self.render_partial(area, buf, 0);
    }

    fn render_partial(&self, area: Rect, buf: &mut Buffer, skip_lines: usize) {
        if skip_lines >= 1 || area.height == 0 || area.width == 0 {
            return;
        }
        let style = Style::default().fg(self.fg);
        let text = match &self.elapsed_label {
            Some(label) => Self::ruled_with_label(area.width, label),
            None => Self::solid_line(area.width),
        };
        let line = Line::from(Span::styled(text, style));
        Paragraph::new(line).render(area, buf);
    }

    fn height(&self, _width: u16) -> u16 {
        1
    }
}

/// A blank line separator drawn between message groups of different
/// categories (user ↔ system ↔ assistant).
pub(crate) struct MessageSeparator {
    _label: String,
    _fg: Color,
}

impl MessageSeparator {
    pub(crate) fn new(label: String, fg: Color) -> Self {
        Self {
            _label: label,
            _fg: fg,
        }
    }
}

impl Renderable for MessageSeparator {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        self.render_partial(area, buf, 0);
    }

    fn render_partial(&self, area: Rect, buf: &mut Buffer, skip_lines: usize) {
        if skip_lines >= 1 || area.height == 0 {
            return;
        }
        // Single blank line to separate message groups
        let blank_line = Line::from("");
        let gap_area = Rect::new(area.x, area.y, area.width, 1);
        Paragraph::new(blank_line).render(gap_area, buf);
    }

    fn height(&self, _width: u16) -> u16 {
        1
    }
}

#[cfg(test)]
mod render_tests {
    use super::*;

    #[test]
    fn task_end_separator_renders_solid_line() {
        let sep = TaskEndSeparator::new(Color::Gray);
        let area = Rect::new(0, 0, 20, 1);
        let mut buf = Buffer::empty(area);
        sep.render(area, &mut buf);
        let text: String = (0..area.width)
            .map(|x| buf[(x, 0)].symbol().to_string())
            .collect();
        assert_eq!(
            text, "────────────────────",
            "task end separator should draw solid line, got: {text}"
        );
    }

    #[test]
    fn task_end_separator_embeds_elapsed_label() {
        let sep = TaskEndSeparator::with_elapsed(Color::Gray, "Elapsed", 65);
        let area = Rect::new(0, 0, 40, 1);
        let mut buf = Buffer::empty(area);
        sep.render(area, &mut buf);
        let text: String = (0..area.width)
            .map(|x| buf[(x, 0)].symbol().to_string())
            .collect();
        assert!(
            text.contains("Elapsed 01:05"),
            "expected centered elapsed label, got: {text}"
        );
        assert!(
            text.contains('─'),
            "expected rule glyphs around label, got: {text}"
        );
        assert_eq!(
            UnicodeWidthStr::width(text.as_str()),
            40,
            "ruled line should fill width, got: {text}"
        );
    }

    #[test]
    fn task_end_raw_round_trips_elapsed() {
        let raw = task_end_separator_raw(125);
        assert!(is_task_end_separator(&raw));
        assert_eq!(task_end_elapsed_secs(&raw), Some(125));
        assert!(is_task_end_separator(TASK_END_SEPARATOR));
        assert_eq!(task_end_elapsed_secs(TASK_END_SEPARATOR), None);
    }

    #[test]
    fn message_separator_renders_blank_gap_line() {
        let sep = MessageSeparator::new("💬 user".into(), Color::Cyan);
        let area = Rect::new(0, 0, 10, 1);
        let mut buf = Buffer::empty(area);
        sep.render(area, &mut buf);
        assert_eq!(sep.height(10), 1);
        let rendered: String = (0..area.width)
            .map(|x| buf[(x, 0)].symbol().to_string())
            .collect();
        assert!(
            rendered.trim().is_empty(),
            "message separator row should stay visually blank"
        );
    }
}
