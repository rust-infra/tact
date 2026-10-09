//! `tact-ui hooks …` — review the command hooks Tac would run.
//!
//! Split by side effect, like `mcp_cli`: `list` only reads and renders,
//! `trust` / `forget` write the user-level review store
//! (`~/.tact/hooks-state.json`). Nothing here connects to a server or runs a
//! hook — reviewing a hook must not execute it.

use anyhow::{Context as _, Result, bail};

use tact_extensions::config::HooksSubcommand;
use tact_extensions::plugin::{
    HookLoadReport, HookSummary, forget_hook_trust, survey_hooks, trust_hooks,
};

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

/// Renders `hooks list`, split so the wording is testable without stdout.
///
/// `pending` is named before `trusted`: the reader's question is "what is
/// waiting for me", and a long list of approved hooks is the boring half.
#[must_use]
pub fn render_hooks_listing(report: &HookLoadReport) -> String {
    let total = report.trusted.len() + report.pending.len();
    if total == 0 {
        return "No command hooks configured.\n\n\
                Declare them in `~/.tact/hooks.json` or `.tact/hooks.json` \
                (Codex's `hooks.json` schema), or install a plugin that ships them."
            .to_string();
    }

    let mut out = format!("{total} hook(s) configured ({total} = trusted + pending):");
    out.push_str(&format!(
        "\n  {} trusted, {} need review",
        report.trusted.len(),
        report.pending.len()
    ));

    if !report.pending.is_empty() {
        out.push_str("\n\nNeeds review (not run):\n");
        out.push_str(
            &report
                .pending
                .iter()
                .map(|hook| format!("  {}", hook.describe()))
                .collect::<Vec<_>>()
                .join("\n"),
        );
        out.push_str(
            "\n\nApprove with `tact-ui hooks trust --all`, or narrow it \
                      with `--source <label>`.",
        );
    }

    if !report.trusted.is_empty() {
        out.push_str("\n\nTrusted (runs):\n");
        out.push_str(
            &report
                .trusted
                .iter()
                .map(|hook| format!("  {}", hook.describe()))
                .collect::<Vec<_>>()
                .join("\n"),
        );
    }

    out
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
    fn an_empty_configuration_explains_where_hooks_live() {
        let text = render_hooks_listing(&HookLoadReport::default());
        assert!(text.contains("No command hooks configured"), "{text}");
        assert!(text.contains("~/.tact/hooks.json"), "{text}");
        assert!(text.contains(".tact/hooks.json"), "{text}");
    }

    #[test]
    fn pending_hooks_are_named_before_trusted_ones() {
        let report = HookLoadReport {
            trusted: vec![hook("plugin ponytail", "SessionStart", "run-trusted.sh")],
            pending: vec![hook("~/.tact/hooks.json", "PreToolUse", "run-pending.sh")],
        };

        let text = render_hooks_listing(&report);
        let pending_at = text.find("Needs review").expect("pending section");
        let trusted_at = text.find("Trusted (runs)").expect("trusted section");
        assert!(pending_at < trusted_at, "{text}");
        assert!(text.contains("2 hook(s) configured"), "{text}");
        assert!(text.contains("1 trusted, 1 need review"), "{text}");
        // The command itself is what a reviewer is approving, so it must show.
        assert!(text.contains("run-pending.sh"), "{text}");
        assert!(text.contains("hooks trust --all"), "{text}");
    }

    #[test]
    fn an_all_trusted_configuration_has_no_review_section() {
        let report = HookLoadReport {
            trusted: vec![hook("plugin ponytail", "SessionStart", "run.sh")],
            pending: Vec::new(),
        };

        let text = render_hooks_listing(&report);
        assert!(!text.contains("Needs review"), "{text}");
        assert!(text.contains("Trusted (runs)"), "{text}");
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
