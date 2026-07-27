//! Human-readable titles for `task_*` tool rows and related UI strings.

use serde_json::Value;

use super::{SharedTaskManager, TaskRecord, TaskStatus};

/// True for the four persistent-task tools.
pub fn is_task_tool(name: &str) -> bool {
    matches!(
        name,
        "task_create" | "task_update" | "task_get" | "task_list"
    )
}

/// Build the TUI/tool title for a task tool call.
///
/// `before` is the record prior to mutation (update/get); ignored for create/list.
/// `after` is the post-mutation record when known (preferred for finished steps).
pub fn format_task_tool_title(
    name: &str,
    input: &Value,
    before: Option<&TaskRecord>,
    after: Option<&TaskRecord>,
) -> String {
    let record = after.or(before);
    let id = record
        .map(|r| r.id)
        .or_else(|| input.get("task_id").and_then(|v| v.as_u64()));

    let primary = primary_action(name, input, before, after);
    let mut parts: Vec<String> = Vec::new();

    let head = match id {
        Some(id) => format!("# Task.{id} · {primary}"),
        None => format!("# Task · {primary}"),
    };
    parts.push(head);

    let subject = record
        .map(|r| r.subject.as_str())
        .or_else(|| input.get("subject").and_then(|v| v.as_str()))
        .unwrap_or("");
    if !subject.is_empty() {
        parts.push(format!("subject: {subject}"));
    }

    let owner = record.map(|r| r.owner.as_str()).unwrap_or("");
    let owner_from_input = input.get("owner").and_then(|v| v.as_str());
    let owner_show = if !owner.is_empty() {
        owner
    } else {
        owner_from_input.unwrap_or("")
    };
    if !owner_show.is_empty() {
        parts.push(format!("owner:{owner_show}"));
    }

    let before_bb = before.map(|r| r.blocked_by.as_slice()).unwrap_or(&[]);
    let before_bl = before.map(|r| r.blocks.as_slice()).unwrap_or(&[]);
    let after_bb = record.map(|r| r.blocked_by.as_slice()).unwrap_or(before_bb);
    let after_bl = record.map(|r| r.blocks.as_slice()).unwrap_or(before_bl);

    // If we only have input intent (pre-exec update), predict after lists.
    let (bb_old, bb_new, bl_old, bl_new) = if after.is_none() && name == "task_update" {
        let mut pred_bb = before_bb.to_vec();
        let mut pred_bl = before_bl.to_vec();
        merge_ids(&mut pred_bb, input_u64_list(input, "addBlockedBy"));
        merge_ids(&mut pred_bl, input_u64_list(input, "addBlocks"));
        (before_bb.to_vec(), pred_bb, before_bl.to_vec(), pred_bl)
    } else {
        (
            before_bb.to_vec(),
            after_bb.to_vec(),
            before_bl.to_vec(),
            after_bl.to_vec(),
        )
    };

    if !bb_new.is_empty() || !bb_old.is_empty() {
        parts.push(format!(
            "blocked by: {}",
            format_id_transition(&bb_old, &bb_new)
        ));
    }
    if !bl_new.is_empty() || !bl_old.is_empty() {
        parts.push(format!(
            "blocks: {}",
            format_id_transition(&bl_old, &bl_new)
        ));
    }

    parts.join(" * ")
}

/// Resolve title using the live task manager (lookup before/after by id).
pub fn format_task_tool_title_with_manager(
    manager: &SharedTaskManager,
    name: &str,
    input: &Value,
    prefer_after: bool,
) -> String {
    let id = input.get("task_id").and_then(|v| v.as_u64());
    let before = id.and_then(|id| manager.get(id).ok());
    let after = if prefer_after {
        id.and_then(|id| manager.get(id).ok()).or_else(|| {
            // create: try newest matching subject
            if name == "task_create" {
                let subject = input.get("subject").and_then(|v| v.as_str())?;
                let list = manager.list().ok()?;
                list.into_iter()
                    .filter(|t| t.subject == subject)
                    .max_by_key(|t| t.id)
            } else {
                None
            }
        })
    } else {
        None
    };
    format_task_tool_title(
        name,
        input,
        before.as_ref(),
        if prefer_after {
            after.as_ref().or(before.as_ref())
        } else {
            None
        },
    )
}

fn primary_action(
    name: &str,
    input: &Value,
    before: Option<&TaskRecord>,
    after: Option<&TaskRecord>,
) -> &'static str {
    match name {
        "task_create" => "create",
        "task_list" => "list",
        "task_get" => "view",
        "task_update" => {
            let status =
                input
                    .get("status")
                    .and_then(|v| v.as_str())
                    .or_else(|| match (before, after) {
                        (Some(b), Some(a)) if b.status != a.status => Some(status_str(a.status)),
                        (None, Some(a)) => Some(status_str(a.status)),
                        _ => None,
                    });
            if let Some(s) = status {
                return match s {
                    "in_progress" => "execute",
                    "completed" => "complete",
                    "pending" => "reset",
                    "deleted" => "delete",
                    _ => "update",
                };
            }
            if input.get("owner").and_then(|v| v.as_str()).is_some() {
                return "set owner";
            }
            let add_bb = input_u64_list(input, "addBlockedBy");
            let add_bl = input_u64_list(input, "addBlocks");
            if !add_bb.is_empty() {
                return "blocked by";
            }
            if !add_bl.is_empty() {
                return "blocks";
            }
            // Compare before/after if present
            if let (Some(b), Some(a)) = (before, after) {
                if b.status != a.status {
                    return match a.status {
                        TaskStatus::InProgress => "execute",
                        TaskStatus::Completed => "complete",
                        TaskStatus::Pending => "reset",
                        TaskStatus::Deleted => "delete",
                    };
                }
                if b.owner != a.owner {
                    return "set owner";
                }
                if b.blocked_by != a.blocked_by {
                    return "blocked by";
                }
                if b.blocks != a.blocks {
                    return "blocks";
                }
            }
            "no-op"
        }
        _ => "update",
    }
}

fn status_str(status: TaskStatus) -> &'static str {
    match status {
        TaskStatus::Pending => "pending",
        TaskStatus::InProgress => "in_progress",
        TaskStatus::Completed => "completed",
        TaskStatus::Deleted => "deleted",
    }
}

fn input_u64_list(input: &Value, key: &str) -> Vec<u64> {
    input
        .get(key)
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|x| x.as_u64()).collect())
        .unwrap_or_default()
}

fn merge_ids(target: &mut Vec<u64>, additions: Vec<u64>) {
    target.extend(additions);
    target.sort_unstable();
    target.dedup();
}

pub fn format_id_list(ids: &[u64]) -> String {
    let inner = ids
        .iter()
        .map(|id| id.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    format!("[{inner}]")
}

pub fn format_id_transition(before: &[u64], after: &[u64]) -> String {
    if before == after {
        format_id_list(after)
    } else {
        format!("{} -> {}", format_id_list(before), format_id_list(after))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_title_without_id() {
        let input = serde_json::json!({"subject": "初始化"});
        let title = format_task_tool_title("task_create", &input, None, None);
        assert_eq!(title, "# Task · create * subject: 初始化");
    }

    #[test]
    fn update_complete_with_dep_transition() {
        let before = TaskRecord {
            id: 24,
            subject: "后端接口".into(),
            description: None,
            status: TaskStatus::InProgress,
            blocked_by: vec![12],
            blocks: vec![20],
            owner: "张2".into(),
            created_at: None,
            started_at: None,
            completed_at: None,
        };
        let after = TaskRecord {
            blocked_by: vec![12, 24],
            status: TaskStatus::Completed,
            ..before.clone()
        };
        let input = serde_json::json!({"task_id": 24, "status": "completed"});
        let title = format_task_tool_title("task_update", &input, Some(&before), Some(&after));
        assert!(title.starts_with("# Task.24 · complete"), "{title}");
        assert!(title.contains("subject: 后端接口"), "{title}");
        assert!(title.contains("owner:张2"), "{title}");
        assert!(title.contains("blocked by: [12] -> [12, 24]"), "{title}");
        assert!(title.contains("blocks: [20]"), "{title}");
    }

    #[test]
    fn empty_update_action() {
        let before = TaskRecord::new(1, "x".into(), None);
        let input = serde_json::json!({"task_id": 1});
        let title = format_task_tool_title("task_update", &input, Some(&before), Some(&before));
        assert!(title.contains("no-op"), "{title}");
    }
}
