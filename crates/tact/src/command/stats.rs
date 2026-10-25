//! `/stats` — the session's local counters.
//!
//! Answered from the shared `SessionStats` handle, so it responds instantly even
//! while a turn is running: the numbers live in an `Arc<RwLock<_>>` the agent
//! writes and this reads. That is why it is a catalog command at all — it needs
//! no agent, only the handle.

use std::sync::{Arc, RwLock};

use tact_kernel::capability::command::{
    Command, CommandInvocation, CommandMeta, CommandOutcome, Emitted,
};
use tact_protocol::AgentUpdate;

use super::runtime_meta;
use crate::stats::SessionStats;

/// The `/stats` command.
pub struct StatsCommand {
    stats: Arc<RwLock<SessionStats>>,
    meta: CommandMeta,
}

impl StatsCommand {
    pub fn new(stats: Arc<RwLock<SessionStats>>) -> Self {
        Self {
            stats,
            meta: runtime_meta("stats", &[], "Show session statistics"),
        }
    }
}

impl Command for StatsCommand {
    fn meta(&self) -> &CommandMeta {
        &self.meta
    }

    fn run(&self, invocation: &CommandInvocation) -> CommandOutcome {
        // No arguments today. Refusing them is better than ignoring them: a user
        // who typed `/stats --json` should be told the flag does not exist, not
        // shown the plain summary and left to wonder.
        if !invocation.args.trim().is_empty() {
            return CommandOutcome::Usage(format!(
                "`/stats` takes no arguments (got `{}`)",
                invocation.args.trim()
            ));
        }
        let summary = match self.stats.read() {
            Ok(stats) => stats.summary(),
            // A poisoned lock means a previous writer panicked; the counters are
            // still readable and a reader should not fail because of it.
            Err(poisoned) => poisoned.into_inner().summary(),
        };
        // A read-out popup, not a log line: a snapshot is not conversation, and
        // the title is part of what this command produces — a host that
        // reformatted it would be re-deciding the command's own output.
        CommandOutcome::Emit(Emitted::new(AgentUpdate::PopupMarkdown {
            title: "Session Statistics".to_string(),
            source: summary,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tact_kernel::capability::command::CommandScope;

    fn command(stats: SessionStats) -> StatsCommand {
        StatsCommand::new(Arc::new(RwLock::new(stats)))
    }

    #[test]
    fn it_names_itself_and_its_scope() {
        let command = command(SessionStats::default());
        let meta = command.meta();
        assert_eq!(meta.name, "stats");
        assert_eq!(meta.scope, CommandScope::Runtime);
        assert!(!meta.needs_args);
        assert!(meta.aliases.is_empty());
    }

    #[test]
    fn it_answers_with_the_summary() {
        let stats = SessionStats {
            prompt_count: 3,
            ..SessionStats::default()
        };
        let outcome = command(stats).run(&CommandInvocation {
            name: "stats".into(),
            args: String::new(),
        });
        // A titled read-out, not a bare message: the command owns the popup
        // shape, so a host cannot render the same command two ways.
        let CommandOutcome::Emit(emitted) = outcome else {
            panic!("expected a popup, got {outcome:?}");
        };
        let AgentUpdate::PopupMarkdown { title, source } = emitted.into_inner() else {
            panic!("expected PopupMarkdown");
        };
        assert_eq!(title, "Session Statistics");
        assert!(source.contains("Session Stats"), "{source}");
    }

    #[test]
    fn an_argument_is_refused_by_name() {
        let outcome = command(SessionStats::default()).run(&CommandInvocation {
            name: "stats".into(),
            args: "  --json ".into(),
        });
        let CommandOutcome::Usage(text) = outcome else {
            panic!("expected usage, got {outcome:?}");
        };
        assert!(text.contains("--json"), "{text}");
    }

    #[test]
    fn a_poisoned_lock_is_still_readable() {
        // A panic while recording stats must not take `/stats` down with it: the
        // counters are plain integers, readable through the poison.
        let stats = Arc::new(RwLock::new(SessionStats::default()));
        let clone = stats.clone();
        let _ = std::thread::spawn(move || {
            let _guard = clone.write().unwrap();
            panic!("poison it");
        })
        .join();
        let outcome = StatsCommand::new(stats).run(&CommandInvocation {
            name: "stats".into(),
            args: String::new(),
        });
        assert!(
            matches!(outcome, CommandOutcome::Emit(_)),
            "the summary is still produced through the poison: {outcome:?}"
        );
    }
}
