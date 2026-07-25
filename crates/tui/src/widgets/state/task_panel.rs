//! Sticky task-progress panel state and pure format helpers.

use std::collections::{HashMap, HashSet};

use tact_protocol::{TaskSnapshot, TaskStatusSnapshot, TasksChangeReason};

use crate::i18n::Messages;

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
    1 + format_tree_lines(snapshot).len().max(1)
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

fn format_task_row(t: &TaskSnapshot) -> String {
    let owner = if t.owner.is_empty() {
        String::new()
    } else {
        format!(" ({})", t.owner)
    };
    format!("{} #{} {}{}", t.status.marker(), t.id, t.subject, owner)
}

/// Dependency tree by `blocks` edges; multi-parent nodes repeat (A1).
pub(crate) fn format_tree_lines(tasks: &[TaskSnapshot]) -> Vec<String> {
    if tasks.is_empty() {
        return Vec::new();
    }
    let by_id: HashMap<u64, &TaskSnapshot> = tasks.iter().map(|t| (t.id, t)).collect();
    let mut roots: Vec<&TaskSnapshot> = tasks
        .iter()
        .filter(|t| t.blocked_by.is_empty())
        .collect();
    roots.sort_by_key(|t| t.id);

    let mut out = Vec::new();
    let mut seen_as_root_child = HashSet::new();
    for root in &roots {
        walk_tree(&mut out, root, &by_id, "", true, true, &mut Vec::new());
        mark_descendants(*root, &by_id, &mut seen_as_root_child);
    }
    // Orphans (cycles / missing parents): append flat.
    let mut orphans: Vec<&TaskSnapshot> = tasks
        .iter()
        .filter(|t| !roots.iter().any(|r| r.id == t.id) && !seen_as_root_child.contains(&t.id))
        .collect();
    // Also include nodes never reached from any root walk.
    let mut reached = HashSet::new();
    for line_task in tasks {
        // crude: if id appears in any tree line already
        let needle = format!(" #{} ", line_task.id);
        if out.iter().any(|l| l.contains(&needle) || l.contains(&format!(" #{}", line_task.id))) {
            reached.insert(line_task.id);
        }
    }
    orphans.retain(|t| !reached.contains(&t.id));
    orphans.sort_by_key(|t| t.id);
    for t in orphans {
        out.push(format_task_row(t));
    }
    out
}

fn mark_descendants(
    node: &TaskSnapshot,
    by_id: &HashMap<u64, &TaskSnapshot>,
    seen: &mut HashSet<u64>,
) {
    for &cid in &node.blocks {
        if seen.insert(cid)
            && let Some(child) = by_id.get(&cid)
        {
            mark_descendants(child, by_id, seen);
        }
    }
}

fn walk_tree(
    out: &mut Vec<String>,
    node: &TaskSnapshot,
    by_id: &HashMap<u64, &TaskSnapshot>,
    prefix: &str,
    is_last: bool,
    is_root: bool,
    stack: &mut Vec<u64>,
) {
    if stack.contains(&node.id) {
        let branch = if is_root {
            String::new()
        } else if is_last {
            format!("{prefix}└─ ")
        } else {
            format!("{prefix}├─ ")
        };
        out.push(format!("{branch}… #{} (cycle)", node.id));
        return;
    }

    let branch = if is_root {
        String::new()
    } else if is_last {
        format!("{prefix}└─ ")
    } else {
        format!("{prefix}├─ ")
    };
    out.push(format!("{branch}{}", format_task_row(node)));

    let kids: Vec<&TaskSnapshot> = node
        .blocks
        .iter()
        .filter_map(|id| by_id.get(id).copied())
        .collect();
    let extension = if is_root {
        String::new()
    } else if is_last {
        format!("{prefix}   ")
    } else {
        format!("{prefix}│  ")
    };

    stack.push(node.id);
    for (i, kid) in kids.iter().enumerate() {
        walk_tree(
            out,
            kid,
            by_id,
            &extension,
            i + 1 == kids.len(),
            false,
            stack,
        );
    }
    stack.pop();
}

/// Flat checklist (legacy helper / tests).
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
        let many: Vec<_> = (0..10).map(|i| snap(i, TaskStatusSnapshot::Pending)).collect();
        assert_eq!(sticky_height(false, &many), 1);
        assert_eq!(sticky_height(true, &many), 1 + many.len());
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
    fn tree_repeats_multiparent_child() {
        let mut a = snap(1, TaskStatusSnapshot::Completed);
        a.blocks = vec![3];
        let mut b = snap(2, TaskStatusSnapshot::Pending);
        b.blocks = vec![3];
        let mut c = snap(3, TaskStatusSnapshot::Pending);
        c.blocked_by = vec![1, 2];
        let lines = format_tree_lines(&[a, b, c]);
        let count3 = lines.iter().filter(|l| l.contains("#3")).count();
        assert!(count3 >= 2, "expected #3 under both parents, got:\n{}", lines.join("\n"));
    }

    #[test]
    fn format_tasks_log_card_short_diff() {
        let msgs = Messages::by_language(Language::English);
        let prev = vec![snap(1, TaskStatusSnapshot::InProgress)];
        let mut next = snap(1, TaskStatusSnapshot::Completed);
        next.subject = "后端接口".into();
        let text = format_tasks_log_card(
            &msgs,
            TasksChangeReason::Updated,
            &prev,
            &[next],
        );
        assert!(text.starts_with("📋 # Task.1 · 完成任务"), "got:\n{text}");
    }
}
