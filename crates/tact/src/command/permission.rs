//! `/permission <mode>` as a catalog command.
//!
//! The picker in the view still owns *choosing* a mode; this command owns applying
//! one. That split is why the transport loses a variant: the view no longer has to
//! tell the driver "set the permission mode", it says "run `/permission plan`",
//! which is what every other command says.
//!
//! It needs only the shared permission manager, which is reachable without the
//! agent — the manager is an `Arc` the whole runtime holds.

use std::sync::Arc;

use async_trait::async_trait;
use tact_kernel::capability::command::{
    Command, CommandInvocation, CommandMeta, CommandOutcome, CommandScope,
};

/// Apply a permission mode.
pub struct PermissionCommand {
    meta: CommandMeta,
    manager: crate::permission::SharedPermissionManager,
}

impl PermissionCommand {
    /// A command over the shared manager.
    pub fn new(manager: crate::permission::SharedPermissionManager) -> Self {
        Self {
            meta: CommandMeta {
                name: "permission".to_string(),
                aliases: Vec::new(),
                summary: "Set the permission mode (default, plan or auto)".to_string(),
                usage: "/permission <default|plan|auto>".to_string(),
                scope: CommandScope::Runtime,
                // The mode is the argument, so Enter completes instead of
                // applying whatever happens to be highlighted.
                needs_args: true,
            },
            manager,
        }
    }

    /// Parse a mode name.
    ///
    /// `Err` is a usage line, not a failure: the user typed a mode that does not
    /// exist, and the message names the ones that do.
    fn parse(mode: &str) -> Result<crate::permission::PermissionMode, String> {
        match mode.trim() {
            "plan" => Ok(crate::permission::PermissionMode::Plan),
            "default" => Ok(crate::permission::PermissionMode::Default),
            "auto" => Ok(crate::permission::PermissionMode::Auto),
            "" => Err("`/permission` needs a mode".to_string()),
            other => Err(format!(
                "unknown permission mode `{other}`; expected default, plan or auto"
            )),
        }
    }
}

#[async_trait]
impl Command for PermissionCommand {
    fn meta(&self) -> &CommandMeta {
        &self.meta
    }

    async fn run_async(&self, invocation: &CommandInvocation) -> CommandOutcome {
        let mode = match Self::parse(&invocation.args) {
            Ok(mode) => mode,
            Err(message) => {
                return CommandOutcome::Usage(format!("{message}\n{}", self.meta.usage));
            }
        };
        self.manager.set_mode(mode);
        // The view already shows the localized confirmation for a mode change, so
        // this stays silent rather than adding a second line saying the same
        // thing.
        CommandOutcome::Done
    }
}

/// The manager a [`PermissionCommand`] writes through.
impl PermissionCommand {
    /// The shared manager this command holds, for a caller that also wants it.
    pub fn manager(&self) -> &crate::permission::SharedPermissionManager {
        &self.manager
    }
}

/// Build the wiring a host passes for `/permission`.
pub fn wiring(manager: crate::permission::SharedPermissionManager) -> Arc<dyn Command> {
    Arc::new(PermissionCommand::new(manager))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manager() -> crate::permission::SharedPermissionManager {
        crate::permission::SharedPermissionManager::new(
            crate::permission::PermissionManager::try_new(
                crate::permission::PermissionMode::Default,
            )
            .expect("permission manager"),
        )
    }

    fn command() -> PermissionCommand {
        PermissionCommand::new(manager())
    }

    /// Each mode name parses to its own mode.
    #[test]
    fn every_mode_name_parses() {
        assert_eq!(
            PermissionCommand::parse("plan").unwrap(),
            crate::permission::PermissionMode::Plan
        );
        assert_eq!(
            PermissionCommand::parse("default").unwrap(),
            crate::permission::PermissionMode::Default
        );
        assert_eq!(
            PermissionCommand::parse("auto").unwrap(),
            crate::permission::PermissionMode::Auto
        );
        // Surrounding whitespace is the shell's, not the user's intent.
        assert_eq!(
            PermissionCommand::parse("  plan ").unwrap(),
            crate::permission::PermissionMode::Plan
        );
    }

    /// An unknown mode is refused and the message names the real ones.
    #[test]
    fn an_unknown_mode_is_refused() {
        let error = PermissionCommand::parse("yolo").unwrap_err();
        assert!(error.contains("yolo"), "{error}");
        assert!(error.contains("plan"), "the valid modes are named: {error}");
    }

    /// A missing mode is a usage error rather than a silent no-op.
    #[test]
    fn a_missing_mode_is_refused() {
        assert!(PermissionCommand::parse("").is_err());
    }

    /// Running the command applies the mode to the shared manager.
    #[tokio::test]
    async fn running_applies_the_mode() {
        let command = command();
        let outcome = command
            .run_async(&CommandInvocation {
                name: "permission".to_string(),
                args: "plan".to_string(),
            })
            .await;
        assert!(matches!(outcome, CommandOutcome::Done), "{outcome:?}");
        assert_eq!(
            command.manager().mode(),
            crate::permission::PermissionMode::Plan,
            "the shared manager is what the runtime reads"
        );
    }

    /// A usage error leaves the mode alone.
    #[tokio::test]
    async fn a_usage_error_does_not_change_the_mode() {
        let command = command();
        let before = command.manager().mode();
        let outcome = command
            .run_async(&CommandInvocation {
                name: "permission".to_string(),
                args: "yolo".to_string(),
            })
            .await;
        assert!(matches!(outcome, CommandOutcome::Usage(_)), "{outcome:?}");
        assert_eq!(command.manager().mode(), before);
    }

    /// The metadata says the mode is an argument.
    #[test]
    fn the_metadata_says_it_takes_an_argument() {
        let meta = command().meta().clone();
        assert_eq!(meta.name, "permission");
        assert!(meta.needs_args);
        assert_eq!(meta.scope, CommandScope::Runtime);
    }
}
