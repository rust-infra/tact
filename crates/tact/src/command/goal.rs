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
    /// `/goal pause <id>` — ask the owning run to stop at a safe point.
    Pause { id: String },
    /// `/goal cancel <id>` — an explicit terminal decision.
    Cancel { id: String },
    /// `/goal archive <id>` or `/goal unarchive <id>`.
    Archive { id: String, archived: bool },
    /// `/goal create <objective> [-- <criterion>; ...]` — mint a Goal and start its run.
    Create {
        title: String,
        objective: String,
        criteria: Vec<String>,
    },
    /// `/goal accept <id> <text>` — record a human acceptance.
    Accept { id: String, text: String },
    /// `/goal steer [<id>] <text>` — redirect an active Goal.
    ///
    /// No id field: a leading id in the text is resolved by [`steer`], which can
    /// read the store — the parser cannot, so it must not try.
    Steer { text: String },
}

/// The `/goal` command.
pub struct GoalCommand {
    goal_manager: SharedGoalManager,
    task_manager: SharedTaskManager,
    limits: GoalLimits,
    /// Where a pause or cancel reports itself.
    events: tact_protocol::events::UpdateSink,
    /// Creating and accepting a Goal need the domain service.
    service: std::sync::Arc<dyn crate::goal::service::GoalService>,
    meta: CommandMeta,
}

impl GoalCommand {
    pub fn new(
        goal_manager: SharedGoalManager,
        task_manager: SharedTaskManager,
        limits: GoalLimits,
        events: tact_protocol::events::UpdateSink,
        service: std::sync::Arc<dyn crate::goal::service::GoalService>,
    ) -> Self {
        Self {
            goal_manager,
            task_manager,
            limits,
            events,
            service,
            meta: CommandMeta {
                name: "goal".to_string(),
                aliases: Vec::new(),
                summary: "Create, accept, list, show, pause, cancel, archive or steer a Goal"
                    .to_string(),
                usage:
                    "/goal [list [--all] | show <id> | create <objective> [-- <criterion>; ...] | \
                     accept <id> <text> | pause <id> | cancel <id> | archive <id> | \
                     unarchive <id> | steer [<id>] <text>]"
                        .to_string(),
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
        // The remainder with its original spacing, for verbs whose grammar is
        // free text: `create`'s `-- ` separator only means anything if the space
        // before it survived, and re-joining tokens would collapse it.
        let trimmed = args.trim_start();
        let raw_rest = trimmed
            .find(char::is_whitespace)
            .map_or("", |index| trimmed[index..].trim_start());
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
            Some(verb @ ("pause" | "cancel")) => match (words.next(), words.next()) {
                (Some(id), None) => Ok(if verb == "pause" {
                    GoalVerb::Pause { id: id.to_string() }
                } else {
                    GoalVerb::Cancel { id: id.to_string() }
                }),
                (None, _) => Err(format!("`/goal {verb}` needs an id")),
                (Some(_), Some(extra)) => {
                    Err(format!("`/goal {verb}` takes one id (got extra `{extra}`)"))
                }
            },
            Some(verb @ ("archive" | "unarchive")) => match (words.next(), words.next()) {
                (Some(id), None) => Ok(GoalVerb::Archive {
                    id: id.to_string(),
                    archived: verb == "archive",
                }),
                (None, _) => Err(format!("`/goal {verb}` needs an id")),
                (Some(_), Some(extra)) => {
                    Err(format!("`/goal {verb}` takes one id (got extra `{extra}`)"))
                }
            },
            Some("create") => {
                // `<objective> [-- <criterion>; <criterion> ...]`, the grammar the
                // view already had. Criteria are the user's own acceptance gate:
                // without them the Goal finishes on the model's completion proposal
                // (self-attested); the `--` separator keeps ordinary prose —
                // including hyphenated words — in the objective.
                let (objective, criteria) = match raw_rest.split_once(" -- ") {
                    Some((objective, criteria)) => (
                        objective.trim(),
                        criteria
                            .split(';')
                            .map(str::trim)
                            .filter(|criterion| !criterion.is_empty())
                            .map(str::to_string)
                            .collect::<Vec<_>>(),
                    ),
                    None => (raw_rest.trim(), Vec::new()),
                };
                if objective.is_empty() {
                    return Err("`/goal create` needs an objective".to_string());
                }
                Ok(GoalVerb::Create {
                    // The title is the objective's first 80 characters, which is
                    // what the panel row can show; the objective keeps the whole
                    // text.
                    title: objective.chars().take(80).collect(),
                    objective: objective.to_string(),
                    criteria,
                })
            }
            Some("accept") => {
                // The id is one token; everything after it is the acceptance note,
                // which is prose and may contain anything.
                match words.next() {
                    Some(id) => {
                        let text = words.collect::<Vec<_>>().join(" ");
                        if text.is_empty() {
                            Err("`/goal accept` needs an acceptance note".to_string())
                        } else {
                            Ok(GoalVerb::Accept {
                                id: id.to_string(),
                                text,
                            })
                        }
                    }
                    None => Err("`/goal accept` needs an id and a note".to_string()),
                }
            }
            Some("steer") => {
                // Whole-line text, with no id split here: `steer` accepts a leading
                // id/prefix in the text and the store is authoritative about whether
                // it matches. Splitting in the parser would mean two places deciding
                // what "the id" is, and the one that cannot read the store would win.
                let text = words.collect::<Vec<_>>().join(" ");
                if text.is_empty() {
                    return Err("`/goal steer` needs an instruction".to_string());
                }
                Ok(GoalVerb::Steer { text })
            }
            Some(other) => Err(format!(
                "unknown verb `{other}`; expected `list`, `show`, `pause`, `cancel`, \
                 `archive` or `steer`"
            )),
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
            GoalVerb::Pause { id } => {
                // The lifecycle helper answers by emitting, and it may emit more
                // than one line (a request, then the panel refresh), so it reports
                // through the sink the host supplied rather than through the single
                // outcome a command may return.
                crate::goal::lifecycle::request_pause(
                    &self.goal_manager,
                    &self.task_manager,
                    self.limits,
                    &self.events,
                    // Not an interrupt of *this* process: the request is durable
                    // first, so it reaches the owner wherever it runs. A command
                    // cannot know whether this process owns the Goal, and flipping
                    // the flag blind would cancel whatever turn *is* running here.
                    None,
                    &id,
                )
                .await;
                CommandOutcome::Done
            }
            GoalVerb::Cancel { id } => {
                crate::goal::lifecycle::request_cancel(
                    &self.goal_manager,
                    &self.task_manager,
                    self.limits,
                    &self.events,
                    None,
                    &id,
                )
                .await;
                CommandOutcome::Done
            }
            GoalVerb::Archive { id, archived } => {
                crate::goal::lifecycle::set_archived(
                    &self.goal_manager,
                    &self.task_manager,
                    self.limits,
                    &self.events,
                    &id,
                    archived,
                )
                .await;
                CommandOutcome::Done
            }
            GoalVerb::Create {
                title,
                objective,
                criteria,
            } => {
                if self
                    .service
                    .create(title, objective, criteria)
                    .await
                    .is_some()
                {
                    CommandOutcome::Done
                } else {
                    // The service already said why; a second line would repeat it.
                    CommandOutcome::Failed("Goal creation refused".to_string())
                }
            }
            GoalVerb::Accept { id, text } => {
                self.service.accept(&id, &text).await;
                CommandOutcome::Done
            }
            GoalVerb::Steer { text } => {
                crate::goal::lifecycle::steer(
                    &self.goal_manager,
                    &self.task_manager,
                    &self.events,
                    self.limits,
                    None,
                    &text,
                )
                .await;
                CommandOutcome::Done
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

    /// A service that does nothing, for tests that only exercise the read verbs.
    struct NoopService;

    #[async_trait::async_trait]
    impl crate::goal::service::GoalService for NoopService {
        async fn create(
            &self,
            _title: String,
            _objective: String,
            _criteria: Vec<String>,
        ) -> Option<String> {
            None
        }

        async fn accept(&self, _goal_id: &str, _text: &str) {}
    }

    /// `/goal create` keeps the objective whole and splits criteria on `;` after
    /// `--`. These assertions used to live in the view, which was where the grammar
    /// was; the grammar moved here with the verb, so the tests did too.
    #[tokio::test]
    async fn create_splits_criteria_after_the_separator() {
        let command = command().await;
        let verb = command
            .parse(
                "create Ship the release -- the full test suite passes; the changelog is written",
            )
            .unwrap();
        match verb {
            GoalVerb::Create {
                title,
                objective,
                criteria,
            } => {
                assert_eq!(title, "Ship the release");
                assert_eq!(objective, "Ship the release");
                assert_eq!(
                    criteria,
                    vec![
                        "the full test suite passes".to_string(),
                        "the changelog is written".to_string(),
                    ]
                );
            }
            other => panic!("expected create, got {other:?}"),
        }
    }

    /// Without `--`, a dash in the objective is just a dash.
    #[tokio::test]
    async fn create_without_a_separator_keeps_dashes() {
        let command = command().await;
        match command
            .parse("create Upgrade to rust-2024 edition")
            .unwrap()
        {
            GoalVerb::Create {
                objective,
                criteria,
                ..
            } => {
                assert_eq!(objective, "Upgrade to rust-2024 edition");
                assert!(criteria.is_empty());
            }
            other => panic!("expected create, got {other:?}"),
        }
    }

    /// A create with no objective is a usage error, not an empty Goal.
    #[tokio::test]
    async fn create_needs_an_objective() {
        let command = command().await;
        assert!(command.parse("create").is_err());
        assert!(command.parse("create    ").is_err());
        // A leading `--` with no space before it is prose, not a separator: the
        // grammar needs `" -- "`, so this is an objective that happens to start
        // with dashes. The view behaves the same way, which is why the assertion
        // is here rather than a refusal.
        assert!(command.parse("create -- only criteria").is_ok());
    }

    /// Accept needs an id and a note; the note may contain spaces.
    #[tokio::test]
    async fn accept_takes_an_id_and_a_free_text_note() {
        let command = command().await;
        match command
            .parse("accept goal-1 accepted after a long manual review")
            .unwrap()
        {
            GoalVerb::Accept { id, text } => {
                assert_eq!(id, "goal-1");
                assert_eq!(text, "accepted after a long manual review");
            }
            other => panic!("expected accept, got {other:?}"),
        }
        assert!(
            command.parse("accept goal-1").is_err(),
            "the note is required"
        );
        assert!(command.parse("accept").is_err());
    }

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
        GoalCommand::new(
            goal_manager,
            task_manager,
            GoalLimits::default(),
            tact_protocol::events::UpdateSink::default(),
            std::sync::Arc::new(NoopService),
        )
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
