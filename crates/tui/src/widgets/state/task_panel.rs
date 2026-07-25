//! Sticky task-progress panel state and pure format helpers.

use tact_protocol::{TaskSnapshot, TaskStatusSnapshot, TasksChangeReason};

use crate::i18n::Messages;

#[derive(Debug, Clone)]
pub(crate) struct TaskPanelState {
    pub(crate) snapshot: Vec<TaskSnapshot>,
    /// Set on first [`AgentUpdate::TasksChanged`] this UI session.
    pub(crate) session_seen: bool,
    pub(crate) visible: bool,
    pub(crate) expanded: bool,
    pub(crate) scroll: usize,
    pub(crate) max_visible: usize,
}

impl Default for TaskPanelState {
    fn default() -> Self {
        Self {
            snapshot: Vec::new(),
            session_seen: false,
            visible: false,
            expanded: false,
            scroll: 0,
            max_visible: 10,
        }
    }
}

impl TaskPanelState {
    #[allow(dead_code)] // retained for tests / callers that still query panel-only height
    pub(crate) fn sticky_height(&self) -> usize {
        sticky_height(self.expanded, &self.snapshot)
    }

    pub(crate) fn apply_snapshot(&mut self, tasks: Vec<TaskSnapshot>) {
        self.scroll = 0;
        let was_visible = self.visible;
        self.snapshot = tasks;
        self.session_seen = true;
        self.visible = has_open_items(&self.snapshot);
        if self.visible {
            if !was_visible {
                // Default expanded when the strip first appears (or reappears).
                self.expanded = true;
            }
        } else {
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
    2 + format_grouped_lines(snapshot, 0, 10).len().max(1)
}

pub(crate) fn completed_count(tasks: &[TaskSnapshot]) -> usize {
    tasks
        .iter()
        .filter(|t| t.status == TaskStatusSnapshot::Completed)
        .count()
}

pub(crate) fn format_duration(
    started_at: Option<i64>,
    completed_at: Option<i64>,
) -> Option<String> {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;

    let end = completed_at.unwrap_or(now_ms);
    let start = started_at?;
    if end <= start {
        return None;
    }
    let secs = (end - start) / 1000;
    if secs < 60 {
        Some(format!("{}s", secs))
    } else if secs < 3600 {
        Some(format!("{}m {}s", secs / 60, secs % 60))
    } else {
        Some(format!("{}h {:02}m", secs / 3600, (secs % 3600) / 60))
    }
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

#[allow(dead_code)]
fn format_task_row(t: &TaskSnapshot) -> String {
    let owner = if t.owner.is_empty() {
        String::new()
    } else {
        format!(" ({})", t.owner)
    };
    format!("{} #{} {}{}", t.status.marker(), t.id, t.subject, owner)
}

/// Dependency-tree-free grouped lines: In Progress, Pending, Completed.
/// Each group is sorted by recency/duration. Supports scrolling.
pub(crate) fn format_grouped_lines(
    tasks: &[TaskSnapshot],
    scroll: usize,
    max_visible: usize,
) -> Vec<String> {
    if tasks.is_empty() {
        return Vec::new();
    }

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;

    let mut in_progress: Vec<&TaskSnapshot> = Vec::new();
    let mut pending: Vec<&TaskSnapshot> = Vec::new();
    let mut completed: Vec<&TaskSnapshot> = Vec::new();

    for t in tasks {
        match t.status {
            TaskStatusSnapshot::InProgress => in_progress.push(t),
            TaskStatusSnapshot::Pending => pending.push(t),
            TaskStatusSnapshot::Completed => completed.push(t),
        }
    }

    // Sort: InProgress by duration ascending, Pending by created_at descending,
    // Completed by completed_at descending
    in_progress.sort_by_key(|t| {
        let start = t.started_at.unwrap_or(0);
        (now - start).max(0)
    });
    pending.sort_by_key(|b| std::cmp::Reverse(b.created_at.unwrap_or(0)));
    completed.sort_by(|a, b| {
        b.completed_at
            .unwrap_or(0)
            .cmp(&a.completed_at.unwrap_or(0))
    });

    let mut all_lines: Vec<String> = Vec::new();

    fn push_group(out: &mut Vec<String>, header: &str, items: &[&TaskSnapshot]) {
        if items.is_empty() {
            return;
        }
        out.push(format!("── {} ──", header));
        for t in items {
            let meta_parts: Vec<String> = [
                if t.owner.is_empty() {
                    None
                } else {
                    Some(t.owner.clone())
                },
                if t.blocked_by.is_empty() {
                    None
                } else {
                    let ids: Vec<String> =
                        t.blocked_by.iter().map(|id| format!("#{}", id)).collect();
                    Some(format!("⏳ {}", ids.join(", ")))
                },
                format_duration(t.started_at, t.completed_at).map(|d| format!("⏱ {}", d)),
            ]
            .into_iter()
            .flatten()
            .collect();

            let meta = if meta_parts.is_empty() {
                String::new()
            } else {
                format!("  {}", meta_parts.join("  "))
            };

            out.push(format!(
                "{} #{} {}{}",
                t.status.marker(),
                t.id,
                t.subject,
                meta
            ));
        }
    }

    push_group(&mut all_lines, "In Progress", &in_progress);
    push_group(&mut all_lines, "Pending", &pending);
    push_group(&mut all_lines, "Completed", &completed);

    let total = all_lines.len();
    if total <= max_visible {
        return all_lines;
    }

    let scroll = scroll.min(total.saturating_sub(max_visible));
    let mut visible: Vec<String> = all_lines
        .iter()
        .skip(scroll)
        .take(max_visible)
        .cloned()
        .collect();
    let remaining = total.saturating_sub(scroll + max_visible);
    if remaining > 0 {
        visible.push(format!("⋯ +{} more · scroll ▼", remaining));
    } else if scroll > 0 {
        visible.push("⋯ scroll ▲".into());
    }
    visible
}

/// Flat checklist (legacy helper / tests).
#[allow(dead_code)]
pub(crate) fn format_checklist_lines(tasks: &[TaskSnapshot]) -> Vec<String> {
    tasks.iter().map(format_task_row).collect()
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

/// Short Log card: primary action + changed fields only.
#[allow(dead_code)]
#[allow(clippy::collapsible_if)]
pub(crate) fn format_tasks_log_card(
    _msgs: &Messages,
    reason: TasksChangeReason,
    prev: &[TaskSnapshot],
    next: &[TaskSnapshot],
) -> String {
    let focus = focus_changed_task(reason, prev, next);
    let Some(curr) = focus else {
        return "📋 Tasks updated".into();
    };
    let prev_t = prev.iter().find(|t| t.id == curr.id);
    let primary = primary_action_for_change(reason, prev_t, curr);
    let mut out = format!("📋 # Task.{} · {primary}", curr.id);

    if prev_t.map(|p| p.subject.as_str()) != Some(curr.subject.as_str())
        || matches!(reason, TasksChangeReason::Created)
    {
        if !curr.subject.is_empty() {
            out.push_str(&format!("\n  任务名: {}", curr.subject));
        }
    }
    let prev_owner = prev_t.map(|p| p.owner.as_str()).unwrap_or("");
    if prev_owner != curr.owner && !curr.owner.is_empty() {
        out.push_str(&format!("\n  负责人:{}", curr.owner));
    }
    let prev_bb = prev_t.map(|p| p.blocked_by.as_slice()).unwrap_or(&[]);
    if prev_bb != curr.blocked_by.as_slice() {
        out.push_str(&format!(
            "\n  被阻塞于: {}",
            tact::task::format_id_transition(prev_bb, &curr.blocked_by)
        ));
    }
    let prev_bl = prev_t.map(|p| p.blocks.as_slice()).unwrap_or(&[]);
    if prev_bl != curr.blocks.as_slice() {
        out.push_str(&format!(
            "\n  阻塞: {}",
            tact::task::format_id_transition(prev_bl, &curr.blocks)
        ));
    }
    out
}

#[allow(dead_code)]
fn focus_changed_task<'a>(
    reason: TasksChangeReason,
    prev: &[TaskSnapshot],
    next: &'a [TaskSnapshot],
) -> Option<&'a TaskSnapshot> {
    match reason {
        TasksChangeReason::Created => next
            .iter()
            .find(|t| prev.iter().all(|p| p.id != t.id))
            .or_else(|| next.last()),
        TasksChangeReason::Updated => next
            .iter()
            .find(|t| {
                prev.iter()
                    .find(|p| p.id == t.id)
                    .map(|p| p != *t)
                    .unwrap_or(true)
            })
            .or_else(|| next.last()),
    }
}

#[allow(dead_code)]
fn primary_action_for_change(
    reason: TasksChangeReason,
    prev: Option<&TaskSnapshot>,
    curr: &TaskSnapshot,
) -> &'static str {
    if matches!(reason, TasksChangeReason::Created) || prev.is_none() {
        return "创建任务";
    }
    let prev = prev.unwrap();
    if prev.status != curr.status {
        return match curr.status {
            TaskStatusSnapshot::InProgress => "执行任务",
            TaskStatusSnapshot::Completed => "完成任务",
            TaskStatusSnapshot::Pending => "重置任务",
        };
    }
    if prev.owner != curr.owner {
        return "设置负责人";
    }
    if prev.blocked_by != curr.blocked_by {
        return "被阻塞于";
    }
    if prev.blocks != curr.blocks {
        return "阻塞";
    }
    "空更新"
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
            blocks: Vec::new(),
            blocked_by: Vec::new(),
            ..Default::default()
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
    fn sticky_height_collapsed_and_expanded_full() {
        let many: Vec<_> = (0..10)
            .map(|i| snap(i, TaskStatusSnapshot::Pending))
            .collect();
        assert_eq!(sticky_height(false, &many), 1);
        // format_grouped_lines adds 1 header + N items = N+1 lines; sticky_height adds 2 (title + blank)
        assert_eq!(sticky_height(true, &many), 2 + (1 + many.len()));
    }

    #[test]
    fn format_checklist_includes_id() {
        let many: Vec<_> = (0..3)
            .map(|i| snap(i, TaskStatusSnapshot::Pending))
            .collect();
        let lines = format_checklist_lines(&many);
        assert!(lines[0].contains("#0"), "{lines:?}");
        assert!(lines[2].contains("#2"), "{lines:?}");
    }

    #[test]
    fn format_tasks_log_card_short_diff() {
        let msgs = Messages::by_language(Language::English);
        let prev = vec![snap(1, TaskStatusSnapshot::InProgress)];
        let mut next = snap(1, TaskStatusSnapshot::Completed);
        next.subject = "后端接口".into();
        let text = format_tasks_log_card(&msgs, TasksChangeReason::Updated, &prev, &[next]);
        assert!(text.starts_with("📋 # Task.1 · 完成任务"), "got:\n{text}");
    }

    #[test]
    fn format_duration_returns_none_for_no_start() {
        assert_eq!(format_duration(None, None), None);
        assert_eq!(format_duration(None, Some(1000)), None);
    }

    #[test]
    fn format_duration_produces_readable_strings() {
        // completed: 100s = 1m 40s
        let started = 1000;
        let completed = 101_000; // 100s later
        let d = format_duration(Some(started), Some(completed));
        assert_eq!(d, Some("1m 40s".into()));

        // 65 minutes
        let completed = 65 * 60 * 1000 + 1000;
        let d = format_duration(Some(1000), Some(completed));
        assert_eq!(d, Some("1h 05m".into()));

        // 30 seconds
        let completed = 31_000;
        let d = format_duration(Some(1000), Some(completed));
        assert_eq!(d, Some("30s".into()));
    }

    #[test]
    fn format_grouped_lines_produces_grouped_output() {
        let ip = TaskSnapshot {
            id: 1,
            subject: "work".into(),
            status: TaskStatusSnapshot::InProgress,
            owner: "alice".into(),
            ..Default::default()
        };
        let pd = TaskSnapshot {
            id: 2,
            subject: "todo".into(),
            status: TaskStatusSnapshot::Pending,
            ..Default::default()
        };
        let done = TaskSnapshot {
            id: 3,
            subject: "done".into(),
            status: TaskStatusSnapshot::Completed,
            owner: "bob".into(),
            ..Default::default()
        };

        let lines = format_grouped_lines(&[pd.clone(), done.clone(), ip.clone()], 0, 10);
        assert!(
            lines.len() >= 3,
            "should have at least 3 content lines, got {}:\n{:#?}",
            lines.len(),
            lines
        );

        // Groups should be in order: In Progress, Pending, Completed
        let text = lines.join("\n");
        let ip_pos = text.find("In Progress").unwrap();
        let pd_pos = text.find("Pending").unwrap();
        let done_pos = text.find("Completed").unwrap();
        assert!(ip_pos < pd_pos, "InProgress before Pending");
        assert!(pd_pos < done_pos, "Pending before Completed");

        // Each task should be present
        assert!(text.contains("#1 work"), "ip task");
        assert!(text.contains("#2 todo"), "pd task");
        assert!(text.contains("#3 done"), "completed task");

        // Owner should show
        assert!(text.contains("alice"), "owner alice");
        assert!(text.contains("bob"), "owner bob");
    }

    #[test]
    fn format_grouped_lines_scroll_caps_at_max_visible() {
        let many: Vec<_> = (0..15)
            .map(|i| TaskSnapshot {
                id: i,
                subject: format!("t-{i}"),
                status: TaskStatusSnapshot::Pending,
                ..Default::default()
            })
            .collect();

        // max_visible = 10
        let lines = format_grouped_lines(&many, 0, 10);
        // 1 group header + 10 task rows + 1 "more" line
        assert!(
            lines.len() <= 12,
            "capped at ~12 lines, got {}",
            lines.len()
        );
        assert!(
            lines.last().unwrap().contains("more"),
            "should show remaining count"
        );
    }
}
