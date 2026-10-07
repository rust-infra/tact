use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Paragraph, Widget},
};

use super::super::renderable::Renderable;

/// Sentinel prefix stored in `LogItem::raw` for task-end rules.
/// Optional payload: `\x07tact-task-end\x1f{secs}` encodes elapsed seconds.
pub const TASK_END_SEPARATOR: &str = "\x07tact-task-end";
const TASK_END_ELAPSED_SEP: char = '\x1f';

pub fn is_task_end_separator(raw: &str) -> bool {
    raw.starts_with(TASK_END_SEPARATOR)
}

/// Build raw sentinel with frozen elapsed seconds.
pub fn task_end_separator_raw(elapsed_secs: i64) -> String {
    format!(
        "{TASK_END_SEPARATOR}{TASK_END_ELAPSED_SEP}{}",
        elapsed_secs.max(0)
    )
}

/// Parse elapsed seconds from a task-end sentinel, if present.
pub fn task_end_elapsed_secs(raw: &str) -> Option<i64> {
    let prefix = format!("{TASK_END_SEPARATOR}{TASK_END_ELAPSED_SEP}");
    raw.strip_prefix(&prefix)?.parse().ok()
}

/// Full-width accent-colored rule appended after a completed task response.
///
/// It is the turn *boundary* and nothing else: the frozen elapsed label that
/// used to sit centered in it was dropped (2026-10-05) because the same number
/// is drawn twice already — the task-stats row directly below it, and the
/// bottom bar's turn segment. The `raw` sentinel still carries the seconds
/// ([`task_end_separator_raw`]); only the rendering stopped showing them.
pub struct TaskEndSeparator {
    fg: Color,
}

impl TaskEndSeparator {
    pub fn new(fg: Color) -> Self {
        Self { fg }
    }

    fn solid_line(width: u16) -> String {
        "─".repeat(width as usize)
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
        let line = Line::from(Span::styled(Self::solid_line(area.width), style));
        Paragraph::new(line).render(area, buf);
    }

    fn height(&self, _width: u16) -> u16 {
        1
    }
}

/// A blank line separator drawn between message groups of different
/// categories (user ↔ system ↔ assistant).
pub struct MessageSeparator {
    _label: String,
    _fg: Color,
}

impl MessageSeparator {
    pub fn new(label: String, fg: Color) -> Self {
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

    /// The rule carries no elapsed label: the turn's clock lives on the
    /// task-stats row below it (and the bottom bar's turn segment). Locked at
    /// the cell level because the sentinel payload still holds the seconds —
    /// nothing may start drawing them again by accident.
    #[test]
    fn task_end_separator_draws_no_elapsed_label() {
        let sep = TaskEndSeparator::new(Color::Gray);
        let area = Rect::new(0, 0, 40, 1);
        let mut buf = Buffer::empty(area);
        sep.render(area, &mut buf);
        let text: String = (0..area.width)
            .map(|x| buf[(x, 0)].symbol().to_string())
            .collect();
        assert_eq!(
            text,
            "─".repeat(40),
            "the task-end rule must be solid, got: {text}"
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
