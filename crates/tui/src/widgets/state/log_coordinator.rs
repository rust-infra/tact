//! Shared-log ownership — the priority-0 component.
//!
//! Phase 1 of the kit plan: `App::log_items` moves here; primitive row
//! operations (append / insert / splice / remove / placeholder push) become
//! coordinator methods. Cross-state helpers (gap checks, index fixups,
//! i18n-driven messages) stay on `App` for now and delegate to these
//! primitives. Phase 3 moves this module into `agent_tui_kit`.

use ratatui::text::Line;

use crate::theme::Theme;

use super::{LogItem, LogItemKind};

/// Owns the shared log rows and all primitive row operations.
#[derive(Default)]
pub(crate) struct LogCoordinator {
    /// The physical log rows (user / assistant / system / placeholder).
    pub(crate) items: Vec<LogItem>,
}

impl LogCoordinator {
    /// Append one log row, keeping all row metadata together in `items`.
    pub(crate) fn append_msg(&mut self, line: Line<'static>, raw: String, kind: LogItemKind) {
        self.items.push(LogItem::new(line, raw, kind));
    }

    /// Append a whole-Markdown notice as a single log item.
    pub(crate) fn append_markdown(&mut self, content: String, theme: &Theme, kind: LogItemKind) {
        self.items.push(LogItem::markdown(content, theme, kind));
    }

    /// Append a blank row of the given kind.
    pub(crate) fn append_blank(&mut self, kind: LogItemKind) {
        self.append_msg(Line::from(""), String::new(), kind);
    }

    pub(crate) fn extend_msgs(
        &mut self,
        lines: Vec<Line<'static>>,
        raw_lines: Vec<String>,
        kind: LogItemKind,
    ) {
        debug_assert_eq!(lines.len(), raw_lines.len());
        for (line, raw) in lines.into_iter().zip(raw_lines) {
            self.append_msg(line, raw, kind);
        }
    }

    pub(crate) fn insert_msg(
        &mut self,
        idx: usize,
        line: Line<'static>,
        raw: String,
        kind: LogItemKind,
    ) {
        self.items.insert(idx, LogItem::new(line, raw, kind));
    }

    pub(crate) fn splice_msgs(
        &mut self,
        range: std::ops::Range<usize>,
        lines: Vec<Line<'static>>,
        raw: Vec<String>,
        kind: LogItemKind,
    ) {
        debug_assert_eq!(lines.len(), raw.len());
        self.items.splice(
            range,
            lines
                .into_iter()
                .zip(raw)
                .map(|(line, raw)| LogItem::new(line, raw, kind)),
        );
    }

    pub(crate) fn drain_msgs(&mut self, range: std::ops::Range<usize>) {
        self.items.drain(range);
    }

    pub(crate) fn remove_msg(&mut self, idx: usize) {
        self.items.remove(idx);
    }

    /// Push `rows` blank placeholder rows of `kind`; returns the first
    /// physical index (the anchor row for the component that owns them).
    pub(crate) fn push_placeholder_rows(&mut self, kind: LogItemKind, rows: usize) -> usize {
        let phys_idx = self.items.len();
        for _ in 0..rows {
            self.append_blank(kind);
        }
        phys_idx
    }
}
