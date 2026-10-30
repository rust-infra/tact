//! `tact-ui hooks …` — review the command hooks Tac would run.
//!
//! Split by side effect, like `mcp_cli`: `list` only reads and renders,
//! `trust` / `forget` write the user-level review store
//! (`~/.tact/hooks-state.json`). Nothing here connects to a server or runs a
//! hook — reviewing a hook must not execute it.

use anyhow::{Context as _, Result, bail};

use tact::config::HooksSubcommand;
use tact::plugin::{HookSummary, forget_hook_trust, survey_hooks, trust_hooks};
use tact_host::rendering::render_hooks_listing;

/// Runs one `tact-ui hooks` subcommand.
pub fn run_hooks_cli(command: HooksSubcommand) -> Result<()> {
    let work_dir = std::env::current_dir().context("failed to resolve the working directory")?;
    match command {
        HooksSubcommand::List => list(&work_dir),
        HooksSubcommand::Trust { all, source } => trust(&work_dir, all, source.as_deref()),
        HooksSubcommand::Forget { all } => forget(all),
    }
}

/// Prints every configured hook with its review status.
fn list(work_dir: &std::path::Path) -> Result<()> {
    let report = survey_hooks(work_dir)?;
    println!("{}", render_hooks_listing(&report));
    Ok(())
}

/// Approves pending hooks and says what will happen next.
fn trust(work_dir: &std::path::Path, all: bool, source: Option<&str>) -> Result<()> {
    let approved = trust_hooks(work_dir, all, source)?;
    if approved.is_empty() {
        println!("Nothing to approve: every configured hook has already been reviewed.");
        return Ok(());
    }
    println!("Approved {} hook(s):", approved.len());
    for hook in &approved {
        println!("  {}", hook.describe());
    }
    println!(
        "\nThey run from the next session onward. Review the store with \
         `tact-ui hooks list`, revoke with `tact-ui hooks forget --all`."
    );
    Ok(())
}

/// Clears every review decision.
fn forget(all: bool) -> Result<()> {
    if !all {
        bail!("pass --all: revoking every hook approval is never implicit");
    }
    forget_hook_trust()?;
    println!(
        "Forgot every hook approval. No hook runs until it is reviewed again \
         with `tact-ui hooks trust --all`."
    );
    Ok(())
}

/// One line summarising a pending set, for the load notice.
///
/// The notice and the listing speak the same words, so a user who reads one
/// recognises the other.
#[must_use]
pub fn pending_summary(pending: &[HookSummary]) -> String {
    match pending.len() {
        0 => String::new(),
        1 => "1 hook needs review and was not run".to_string(),
        n => format!("{n} hooks need review and were not run"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hook(source: &str, event: &str, command: &str) -> HookSummary {
        HookSummary {
            hash: format!("{source}:{event}:{command}"),
            source: source.to_string(),
            event: event.to_string(),
            matcher: None,
            command: command.to_string(),
        }
    }

    #[test]
    fn the_pending_summary_counts_correctly() {
        assert_eq!(pending_summary(&[]), "");
        assert_eq!(
            pending_summary(&[hook("a", "Stop", "x")]),
            "1 hook needs review and was not run"
        );
        assert_eq!(
            pending_summary(&[hook("a", "Stop", "x"), hook("b", "Stop", "y")]),
            "2 hooks need review and were not run"
        );
    }
}
