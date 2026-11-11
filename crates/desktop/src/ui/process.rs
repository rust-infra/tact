//! Pure folding rules for the transcript's activity groups.
//!
//! The desktop transcript collapses a run of thinking, commentary and tool
//! rows into one summary. These decisions do not need GPUI entities, which
//! keeps the grouping rules independent from the shell that paints them.

use std::collections::HashSet;
use std::sync::Arc;

use tact_catalog::CatalogLocale;
use tact_desktop_core::{
    GateState, MessagePhase, PlanRow, RowBody, RowId, RowStatus, ToolGroupItem, TranscriptRow,
    TranscriptView,
};

/// How one row participates in an activity fold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProcessRowRole {
    /// Painted as-is; no group owns this row.
    Normal,
    /// Painted as the group's collapsed summary line.
    Summary(usize),
    /// Folded away behind the group's summary.
    Hidden(usize),
}

/// One run of thinking, narration and tool rows, collapsed behind a summary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcessGroup {
    pub first_index: usize,
    pub end_index: usize,
    pub first_row: RowId,
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

/// Whether a row belongs to an activity group: thinking, tool calls, and
/// narration that sits between them.
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
///
/// `run_tool_ids` is the set of tool ids in the run that contains `plan`, as
/// returned by [`run_tool_ids`]. Keeping the scan out of this predicate lets
/// the caller pay for it once per layout instead of once per painted row.
pub fn plan_is_tool_execution(plan: &PlanRow, run_tool_ids: &HashSet<&str>) -> bool {
    !plan.steps.is_empty()
        && plan.steps.iter().all(|step| {
            step.tool_id
                .as_deref()
                .is_some_and(|tool_id| run_tool_ids.contains(tool_id))
        })
}

/// The tool ids inside one run, as a set of borrowed names.
fn run_tool_ids(run: &[Arc<TranscriptRow>]) -> HashSet<&str> {
    let mut tool_ids = HashSet::new();
    for row in run {
        let RowBody::ToolGroup(group) = &row.body else {
            continue;
        };
        for item in group.items.iter() {
            if let ToolGroupItem::Tool(tool) = item {
                tool_ids.insert(tool.tool_id.as_ref());
            }
        }
    }
    tool_ids
}

/// Plan rows whose card yields to the run's tool group, keyed by row id.
///
/// The transcript is split into runs at its user turns, each run's tool ids are
/// collected once, and every plan in that run is tested against them — so this
/// is one pass over the rows. The renderer and the fold both read the result
/// instead of rescanning the transcript for every plan they meet.
///
/// One run owns one execution group, and a later turn in that run gets a fresh
/// loop id while appending to the same group. Runs are therefore delimited by
/// user turns rather than by loop id, so a group still absorbs its plan.
pub fn yielded_plans(rows: &[Arc<TranscriptRow>]) -> HashSet<RowId> {
    let mut yielded = HashSet::new();
    let mut run_start = 0;
    while run_start < rows.len() {
        let run_end = rows[run_start..]
            .iter()
            .position(|row| matches!(&row.body, RowBody::User(_)))
            .map(|offset| run_start + offset)
            .unwrap_or(rows.len());
        let run = &rows[run_start..run_end];
        let tool_ids = run_tool_ids(run);
        for row in run {
            let RowBody::Plan(plan) = &row.body else {
                continue;
            };
            if plan_is_tool_execution(plan, &tool_ids) {
                yielded.insert(row.id);
            }
        }
        // The user row at `run_end` opens the next run; step past it.
        run_start = run_end + 1;
    }
    yielded
}

/// [`yielded_plans`] for a transcript view, collecting its rows once.
pub fn yielded_plans_of(view: &TranscriptView) -> HashSet<RowId> {
    let rows = view.iter().cloned().collect::<Vec<_>>();
    yielded_plans(&rows)
}

/// Build one fold layout for the transcript's current row sequence.
pub fn build_process_layout(rows: &[Arc<TranscriptRow>]) -> ProcessLayout {
    let yielded = yielded_plans(rows);
    let mut roles = vec![ProcessRowRole::Normal; rows.len()];
    let mut groups = Vec::new();
    let mut index = 0;
    while index < rows.len() {
        if !is_process_row(&rows[index].body) {
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
            if !is_process_row(&row.body) {
                if process_row_transparent(row, &yielded) {
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
    ProcessLayout {
        roles,
        groups,
        yielded_plans: yielded,
    }
}

/// One layout pass over the transcript's rows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProcessLayout {
    /// How each row participates in the fold, in row order.
    pub roles: Vec<ProcessRowRole>,
    /// The activity groups, in row order.
    pub groups: Vec<ProcessGroup>,
    /// Plan rows whose card yields to the run's tool group.
    pub yielded_plans: HashSet<RowId>,
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
fn process_row_transparent(row: &TranscriptRow, yielded_plans: &HashSet<RowId>) -> bool {
    matches!(&row.body, RowBody::Plan(_)) && yielded_plans.contains(&row.id)
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

    /// Paired with `reducer::settle_final_answer`: a trailing answer the
    /// provider labeled `Commentary` is promoted to `FinalAnswer`, so the fold
    /// keeps it visible. This pins the fold side of that contract.
    #[test]
    fn an_answer_after_the_tool_group_stays_out_of_the_fold() {
        use tact_desktop_core::transcript::AssistantRow;
        let demo = tact_desktop_core::fixtures::demo_transcript();
        let group = demo
            .iter()
            .find(|row| matches!(&row.body, RowBody::ToolGroup(_)))
            .cloned()
            .expect("demo tool group");
        let mut answer = (*demo
            .iter()
            .find(|row| matches!(&row.body, RowBody::Assistant(_)))
            .cloned()
            .expect("demo assistant"))
        .clone();
        let set = |row: &mut TranscriptRow, phase: Option<MessagePhase>| {
            row.body = RowBody::Assistant(AssistantRow {
                markdown: Arc::from("checked it, all tests pass"),
                status: RowStatus::Success,
                suggestions: Arc::from([]),
                phase,
            });
        };
        set(&mut answer, Some(MessagePhase::FinalAnswer));
        let settled = [group.clone(), Arc::new(answer.clone())];
        assert_eq!(
            build_process_layout(&settled).roles[1],
            ProcessRowRole::Normal,
            "the settled answer stands on its own"
        );

        set(&mut answer, Some(MessagePhase::Commentary));
        let stale = [group, Arc::new(answer)];
        assert_eq!(
            build_process_layout(&stale).roles[1],
            ProcessRowRole::Hidden(0),
            "an unsettled label would hide it, which is why the reducer settles first"
        );
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
