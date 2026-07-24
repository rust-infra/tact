//! Sticky task-progress panel state and pure format helpers.

use tact_protocol::{TaskSnapshot, TaskStatusSnapshot, TasksChangeReason};

use crate::i18n::Messages;

/// Max checklist body rows in sticky expand / Log card.
pub(crate) const STICKY_BODY_CAP: usize = 6;

#[derive(Debug, Clone, Default)]
pub(crate) struct TaskPanelState {
    pub(crate) snapshot: Vec<TaskSnapshot>,
    /// Set on first [`AgentUpdate::TasksChanged`] this UI session.
    pub(crate) session_seen: bool,
    pub(crate) visible: bool,
    pub(crate) expanded: bool,
}

impl TaskPanelState {
    pub(crate) fn sticky_height(&self) -> usize {
        sticky_height(self.expanded, &self.snapshot)
    }

    pub(crate) fn apply_snapshot(&mut self, tasks: Vec<TaskSnapshot>) {
        self.snapshot = tasks;
        self.session_seen = true;
        self.visible = has_open_items(&self.snapshot);
        if !self.visible {
            self.expanded = false;
        }
    }
}

pub(crate) fn has_open_items(tasks: &[TaskSnapshot]) -> bool {
    tasks.iter().any(|t| {
        matches!(
            t.status,
            TaskStatusSnapshot::Pending | TaskStatusSnapshot::InProgress
        )
    })
}

pub(crate) fn sticky_height(expanded: bool, snapshot: &[TaskSnapshot]) -> usize {
    if !expanded {
        return 1;
    }
    1 + snapshot.len().min(STICKY_BODY_CAP)
}

pub(crate) fn completed_count(tasks: &[TaskSnapshot]) -> usize {
    tasks
        .iter()
        .filter(|t| t.status == TaskStatusSnapshot::Completed)
        .count()
}

pub(crate) fn focus_subject(tasks: &[TaskSnapshot]) -> Option<&str> {
    tasks
        .iter()
        .find(|t| t.status == TaskStatusSnapshot::InProgress)
        .or_else(|| {
            tasks
                .iter()
                .find(|t| t.status == TaskStatusSnapshot::Pending)
        })
        .map(|t| t.subject.as_str())
}

/// Checklist lines capped at `cap`, with `… +K` when truncated.
pub(crate) fn format_checklist_lines(tasks: &[TaskSnapshot], cap: usize) -> Vec<String> {
    let mut lines: Vec<String> = tasks
        .iter()
        .take(cap)
        .map(|t| {
            let owner = if t.owner.is_empty() {
                String::new()
            } else {
                format!(" ({})", t.owner)
            };
            format!("{} {}{}", t.status.marker(), t.subject, owner)
        })
        .collect();
    if tasks.len() > cap {
        lines.push(format!("… +{}", tasks.len() - cap));
    }
    lines
}

pub(crate) fn format_sticky_title_line(msgs: &Messages, tasks: &[TaskSnapshot]) -> String {
    let done = completed_count(tasks);
    let total = tasks.len();
    let title = msgs.tasks_sticky_title;
    match focus_subject(tasks) {
        Some(subject) => format!("▸ {title} {done}/{total} · {subject}  ▼"),
        None => format!("▸ {title} {done}/{total}  ▼"),
    }
}

pub(crate) fn format_tasks_log_card(
    msgs: &Messages,
    reason: TasksChangeReason,
    tasks: &[TaskSnapshot],
) -> String {
    let done = completed_count(tasks);
    let total = tasks.len();
    let header = match reason {
        TasksChangeReason::Created => {
            format_two_placeholders(msgs.tasks_log_created_tmpl, done, total)
        }
        TasksChangeReason::Updated => {
            format_two_placeholders(msgs.tasks_log_updated_tmpl, done, total)
        }
    };
    // Leading 📋 keeps `add_system_message` on the plain-line path (not Markdown),
    // so newlines stay as separate Log rows instead of collapsing to spaces.
    let mut out = format!("📋 {header}");
    for line in format_checklist_lines(tasks, STICKY_BODY_CAP) {
        out.push('\n');
        out.push_str("  ");
        out.push_str(&line);
    }
    out
}

fn format_two_placeholders(tmpl: &str, a: usize, b: usize) -> String {
    // Templates use two `{}` in order: done, total.
    let mut parts = tmpl.splitn(3, "{}");
    let prefix = parts.next().unwrap_or("");
    let mid = parts.next().unwrap_or("");
    let suffix = parts.next().unwrap_or("");
    format!("{prefix}{a}{mid}{b}{suffix}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::{Language, Messages};

    fn snap(id: u64, status: TaskStatusSnapshot) -> TaskSnapshot {
        TaskSnapshot {
            id,
            subject: format!("task-{id}"),
            status,
            owner: String::new(),
        }
    }

    #[test]
    fn has_open_items_detects_pending_and_in_progress() {
        assert!(!has_open_items(&[]));
        assert!(has_open_items(&[snap(1, TaskStatusSnapshot::Pending)]));
        assert!(has_open_items(&[snap(1, TaskStatusSnapshot::InProgress)]));
        assert!(!has_open_items(&[snap(1, TaskStatusSnapshot::Completed)]));
    }

    #[test]
    fn sticky_height_collapsed_and_expanded_cap() {
        let many: Vec<_> = (0..10).map(|i| snap(i, TaskStatusSnapshot::Pending)).collect();
        assert_eq!(sticky_height(false, &many), 1);
        assert_eq!(sticky_height(true, &many), 1 + STICKY_BODY_CAP);
    }

    #[test]
    fn format_checklist_caps_with_ellipsis() {
        let many: Vec<_> = (0..8)
            .map(|i| snap(i, TaskStatusSnapshot::Pending))
            .collect();
        let text = format_checklist_lines(&many, 6).join("\n");
        assert!(text.contains("… +2"), "got:\n{text}");
    }

    #[test]
    fn format_tasks_log_card_includes_counts_and_rows() {
        let msgs = Messages::by_language(Language::English);
        let tasks = vec![
            snap(1, TaskStatusSnapshot::Completed),
            snap(2, TaskStatusSnapshot::InProgress),
        ];
        let text = format_tasks_log_card(&msgs, TasksChangeReason::Updated, &tasks);
        assert!(text.starts_with("📋 "), "got:\n{text}");
        assert!(text.contains("1/2"), "got:\n{text}");
        assert!(
            text.contains("\n  [>] task-2"),
            "checklist must be on its own indented line, got:\n{text}"
        );
        assert!(
            !text.contains("updated [>]"),
            "header and checklist must not collapse onto one line, got:\n{text}"
        );
    }
}
