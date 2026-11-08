//! Pure folding rules for the transcript's activity groups.
//!
//! The desktop transcript collapses a run of thinking, commentary and tool
//! rows into one summary. These decisions do not need GPUI entities, which
//! keeps the grouping rules independent from the shell that paints them.

use std::sync::Arc;

use tact_catalog::CatalogLocale;
use tact_desktop_core::{
    GateState, MessagePhase, PlanRow, RowBody, RowStatus, ToolGroupItem, TranscriptRow,
    TranscriptView,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProcessRowRole {
    Normal,
    Summary(usize),
    Hidden(usize),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcessGroup {
    pub first_index: usize,
    pub end_index: usize,
    pub first_row: tact_desktop_core::RowId,
    pub first_ordinal: u64,
    pub commands: usize,
    pub files_read: usize,
    pub files_changed: usize,
    pub skills: usize,
    pub other_tools: usize,
    pub thoughts: usize,
    pub active: bool,
}

/// Rows the answers-only view keeps: the user's words, assistant text that is
/// not narration, decisions still waiting, and errors.
pub fn answers_only_visible(body: &RowBody) -> bool {
    match body {
        RowBody::User(_) | RowBody::Error(_) => true,
        RowBody::Assistant(assistant) => assistant.phase != Some(MessagePhase::Commentary),
        RowBody::Gate(gate) => matches!(gate.status, GateState::Pending | GateState::Submitting),
        RowBody::ToolGroup(group) => group.items.iter().any(|item| {
            matches!(
                item,
                ToolGroupItem::Gate(gate)
                    if matches!(gate.status, GateState::Pending | GateState::Submitting)
            )
        }),
        _ => false,
    }
}

pub fn is_process_row(body: &RowBody) -> bool {
    match body {
        RowBody::Thinking(_) | RowBody::ToolGroup(_) => true,
        RowBody::Assistant(assistant) => assistant.phase == Some(MessagePhase::Commentary),
        _ => false,
    }
}

/// Whether a row's content starts at the message column rather than at the
/// transcript's left edge.
///
/// The design's row grid gives every row a 31px avatar column and a 12px gap;
/// user and assistant rows draw a mark there, while plan, thinking, tool,
/// diff, gate and notice rows are content that belongs beside the message.
pub fn row_starts_at_message_column(body: &RowBody) -> bool {
    !matches!(body, RowBody::User(_) | RowBody::Assistant(_))
}

/// Whether a plan row only repeats tool calls already visible in this run.
///
/// The runtime emits a plan step for every concrete tool call. Drawing that
/// plan beside the execution group duplicates every command; the tool rows are
/// the richer surface, so the plan yields when all of its steps have a
/// `tool_id` present in the same run. A plan with a conceptual step (no tool
/// id) keeps its card because the tool group cannot represent that step.
pub fn plan_is_tool_execution(
    row: &TranscriptRow,
    plan: &PlanRow,
    transcript: &TranscriptView,
) -> bool {
    if plan.steps.is_empty() {
        return false;
    }
    let rows: Vec<&TranscriptRow> = transcript.iter().map(|row| row.as_ref()).collect();
    let Some(index) = rows.iter().position(|candidate| candidate.id == row.id) else {
        return false;
    };
    // One run owns one execution group, and a later turn in that run gets a
    // fresh loop id while appending to the same group. Compare within the run
    // (the rows between the user turns that surround the plan) rather than by
    // loop id, so the group still absorbs its plan.
    let start = rows[..index]
        .iter()
        .rposition(|candidate| matches!(&candidate.body, RowBody::User(_)))
        .map(|position| position + 1)
        .unwrap_or(0);
    let end = rows[index..]
        .iter()
        .position(|candidate| matches!(&candidate.body, RowBody::User(_)))
        .map(|offset| index + offset)
        .unwrap_or(rows.len());
    let mut run_tool_ids = std::collections::HashSet::new();
    for candidate in &rows[start..end] {
        let RowBody::ToolGroup(group) = &candidate.body else {
            continue;
        };
        for item in group.items.iter() {
            if let ToolGroupItem::Tool(tool) = item {
                run_tool_ids.insert(tool.tool_id.as_ref());
            }
        }
    }
    plan.steps.iter().all(|step| {
        step.tool_id
            .as_deref()
            .is_some_and(|tool_id| run_tool_ids.contains(tool_id))
    })
}

/// Build one fold layout for the transcript's current row sequence.
pub fn build_process_layout(
    rows: &[Arc<TranscriptRow>],
) -> (Vec<ProcessRowRole>, Vec<ProcessGroup>) {
    let transcript = TranscriptView::from_rows(rows.to_vec());
    let mut roles = vec![ProcessRowRole::Normal; rows.len()];
    let mut groups = Vec::new();
    let mut index = 0;
    while index < rows.len() {
        if !process_row_groupable(&rows[index].body) {
            index += 1;
            continue;
        }
        let start = index;
        let mut commands = 0;
        let mut files_read = 0;
        let mut files_changed = 0;
        let mut skills = 0;
        let mut other_tools = 0;
        let mut thoughts = 0;
        let mut active = false;
        while index < rows.len() {
            let row = &rows[index];
            if !process_row_groupable(&row.body) {
                if process_row_transparent(row, &transcript) {
                    index += 1;
                    continue;
                }
                break;
            }
            match &row.body {
                RowBody::Thinking(thinking) => {
                    thoughts += 1;
                    active |= matches!(thinking.status, RowStatus::Running | RowStatus::Pending);
                }
                RowBody::ToolGroup(group) => {
                    active |= matches!(group.status(), RowStatus::Running | RowStatus::Pending)
                        || group.items.iter().any(|item| {
                            matches!(
                                item,
                                ToolGroupItem::Gate(gate)
                                    if matches!(
                                        gate.status,
                                        GateState::Pending | GateState::Submitting
                                    )
                            )
                        });
                    for item in group.items.iter() {
                        let ToolGroupItem::Tool(tool) = item else {
                            continue;
                        };
                        match process_tool_kind(&tool.name) {
                            ProcessToolKind::Command => commands += 1,
                            ProcessToolKind::FileRead => files_read += 1,
                            ProcessToolKind::FileChanged => files_changed += 1,
                            ProcessToolKind::Skill => skills += 1,
                            ProcessToolKind::Other => other_tools += 1,
                        }
                    }
                }
                // Narration is part of the same process region and adds no count.
                RowBody::Assistant(_) => {}
                _ => break,
            }
            index += 1;
        }
        if index - start >= 2 {
            let group_index = groups.len();
            let first = &rows[start];
            roles[start] = ProcessRowRole::Summary(group_index);
            for (offset, role) in roles[start + 1..index].iter_mut().enumerate() {
                let body = &rows[start + 1 + offset].body;
                if !process_row_visible_when_folded(body) {
                    *role = ProcessRowRole::Hidden(group_index);
                }
            }
            groups.push(ProcessGroup {
                first_index: start,
                end_index: index,
                first_row: first.id,
                first_ordinal: first.ordinal,
                commands,
                files_read,
                files_changed,
                skills,
                other_tools,
                thoughts,
                active,
            });
        }
    }
    (roles, groups)
}

pub fn process_summary_text(group: &ProcessGroup, language: CatalogLocale) -> String {
    let mut parts = Vec::new();
    if group.thoughts > 0 {
        parts.push(match language {
            CatalogLocale::En if group.thoughts == 1 => "Thoughts".to_string(),
            CatalogLocale::En => format!("{} thoughts", group.thoughts),
            CatalogLocale::Zh => format!("{} 段思考", group.thoughts),
        });
    }
    match language {
        CatalogLocale::En => {
            if group.commands > 0 {
                parts.push(if group.commands == 1 {
                    "ran 1 command".to_string()
                } else {
                    format!("ran {} commands", group.commands)
                });
            }
            if group.files_read > 0 {
                parts.push(if group.files_read == 1 {
                    "read 1 file".to_string()
                } else {
                    format!("read {} files", group.files_read)
                });
            }
            if group.files_changed > 0 {
                parts.push(if group.files_changed == 1 {
                    "edited 1 file".to_string()
                } else {
                    format!("edited {} files", group.files_changed)
                });
            }
            if group.skills > 0 {
                parts.push(if group.skills == 1 {
                    "ran 1 skill".to_string()
                } else {
                    format!("ran {} skills", group.skills)
                });
            }
            if group.other_tools > 0 {
                parts.push(if group.other_tools == 1 {
                    "1 tool call".to_string()
                } else {
                    format!("{} tool calls", group.other_tools)
                });
            }
        }
        CatalogLocale::Zh => {
            if group.commands > 0 {
                parts.push(format!("运行了 {} 条命令", group.commands));
            }
            if group.files_read > 0 {
                parts.push(format!("读取了 {} 个文件", group.files_read));
            }
            if group.files_changed > 0 {
                parts.push(format!("修改了 {} 个文件", group.files_changed));
            }
            if group.skills > 0 {
                parts.push(format!("运行了 {} 个 skill", group.skills));
            }
            if group.other_tools > 0 {
                parts.push(format!("{} 次工具调用", group.other_tools));
            }
        }
    }
    if parts.is_empty() {
        return match language {
            CatalogLocale::En => "Activity".into(),
            CatalogLocale::Zh => "活动".into(),
        };
    }
    let summary = parts.join(match language {
        CatalogLocale::En => ", ",
        CatalogLocale::Zh => "，",
    });
    let mut chars = summary.chars();
    let Some(first) = chars.next() else {
        return summary;
    };
    first.to_uppercase().chain(chars).collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ProcessToolKind {
    Command,
    FileRead,
    FileChanged,
    Skill,
    Other,
}

pub fn process_row_groupable(body: &RowBody) -> bool {
    match body {
        RowBody::Thinking(_) | RowBody::ToolGroup(_) => true,
        RowBody::Assistant(assistant) => assistant.phase == Some(MessagePhase::Commentary),
        _ => false,
    }
}

/// A pending decision stays outside the fold so it can be answered. It is
/// still part of the activity group: its tools count toward the summary and
/// its gate keeps the group's clock running.
fn process_row_visible_when_folded(body: &RowBody) -> bool {
    matches!(
        body,
        RowBody::ToolGroup(group)
            if group.items.iter().any(|item| {
                matches!(
                    item,
                    ToolGroupItem::Gate(gate)
                        if matches!(gate.status, GateState::Pending | GateState::Submitting)
                )
            })
    )
}

/// Rows the renderer already drops must not split an activity group.
///
/// The runtime emits a tool-only plan row for every concrete step. The plan
/// card yields to its tool group, so treating it as a boundary would turn one
/// reasoning region into one summary per tool call.
fn process_row_transparent(row: &TranscriptRow, transcript: &TranscriptView) -> bool {
    matches!(&row.body, RowBody::Plan(plan) if plan_is_tool_execution(row, plan, transcript))
}

fn process_tool_kind(name: &str) -> ProcessToolKind {
    let name = name.to_ascii_lowercase();
    if ["bash", "shell", "exec", "command", "terminal", "cmd"]
        .iter()
        .any(|kind| name.contains(kind))
    {
        ProcessToolKind::Command
    } else if [
        "read",
        "cat",
        "view",
        "open_file",
        "list_file",
        "ls",
        "glob",
    ]
    .iter()
    .any(|kind| name.contains(kind))
    {
        ProcessToolKind::FileRead
    } else if [
        "edit", "write", "patch", "create", "delete", "move", "rename", "replace",
    ]
    .iter()
    .any(|kind| name.contains(kind))
    {
        ProcessToolKind::FileChanged
    } else if name.contains("skill") {
        ProcessToolKind::Skill
    } else {
        ProcessToolKind::Other
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_names_fold_into_the_summary_categories() {
        assert_eq!(process_tool_kind("exec_command"), ProcessToolKind::Command);
        assert_eq!(process_tool_kind("read_file"), ProcessToolKind::FileRead);
        assert_eq!(
            process_tool_kind("apply_patch"),
            ProcessToolKind::FileChanged
        );
        assert_eq!(process_tool_kind("load_skill"), ProcessToolKind::Skill);
        assert_eq!(process_tool_kind("web_search"), ProcessToolKind::Other);
    }

    #[test]
    fn an_empty_process_group_still_has_a_label() {
        let group = ProcessGroup {
            first_index: 0,
            end_index: 0,
            first_row: tact_desktop_core::RowId::new(1),
            first_ordinal: 1,
            commands: 0,
            files_read: 0,
            files_changed: 0,
            skills: 0,
            other_tools: 0,
            thoughts: 0,
            active: false,
        };
        assert_eq!(process_summary_text(&group, CatalogLocale::En), "Activity");
        assert_eq!(process_summary_text(&group, CatalogLocale::Zh), "活动");
    }
}
