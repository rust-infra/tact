//! `/goal` — list, or show one.
//!
//! The projection lives in [`crate::goal::snapshots`], which is why this command
//! can exist at all: it used to sit in the frontend, so a command could not reach
//! it without the extension crate depending on the view.
//!
//! Only the read verbs are here. `create`/`resume`/`pause`/`cancel` drive the
//! Goal runner, which owns the agent loop; they stay on the driver's side until
//! the loop is a plugin.

use std::sync::Arc;

use tact_kernel::capability::command::{
    Command, CommandInvocation, CommandMeta, CommandOutcome, CommandScope, Emitted,
};
use tact_protocol::AgentUpdate;

use crate::goal::GoalLimits;
use crate::goal::SharedGoalManager;
use crate::task::SharedTaskManager;

/// What `/goal` was asked to do.
#[derive(Debug, Clone, PartialEq, Eq)]
enum GoalVerb {
    /// `/goal` or `/goal list` — the panel's projection.
    List { include_archived: bool },
    /// `/goal show <id>` — one Goal, as text for the reader.
    Show { id: String },
}

/// The `/goal` command.
pub struct GoalCommand {
    goal_manager: SharedGoalManager,
    task_manager: SharedTaskManager,
    limits: GoalLimits,
    meta: CommandMeta,
}

impl GoalCommand {
    pub fn new(
        goal_manager: SharedGoalManager,
        task_manager: SharedTaskManager,
        limits: GoalLimits,
    ) -> Self {
        Self {
            goal_manager,
            task_manager,
            limits,
            meta: CommandMeta {
                name: "goal".to_string(),
                aliases: Vec::new(),
                summary: "List Goals, or show one".to_string(),
                usage: "/goal [list [--all] | show <id>]".to_string(),
                scope: CommandScope::Runtime,
                // Verbs follow, so Enter completes instead of running.
                needs_args: true,
            },
        }
    }

    /// Parse the verb.
    ///
    /// The grammar is the command's own — the kernel routes and never parses —
    /// so an unknown verb is answered with the usage this command declares.
    fn parse(&self, args: &str) -> Result<GoalVerb, String> {
        let mut words = args.split_whitespace();
        match words.next() {
            None | Some("list") => {
                let rest: Vec<&str> = words.collect();
                match rest.as_slice() {
                    // A bare `/goal list` shows live Goals; archived ones are
                    // deliberately off by default (they would otherwise keep
                    // taking rows in the panel).
                    [] => Ok(GoalVerb::List {
                        include_archived: false,
                    }),
                    ["--all"] | ["-a"] => Ok(GoalVerb::List {
                        include_archived: true,
                    }),
                    [other, ..] => Err(format!("`/goal list` takes only `--all` (got `{other}`)")),
                }
            }
            Some("show") => match (words.next(), words.next()) {
                (Some(id), None) => Ok(GoalVerb::Show { id: id.to_string() }),
                (None, _) => Err("`/goal show` needs an id".to_string()),
                (Some(_), Some(extra)) => {
                    Err(format!("`/goal show` takes one id (got extra `{extra}`)"))
                }
            },
            Some(other) => Err(format!("unknown verb `{other}`; expected `list` or `show`")),
        }
    }
}

#[async_trait::async_trait]
impl Command for GoalCommand {
    fn meta(&self) -> &CommandMeta {
        &self.meta
    }

    async fn run_async(&self, invocation: &CommandInvocation) -> CommandOutcome {
        let verb = match self.parse(&invocation.args) {
            Ok(verb) => verb,
            Err(message) => {
                return CommandOutcome::Usage(format!("{message}\n{}", self.meta.usage));
            }
        };
        match verb {
            GoalVerb::List { include_archived } => {
                match crate::goal::snapshots::snapshots(
                    &self.goal_manager,
                    &self.task_manager,
                    self.limits,
                    include_archived,
                )
                .await
                {
                    // The panel is driven by the projection, not by text: a
                    // command that flattened this into a string would make the
                    // host parse it back into the shape it already had.
                    Ok(goals) => {
                        CommandOutcome::Emit(Emitted::new(AgentUpdate::GoalsChanged { goals }))
                    }
                    Err(error) => {
                        CommandOutcome::Failed(format!("Goal snapshot failed: {error:#}"))
                    }
                }
            }
            GoalVerb::Show { id } => {
                // `show` reaches *any* Goal, archived ones included — that is what
                // keeps archiving from being a dead end. It answers with the
                // detail popup only: the panel refresh is the caller's `list`,
                // because a command outcome carries one update and this one is
                // the answer the user asked for.
                let goals = match crate::goal::snapshots::snapshots(
                    &self.goal_manager,
                    &self.task_manager,
                    self.limits,
                    true,
                )
                .await
                {
                    Ok(goals) => goals,
                    Err(error) => {
                        return CommandOutcome::Failed(format!("Goal snapshot failed: {error:#}"));
                    }
                };
                match goals.iter().find(|goal| goal.id == id) {
                    Some(goal) => CommandOutcome::Emit(Emitted::new(render_goal(goal))),
                    // Asking for a Goal that does not exist is a usage error: the
                    // id was the user's input, and they can retype it.
                    None => CommandOutcome::Usage(format!("Goal {id} not found")),
                }
            }
        }
    }
}

/// One Goal as the read-out popup, for `/goal show`.
///
/// A popup rather than a log line: the details are a read-out, and the log is the
/// conversation. The archived marker is deliberate — `show` is the one read that
/// reaches an archived Goal, so it has to say that it did.
fn render_goal(goal: &tact_protocol::GoalSnapshot) -> AgentUpdate {
    let criteria = if goal.criteria.is_empty() {
        "- (none)".to_string()
    } else {
        goal.criteria
            .iter()
            .map(|criterion| {
                format!(
                    "- {:?}: {}{}",
                    criterion.status,
                    criterion.description,
                    if criterion.required {
                        " (required)"
                    } else {
                        ""
                    }
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    AgentUpdate::PopupMarkdown {
        title: format!("Goal: {}", goal.title),
        source: format!(
            "**ID:** `{}`\n\n**Status:** `{:?}` / `{:?}`{}\n\n{}\n\n**Tasks:** {}/{}\n\n**Acceptance:** {}/{} passed\n\n{}",
            goal.id,
            goal.status,
            goal.phase,
            if goal.archived {
                "\n\n**Archived:** yes"
            } else {
                ""
            },
            goal.objective,
            goal.task_completed,
            goal.task_total,
            goal.acceptance.passed,
            goal.acceptance.required,
            criteria
        ),
    }
}

/// The `/goal` command behind an `Arc`, so a caller can keep a handle.
pub type SharedGoalCommand = Arc<GoalCommand>;

#[cfg(test)]
mod tests {
    use super::*;

    /// A command over two empty, throwaway stores.
    async fn command() -> GoalCommand {
        let dir = tempfile::tempdir().unwrap();
        let goal_path = dir.path().join("goals.db");
        let task_path = dir.path().join("tasks.db");
        let goal_manager =
            SharedGoalManager::new(crate::goal::GoalManager::new(&goal_path).await.unwrap());
        let task_manager =
            SharedTaskManager::new(crate::task::TaskManager::new(&task_path).await.unwrap());
        // The stores are kept alive by the managers, so the directory has to
        // outlive them; a fresh one per test means no shared state.
        std::mem::forget(dir);
        GoalCommand::new(goal_manager, task_manager, GoalLimits::default())
    }

    #[tokio::test]
    async fn a_bare_slash_goal_lists_live_goals() {
        // `/goal` alone is the common case: it opens the panel.
        let command = command().await;
        assert_eq!(
            command.parse("").unwrap(),
            GoalVerb::List {
                include_archived: false
            }
        );
        assert_eq!(
            command.parse("list").unwrap(),
            GoalVerb::List {
                include_archived: false
            }
        );
    }

    #[tokio::test]
    async fn archived_goals_are_opt_in_by_both_spellings() {
        let command = command().await;
        for args in ["list --all", "list -a"] {
            assert_eq!(
                command.parse(args).unwrap(),
                GoalVerb::List {
                    include_archived: true
                }
            );
        }
    }

    #[tokio::test]
    async fn an_unknown_list_flag_is_named() {
        let command = command().await;
        let error = command.parse("list --bogus").unwrap_err();
        assert!(error.contains("--bogus"), "{error}");
        assert!(
            error.contains("--all"),
            "the error names what is accepted: {error}"
        );
    }

    #[tokio::test]
    async fn show_needs_exactly_one_id() {
        let command = command().await;
        assert_eq!(
            command.parse("show abc").unwrap(),
            GoalVerb::Show { id: "abc".into() }
        );
        assert!(command.parse("show").unwrap_err().contains("needs an id"));
        assert!(command.parse("show a b").unwrap_err().contains("one id"));
    }

    #[tokio::test]
    async fn an_unknown_verb_is_named_with_the_alternatives() {
        let command = command().await;
        let error = command.parse("destroy everything").unwrap_err();
        assert!(error.contains("destroy"), "{error}");
        assert!(error.contains("list"), "{error}");
        assert!(error.contains("show"), "{error}");
    }

    #[tokio::test]
    async fn a_bad_verb_answers_with_usage_and_the_synopsis() {
        let outcome = command()
            .await
            .run_async(&CommandInvocation {
                name: "goal".into(),
                args: "destroy".into(),
            })
            .await;
        let CommandOutcome::Usage(text) = outcome else {
            panic!("expected usage, got {outcome:?}");
        };
        assert!(
            text.contains("/goal [list"),
            "the synopsis is shown: {text}"
        );
    }

    #[tokio::test]
    async fn listing_an_empty_workspace_emits_the_empty_projection() {
        // The panel is cleared by an empty `GoalsChanged`, so `/goal` with no
        // Goals must still produce the update rather than nothing.
        let outcome = command()
            .await
            .run_async(&CommandInvocation {
                name: "goal".into(),
                args: String::new(),
            })
            .await;
        let CommandOutcome::Emit(emitted) = outcome else {
            panic!("expected an update, got {outcome:?}");
        };
        let AgentUpdate::GoalsChanged { goals } = emitted.get() else {
            panic!("expected GoalsChanged, got {:?}", emitted.get());
        };
        assert!(goals.is_empty());
    }

    #[tokio::test]
    async fn showing_an_unknown_goal_names_the_id() {
        let outcome = command()
            .await
            .run_async(&CommandInvocation {
                name: "goal".into(),
                args: "show nope".into(),
            })
            .await;
        let CommandOutcome::Usage(text) = outcome else {
            panic!("expected usage, got {outcome:?}");
        };
        assert!(text.contains("nope"), "{text}");
    }
}
