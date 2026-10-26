//! `/model`, `/thinking` and `/effort` — the session's settings as commands.
//!
//! Each already had a picker. The picker still exists and still owns *choosing*;
//! these own *applying*, and they accept the same value as text. That is why the
//! picker can stop carrying a typed payload: it now sends a name and an argument,
//! which is what every other command sends.
//!
//! Validation happens here rather than in the driver. `/effort` used to hand an
//! unparseable value to a setter that printed a line to stderr and cleared the
//! setting; a usage error the user can read is the better answer.

use std::sync::Arc;

use async_trait::async_trait;
use tact_kernel::capability::command::{
    Command, CommandInvocation, CommandMeta, CommandOutcome, CommandScope,
};

use crate::agent::settings_service::{SettingsChange, SettingsService};

/// Which setting a command changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsVerb {
    /// `/model <id>`
    Model,
    /// `/thinking <tokens>`
    Thinking,
    /// `/effort <none|minimal|low|medium|high|xhigh|max>`
    Effort,
}

impl SettingsVerb {
    fn name(self) -> &'static str {
        match self {
            Self::Model => "model",
            Self::Thinking => "thinking",
            Self::Effort => "effort",
        }
    }

    fn summary(self) -> &'static str {
        match self {
            Self::Model => "Set the model this session calls",
            Self::Thinking => "Set the thinking budget, in tokens",
            Self::Effort => "Set the reasoning effort",
        }
    }

    fn usage(self) -> &'static str {
        match self {
            Self::Model => "/model <id>",
            Self::Thinking => "/thinking <tokens>",
            Self::Effort => "/effort <none|minimal|low|medium|high|xhigh|max>",
        }
    }
}

/// One session setting, as a command.
pub struct SettingsCommand {
    verb: SettingsVerb,
    service: Arc<dyn SettingsService>,
    meta: CommandMeta,
}

impl SettingsCommand {
    /// A command for one setting.
    pub fn new(verb: SettingsVerb, service: Arc<dyn SettingsService>) -> Self {
        Self {
            verb,
            service,
            meta: CommandMeta {
                name: verb.name().to_string(),
                aliases: Vec::new(),
                summary: verb.summary().to_string(),
                usage: verb.usage().to_string(),
                scope: CommandScope::Runtime,
                // The value is the argument, so Enter completes instead of
                // applying whatever happens to be highlighted.
                needs_args: true,
            },
        }
    }

    /// Turn the arguments into the change to apply.
    ///
    /// `Err` is a usage line: the value was the user's input and they can retype it.
    fn parse(&self, args: &str) -> Result<SettingsChange, String> {
        let value = args.trim();
        if value.is_empty() {
            return Err(format!("`/{}` needs a value", self.verb.name()));
        }
        match self.verb {
            SettingsVerb::Model => Ok(SettingsChange::Model(value.to_string())),
            SettingsVerb::Thinking => value
                .parse::<usize>()
                .map(SettingsChange::ThinkingBudget)
                .map_err(|_| format!("`/thinking` needs a token count, not `{value}`")),
            SettingsVerb::Effort => match value {
                // `none` is the "no effort" tier; the setter takes `Option`, so it
                // maps to `None` rather than to `ReasoningEffort::None`.
                "none" | "off" => Ok(SettingsChange::ReasoningEffort(None)),
                other => other
                    .parse::<tact_protocol::ReasoningEffort>()
                    .map(|effort| SettingsChange::ReasoningEffort(Some(effort)))
                    .map_err(|error| error.to_string()),
            },
        }
    }
}

#[async_trait]
impl Command for SettingsCommand {
    fn meta(&self) -> &CommandMeta {
        &self.meta
    }

    async fn run_async(&self, invocation: &CommandInvocation) -> CommandOutcome {
        let change = match self.parse(&invocation.args) {
            Ok(change) => change,
            Err(message) => {
                return CommandOutcome::Usage(format!("{message}\n{}", self.meta.usage));
            }
        };
        match self.service.apply(change) {
            // Silent: the agent answers with a `ModelInfo` once it applies the
            // change, and that is what resyncs the status bar. A second line here
            // would say the same thing twice.
            Ok(()) => CommandOutcome::Done,
            Err(error) => CommandOutcome::Failed(format!(
                "/{} could not be applied: {error:#}",
                self.verb.name()
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Recording {
        applied: std::sync::Mutex<Vec<SettingsChange>>,
        closed: bool,
    }

    impl SettingsService for Recording {
        fn apply(&self, change: SettingsChange) -> anyhow::Result<()> {
            if self.closed {
                anyhow::bail!("closed");
            }
            self.applied.lock().unwrap().push(change);
            Ok(())
        }
    }

    fn command(verb: SettingsVerb) -> (SettingsCommand, std::sync::Arc<Recording>) {
        let recording = std::sync::Arc::new(Recording::default());
        (SettingsCommand::new(verb, recording.clone()), recording)
    }

    async fn run(command: &SettingsCommand, args: &str) -> CommandOutcome {
        command
            .run_async(&CommandInvocation {
                name: command.meta.name.clone(),
                args: args.to_string(),
            })
            .await
    }

    /// A model id is taken verbatim.
    #[tokio::test]
    async fn the_model_is_applied_verbatim() {
        let (command, recording) = command(SettingsVerb::Model);
        assert!(matches!(
            run(&command, " gpt-5 ").await,
            CommandOutcome::Done
        ));
        assert_eq!(
            recording.applied.lock().unwrap().as_slice(),
            &[SettingsChange::Model("gpt-5".to_string())]
        );
    }

    /// A token count must be a number.
    #[tokio::test]
    async fn the_thinking_budget_must_be_a_number() {
        let (command, recording) = command(SettingsVerb::Thinking);
        assert!(matches!(run(&command, "32000").await, CommandOutcome::Done));
        assert_eq!(
            recording.applied.lock().unwrap().as_slice(),
            &[SettingsChange::ThinkingBudget(32_000)]
        );

        let outcome = run(&command, "lots").await;
        assert!(matches!(&outcome, CommandOutcome::Usage(text) if text.contains("token count")));
        assert_eq!(
            recording.applied.lock().unwrap().len(),
            1,
            "a refused value must not be applied"
        );
    }

    /// Every effort tier parses, and `none` clears it.
    #[tokio::test]
    async fn every_effort_tier_parses_and_none_clears() {
        let (command, recording) = command(SettingsVerb::Effort);
        for tier in tact_protocol::ReasoningEffort::ALL {
            assert!(
                matches!(run(&command, tier.as_str()).await, CommandOutcome::Done),
                "{tier:?}"
            );
        }
        assert!(matches!(run(&command, "none").await, CommandOutcome::Done));
        let applied = recording.applied.lock().unwrap();
        assert_eq!(applied.len(), tact_protocol::ReasoningEffort::ALL.len() + 1);
        assert_eq!(
            applied.last(),
            Some(&SettingsChange::ReasoningEffort(None)),
            "`none` maps to the absent tier, not to `ReasoningEffort::None`"
        );
    }

    /// An unknown effort is refused and the message names the real ones.
    #[tokio::test]
    async fn an_unknown_effort_names_the_valid_tiers() {
        let (command, recording) = command(SettingsVerb::Effort);
        let outcome = run(&command, "turbo").await;
        match &outcome {
            CommandOutcome::Usage(text) => {
                assert!(text.contains("turbo"), "{text}");
                assert!(text.contains("high"), "the valid tiers are named: {text}");
            }
            other => panic!("expected usage, got {other:?}"),
        }
        assert!(recording.applied.lock().unwrap().is_empty());
    }

    /// A missing value is a usage error for every verb.
    #[tokio::test]
    async fn a_missing_value_is_a_usage_error() {
        for verb in [
            SettingsVerb::Model,
            SettingsVerb::Thinking,
            SettingsVerb::Effort,
        ] {
            let (command, recording) = command(verb);
            assert!(matches!(
                run(&command, "  ").await,
                CommandOutcome::Usage(_)
            ));
            assert!(recording.applied.lock().unwrap().is_empty());
        }
    }

    /// A service that cannot apply is reported, not swallowed.
    #[tokio::test]
    async fn a_failed_apply_is_reported() {
        let recording = std::sync::Arc::new(Recording {
            closed: true,
            ..Recording::default()
        });
        let command = SettingsCommand::new(SettingsVerb::Model, recording);
        match run(&command, "gpt-5").await {
            CommandOutcome::Failed(text) => assert!(text.contains("could not be applied")),
            other => panic!("expected a failure, got {other:?}"),
        }
    }

    /// The metadata names the verb and its argument.
    #[test]
    fn the_metadata_matches_the_verb() {
        for (verb, name) in [
            (SettingsVerb::Model, "model"),
            (SettingsVerb::Thinking, "thinking"),
            (SettingsVerb::Effort, "effort"),
        ] {
            let (command, _) = command(verb);
            let meta = command.meta();
            assert_eq!(meta.name, name);
            assert!(meta.needs_args);
            assert_eq!(meta.scope, CommandScope::Runtime);
        }
    }
}
