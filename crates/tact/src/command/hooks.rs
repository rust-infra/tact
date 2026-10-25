//! `/hooks` as a catalog command.
//!
//! Three verbs — `list`, `trust`, `forget` — that read and write hook approvals
//! on disk. None of them touch the agent: they answer from the plugin home and
//! the trust file, which is why they can be a command like any other instead of a
//! transport variant the driver has to interpret.

use std::path::PathBuf;

use async_trait::async_trait;
use tact_kernel::capability::command::{
    Command, CommandInvocation, CommandMeta, CommandOutcome, CommandScope,
};

/// What a `/hooks` invocation asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
enum HooksVerb {
    List,
    Trust { all: bool, source: Option<String> },
    Forget,
}

/// How a hook listing is rendered.
///
/// A function rather than a direct call because the renderer lives with the other
/// CLI surfaces, and a command that reached for it would make the extension crate
/// depend on a frontend. This is the same shape as `BackgroundWiring`: the host
/// supplies the part it owns, the command owns the grammar.
pub type HooksRenderer =
    std::sync::Arc<dyn Fn(&crate::plugin::HookLoadReport) -> String + Send + Sync>;

/// Inspect and approve command hooks.
pub struct HooksCommand {
    meta: CommandMeta,
    work_dir: PathBuf,
    render: HooksRenderer,
}

impl HooksCommand {
    /// A command scoped to the given working directory.
    ///
    /// The directory is where the project's hooks live; a command that read it
    /// from the process would answer for whichever project happened to be
    /// current.
    pub fn new(work_dir: impl Into<PathBuf>, render: HooksRenderer) -> Self {
        Self {
            render,
            meta: CommandMeta {
                name: "hooks".to_string(),
                aliases: Vec::new(),
                summary: "List, approve or forget command hooks".to_string(),
                usage: "/hooks [list | trust [--all] [<source>] | forget --all]".to_string(),
                scope: CommandScope::Runtime,
                // Verbs follow, so Enter completes instead of running.
                needs_args: true,
            },
            work_dir: work_dir.into(),
        }
    }

    /// Parse the verb and its arguments.
    ///
    /// `Err` is a usage line, not a failure: the user typed something the command
    /// does not accept, and the message says what it does accept.
    fn parse(args: &str) -> Result<HooksVerb, String> {
        let mut tokens = args.split_whitespace();
        match tokens.next().unwrap_or("") {
            "" | "list" => {
                if tokens.next().is_some() {
                    return Err("`/hooks list` takes no arguments".to_string());
                }
                Ok(HooksVerb::List)
            }
            "trust" => {
                let mut all = false;
                let mut source = None;
                for token in tokens {
                    if token == "--all" {
                        if all {
                            return Err("`--all` given twice".to_string());
                        }
                        all = true;
                    } else if token.starts_with("--") {
                        return Err(format!("unknown flag `{token}`"));
                    } else if source.is_some() {
                        return Err("`/hooks trust` takes at most one source".to_string());
                    } else {
                        source = Some(token.to_string());
                    }
                }
                Ok(HooksVerb::Trust { all, source })
            }
            "forget" => {
                // `--all` is accepted and required: forgetting one approval by
                // name is not supported, and silently ignoring the flag would let
                // a user think they revoked something they did not.
                let rest: Vec<&str> = tokens.collect();
                match rest.as_slice() {
                    ["--all"] => Ok(HooksVerb::Forget),
                    [] => Err("`/hooks forget` needs `--all`".to_string()),
                    _ => Err("`/hooks forget` only accepts `--all`".to_string()),
                }
            }
            other => Err(format!("unknown `/hooks` verb `{other}`")),
        }
    }
}

#[async_trait]
impl Command for HooksCommand {
    fn meta(&self) -> &CommandMeta {
        &self.meta
    }

    async fn run_async(&self, invocation: &CommandInvocation) -> CommandOutcome {
        let verb = match Self::parse(&invocation.args) {
            Ok(verb) => verb,
            Err(message) => {
                return CommandOutcome::Usage(format!("{message}\n{}", self.meta.usage));
            }
        };
        match verb {
            HooksVerb::List => match crate::plugin::survey_hooks(&self.work_dir) {
                // The same wording `tact-ui hooks list` uses: two surfaces naming
                // the same hooks must not describe them differently.
                Ok(report) => CommandOutcome::Emit(crate::command::Emitted::new(
                    tact_protocol::AgentUpdate::MdInfo((self.render)(&report)),
                )),
                Err(error) => CommandOutcome::Failed(format!("Hooks list failed: {error:#}")),
            },
            HooksVerb::Trust { all, source } => {
                match crate::plugin::trust_hooks(&self.work_dir, all, source.as_deref()) {
                    Ok(approved) if approved.is_empty() => CommandOutcome::Message(
                        "Nothing to approve: every configured hook has already been reviewed."
                            .to_string(),
                    ),
                    Ok(approved) => {
                        let mut lines = vec![format!("Approved {} hook(s):", approved.len())];
                        lines.extend(approved.iter().map(|hook| format!("  {}", hook.describe())));
                        // Hooks are registered when the agent is built, so an
                        // approval that only takes effect next session must say so.
                        lines.push(
                            "They run from the next session onward. Revoke with /hooks forget --all."
                                .to_string(),
                        );
                        CommandOutcome::Message(lines.join("\n"))
                    }
                    Err(error) => CommandOutcome::Failed(format!("Hooks trust failed: {error:#}")),
                }
            }
            HooksVerb::Forget => match crate::plugin::forget_hook_trust() {
                Ok(()) => CommandOutcome::Message(
                    "Forgot every hook approval. No hook runs until it is reviewed again with \
                     /hooks trust --all."
                        .to_string(),
                ),
                Err(error) => CommandOutcome::Failed(format!("Hooks forget failed: {error:#}")),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command() -> HooksCommand {
        // The renderer is the host's; a test supplies one that names the report
        // so the assertion can tell "the command rendered" from "it did not".
        HooksCommand::new(
            std::env::temp_dir(),
            std::sync::Arc::new(|report: &crate::plugin::HookLoadReport| {
                format!(
                    "rendered {} hooks",
                    report.trusted.len() + report.pending.len()
                )
            }),
        )
    }

    /// No verb means `list`: the shortest thing a user can type should work.
    #[test]
    fn an_empty_argument_list_means_list() {
        assert_eq!(HooksCommand::parse("").unwrap(), HooksVerb::List);
        assert_eq!(HooksCommand::parse("list").unwrap(), HooksVerb::List);
    }

    /// `trust` collects its flags and its source.
    #[test]
    fn trust_parses_flags_and_a_source() {
        assert_eq!(
            HooksCommand::parse("trust --all").unwrap(),
            HooksVerb::Trust {
                all: true,
                source: None
            }
        );
        assert_eq!(
            HooksCommand::parse("trust project").unwrap(),
            HooksVerb::Trust {
                all: false,
                source: Some("project".to_string())
            }
        );
        assert_eq!(
            HooksCommand::parse("trust --all project").unwrap(),
            HooksVerb::Trust {
                all: true,
                source: Some("project".to_string())
            }
        );
    }

    /// `forget` requires `--all`, and says so.
    ///
    /// Forgetting one approval by name is not supported; accepting a bare
    /// `forget` would make a user believe they revoked something they did not.
    #[test]
    fn forget_requires_all() {
        assert_eq!(
            HooksCommand::parse("forget --all").unwrap(),
            HooksVerb::Forget
        );
        let error = HooksCommand::parse("forget").unwrap_err();
        assert!(error.contains("--all"), "{error}");
        assert!(HooksCommand::parse("forget one").is_err());
    }

    /// An unknown verb is refused by name rather than treated as `list`.
    #[test]
    fn an_unknown_verb_is_refused() {
        let error = HooksCommand::parse("frobnicate").unwrap_err();
        assert!(error.contains("frobnicate"), "{error}");
    }

    /// Unknown flags are refused, not ignored.
    #[test]
    fn an_unknown_flag_is_refused() {
        let error = HooksCommand::parse("trust --force").unwrap_err();
        assert!(error.contains("--force"), "{error}");
    }

    /// A usage error names the usage line.
    #[tokio::test]
    async fn a_usage_error_shows_the_usage_line() {
        let outcome = command()
            .run_async(&CommandInvocation {
                name: "hooks".to_string(),
                args: "frobnicate".to_string(),
            })
            .await;
        match outcome {
            CommandOutcome::Usage(text) => {
                assert!(text.contains("/hooks"), "{text}");
                assert!(text.contains("trust"), "{text}");
            }
            other => panic!("expected usage, got {other:?}"),
        }
    }

    /// The metadata says it takes arguments.
    #[test]
    fn the_metadata_says_it_takes_arguments() {
        let meta = command().meta().clone();
        assert_eq!(meta.name, "hooks");
        assert!(meta.needs_args, "verbs follow, so Enter completes");
        assert_eq!(meta.scope, CommandScope::Runtime);
    }
}
