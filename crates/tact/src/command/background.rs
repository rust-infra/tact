//! `/background` — what the background task manager is doing.
//!
//! The first *async* catalog command: `check` asks a manager that owns running
//! processes, so it awaits. That is why `Command` grew a `run_async` with a
//! defaulted body — a pure command still implements `run` alone, and the host
//! calls one method for both.

use std::sync::Arc;

use tact_kernel::capability::command::{Command, CommandMeta, CommandOutcome, Emitted};
use tact_protocol::AgentUpdate;

use crate::background::SharedBackgroundManager;

/// The `/background` command.
pub struct BackgroundCommand {
    manager: Arc<SharedBackgroundManager>,
    /// The session whose tasks are listed. Resolved per call, because the driver
    /// learns it only after the session row exists.
    session_id: Arc<dyn Fn() -> Option<String> + Send + Sync>,
    meta: CommandMeta,
}

impl BackgroundCommand {
    pub fn new(
        manager: Arc<SharedBackgroundManager>,
        session_id: Arc<dyn Fn() -> Option<String> + Send + Sync>,
    ) -> Self {
        Self {
            manager,
            session_id,
            meta: CommandMeta {
                name: "background".to_string(),
                aliases: Vec::new(),
                summary: "List background tasks, or detail one".to_string(),
                usage: "/background [id]".to_string(),
                scope: tact_kernel::capability::command::CommandScope::Runtime,
                // `[id]` is optional, so Enter runs rather than completing.
                needs_args: false,
            },
        }
    }
}

#[async_trait::async_trait]
impl Command for BackgroundCommand {
    fn meta(&self) -> &CommandMeta {
        &self.meta
    }

    async fn run_async(
        &self,
        invocation: &tact_kernel::capability::command::CommandInvocation,
    ) -> CommandOutcome {
        let task_id = invocation.args.trim();
        let task_id = (!task_id.is_empty()).then_some(task_id);
        let session = (self.session_id)();
        match self.manager.check(task_id, session.as_deref()).await {
            Ok(output) => {
                // Fenced so the one-line-per-task listing (and the single-task
                // pretty JSON) stays aligned and copyable in the popup; the
                // popup and its title are part of this command's output, not
                // something a host should re-decide.
                CommandOutcome::Emit(Emitted::new(AgentUpdate::PopupMarkdown {
                    title: "⚙️ Background Tasks".to_string(),
                    source: format!("```text\n{output}\n```"),
                }))
            }
            // A failure here is the manager's, not the user's. `Usage` would
            // blame the arguments and `Message` would open a popup, so the
            // failure keeps its own shape: the host renders it as an error.
            Err(error) => CommandOutcome::Failed(format!("Background check failed: {error}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tact_kernel::capability::command::CommandInvocation;

    use crate::background::BackgroundManager;

    /// A manager over an empty database in a throwaway directory.
    async fn command() -> BackgroundCommand {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("background.db");
        // The manager opens lazily here: `check` on an empty store is the case
        // the command has to handle first (a session that never ran anything).
        let manager = Arc::new(SharedBackgroundManager::new(
            BackgroundManager::new(&path).await.unwrap(),
        ));
        std::mem::forget(dir);
        BackgroundCommand::new(manager, Arc::new(|| Some("sess".to_string())))
    }

    #[tokio::test]
    async fn it_names_itself_and_its_optional_argument() {
        let command = command().await;
        let meta = command.meta();
        assert_eq!(meta.name, "background");
        assert_eq!(meta.usage, "/background [id]");
        assert!(!meta.needs_args, "the id is optional");
    }

    #[tokio::test]
    async fn a_listing_is_fenced_for_the_popup() {
        let outcome = command()
            .await
            .run_async(&CommandInvocation {
                name: "background".into(),
                args: String::new(),
            })
            .await;
        let CommandOutcome::Emit(emitted) = outcome else {
            panic!("expected a popup, got {outcome:?}");
        };
        let AgentUpdate::PopupMarkdown { title, source } = emitted.into_inner() else {
            panic!("expected PopupMarkdown");
        };
        assert_eq!(title, "⚙️ Background Tasks");
        assert!(source.starts_with("```text"), "{source}");
        assert!(source.trim_end().ends_with("```"), "{source}");
    }

    #[tokio::test]
    async fn a_failure_names_the_manager_not_the_arguments() {
        // An unknown id must not read as "you typed it wrong": the manager
        // answered, so its wording is what the user sees.
        let outcome = command()
            .await
            .run_async(&CommandInvocation {
                name: "background".into(),
                args: "no-such-task".into(),
            })
            .await;
        let CommandOutcome::Failed(text) = outcome else {
            panic!("a manager failure is its own outcome, got {outcome:?}");
        };
        assert!(text.starts_with("Background check failed:"), "{text}");
        assert!(
            text.contains("Unknown background task"),
            "the manager's wording survives: {text}"
        );
    }
}
