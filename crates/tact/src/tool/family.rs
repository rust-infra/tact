//! The built-in tool families, as data rather than as a hand-written chain.
//!
//! Every built-in tool belongs to exactly one family. The family table is what
//! the composition root installs; `toolset()` is a fold over it. That makes the
//! set of built-ins a value a test can inspect (and a caller can filter) instead
//! of a chain of `.route(..)` calls whose order is invisible.

use anyhow::Result;

use super::{
    Tool, ToolRouter,
    ask_user::AskUserTool,
    background_run::{BackgroundRunTool, CheckBackgroundTool, WaitBackgroundTool},
    bash::BashTool,
    compact::CompactTool,
    edit_file::EditFileTool,
    goal::{GoalCheckpointTool, GoalProposeCompletionTool, GoalReadTool, GoalRequestInputTool},
    load_skill::LoadSkillTool,
    memory::{LoadMemoryTool, SaveMemoryTool},
    read_file::ReadFileTool,
    read_image::ReadImageTool,
    sleep::SleepTool,
    subagent::{CancelSubagentTool, CheckSubagentTool, SpawnSubagentTool, WaitSubagentTool},
    task::{TaskCreateTool, TaskGetTool, TaskListTool, TaskUpdateTool},
    team::{
        BroadcastTool, ListTeammatesTool, PlanApprovalTool, ReadInboxTool, SendMessageTool,
        ShutdownRequestTool, ShutdownResponseTool, SpawnTeammateTool,
    },
    worktree::{
        WorktreeCreateTool, WorktreeEventsTool, WorktreeListTool, WorktreeRemoveTool,
        WorktreeRunTool, WorktreeStatusTool,
    },
    write_file::WriteFileTool,
};

/// One family of built-in tools.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ToolFamily {
    /// The shell and the clock: the tools whose resources are whole-process.
    Shell,
    /// Reading and writing files.
    Files,
    /// Asking the user, and compaction.
    Interaction,
    /// Durable tasks.
    Task,
    /// Bounded goals.
    Goal,
    /// Teammates and messaging.
    Team,
    /// Isolated worktrees.
    Worktree,
    /// Detached background commands.
    Background,
    /// Child agents.
    Subagent,
    /// Persistent memory.
    Memory,
    /// Skill discovery.
    Skill,
    /// The restricted set a child agent gets.
    ///
    /// Deliberately its own family rather than `Shell + Files`: a child must not
    /// get `read_image` (a large payload into a child's context) and the list is
    /// meant to stay auditable in one place. Not part of [`Self::ALL`], which is
    /// the main agent's set.
    SubagentBasics,
}

impl ToolFamily {
    /// Every family, in installation order.
    ///
    /// Order is load-bearing only for the emitted spec list, which the model
    /// reads top-down; it does not affect dispatch.
    pub const ALL: [Self; 11] = [
        Self::Shell,
        Self::Files,
        Self::Interaction,
        Self::Task,
        Self::Goal,
        Self::Team,
        Self::Worktree,
        Self::Background,
        Self::Subagent,
        Self::Memory,
        Self::Skill,
    ];

    pub fn id(self) -> &'static str {
        match self {
            Self::Shell => "builtin.tools.shell",
            Self::Files => "builtin.tools.files",
            Self::Interaction => "builtin.tools.interaction",
            Self::Task => "builtin.tools.task",
            Self::Goal => "builtin.tools.goal",
            Self::Team => "builtin.tools.team",
            Self::Worktree => "builtin.tools.worktree",
            Self::Background => "builtin.tools.background",
            Self::Subagent => "builtin.tools.subagent",
            Self::Memory => "builtin.tools.memory",
            Self::Skill => "builtin.tools.skill",
            Self::SubagentBasics => "builtin.tools.subagent_basics",
        }
    }

    /// Construct this family's handlers.
    ///
    /// `memory_enabled` mirrors `[agent].memory_enabled`: the memory family
    /// contributes nothing when it is off, which is what makes those tools
    /// disappear from the advertised list rather than fail at dispatch.
    pub fn handlers(self, memory_enabled: bool) -> Vec<Box<dyn Tool>> {
        match self {
            Self::Shell => vec![Box::new(BashTool), Box::new(SleepTool)],
            Self::Files => vec![
                Box::new(ReadFileTool),
                Box::new(ReadImageTool),
                Box::new(WriteFileTool),
                Box::new(EditFileTool),
            ],
            Self::Interaction => vec![Box::new(AskUserTool), Box::new(CompactTool)],
            Self::Task => vec![
                Box::new(TaskCreateTool),
                Box::new(TaskGetTool),
                Box::new(TaskListTool),
                Box::new(TaskUpdateTool),
            ],
            Self::Goal => vec![
                Box::new(GoalReadTool),
                Box::new(GoalCheckpointTool),
                Box::new(GoalRequestInputTool),
                Box::new(GoalProposeCompletionTool),
            ],
            Self::Team => vec![
                Box::new(SpawnTeammateTool),
                Box::new(ListTeammatesTool),
                Box::new(SendMessageTool),
                Box::new(BroadcastTool),
                Box::new(ReadInboxTool),
                Box::new(PlanApprovalTool),
                Box::new(ShutdownRequestTool),
                Box::new(ShutdownResponseTool),
            ],
            Self::Worktree => vec![
                Box::new(WorktreeCreateTool),
                Box::new(WorktreeListTool),
                Box::new(WorktreeStatusTool),
                Box::new(WorktreeRunTool),
                Box::new(WorktreeRemoveTool),
                Box::new(WorktreeEventsTool),
            ],
            Self::Background => vec![
                Box::new(BackgroundRunTool),
                Box::new(CheckBackgroundTool),
                Box::new(WaitBackgroundTool),
            ],
            Self::Subagent => vec![
                Box::new(SpawnSubagentTool),
                Box::new(CheckSubagentTool),
                Box::new(WaitSubagentTool),
                Box::new(CancelSubagentTool),
            ],
            Self::Memory => {
                if memory_enabled {
                    vec![Box::new(SaveMemoryTool), Box::new(LoadMemoryTool)]
                } else {
                    Vec::new()
                }
            }
            Self::Skill => vec![Box::new(LoadSkillTool)],
            Self::SubagentBasics => vec![
                Box::new(BashTool),
                Box::new(ReadFileTool),
                Box::new(SleepTool),
                Box::new(WriteFileTool),
                Box::new(EditFileTool),
            ],
        }
    }
}

/// The restricted family set a child agent gets.
///
/// A subagent's tool list is deliberately not the parent's: it cannot spawn
/// another subagent, cannot create tasks, and has no Goal tools (it would be
/// asked to call `goal_checkpoint` for a Goal it does not own).
pub const SUBAGENT_FAMILIES: [ToolFamily; 1] = [ToolFamily::SubagentBasics];

/// Assemble a router from a family list.
pub fn router_from(families: &[ToolFamily], memory_enabled: bool) -> Result<ToolRouter> {
    let mut router = ToolRouter::new();
    for family in families {
        for handler in family.handlers(memory_enabled) {
            router = router.route_boxed(handler)?;
        }
    }
    Ok(router)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(families: &[ToolFamily], memory_enabled: bool) -> Vec<String> {
        let mut names: Vec<String> = router_from(families, memory_enabled)
            .unwrap()
            .tool_specs()
            .into_iter()
            .map(|spec| spec.name)
            .collect();
        names.sort();
        names
    }

    #[test]
    fn the_family_table_reproduces_the_built_in_tool_set() {
        // The composition used to be a chain of `.route(..)` calls. This pins the
        // family table to exactly that set, so reordering or dropping a tool is a
        // test failure rather than a silently smaller tool list.
        let expected = [
            "ask_user",
            "bash",
            "background_run",
            "check_background",
            "wait_background",
            "read_file",
            "read_image",
            "sleep",
            "write_file",
            "edit_file",
            "load_skill",
            "goal_read",
            "goal_checkpoint",
            "goal_request_input",
            "goal_propose_completion",
            "save_memory",
            "load_memory",
            "compact",
            "spawn_subagent",
            "check_subagent",
            "wait_subagent",
            "cancel_subagent",
            "task_create",
            "task_get",
            "task_list",
            "task_update",
            "spawn_teammate",
            "list_teammates",
            "send_message",
            "broadcast",
            "read_inbox",
            "plan_approval",
            "shutdown_request",
            "shutdown_response",
            "worktree_create",
            "worktree_list",
            "worktree_status",
            "worktree_run",
            "worktree_remove",
            "worktree_events",
        ];
        let mut expected: Vec<String> = expected.iter().map(|name| (*name).to_string()).collect();
        expected.sort();
        assert_eq!(names(&ToolFamily::ALL, true), expected);
    }

    #[test]
    fn memory_family_is_absent_when_memory_is_off() {
        let without = names(&ToolFamily::ALL, false);
        assert!(!without.iter().any(|name| name == "save_memory"));
        assert!(!without.iter().any(|name| name == "load_memory"));
        assert_eq!(without.len(), names(&ToolFamily::ALL, true).len() - 2);
    }

    #[test]
    fn subagent_families_are_the_five_restricted_tools() {
        let mut expected = vec![
            "bash".to_string(),
            "read_file".to_string(),
            "sleep".to_string(),
            "write_file".to_string(),
            "edit_file".to_string(),
        ];
        expected.sort();
        assert_eq!(names(&SUBAGENT_FAMILIES, true), expected);
    }

    #[test]
    fn every_family_id_is_unique() {
        let ids: Vec<&str> = ToolFamily::ALL.iter().map(|family| family.id()).collect();
        let unique: std::collections::BTreeSet<&str> = ids.iter().copied().collect();
        assert_eq!(ids.len(), unique.len());
    }

    #[test]
    fn families_do_not_share_a_tool_name() {
        let mut seen = std::collections::BTreeSet::new();
        for family in ToolFamily::ALL {
            for handler in family.handlers(true) {
                let name = handler.metadata().name;
                assert!(seen.insert(name), "{name} is contributed twice");
            }
        }
    }
}
