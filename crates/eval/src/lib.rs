//! Repeatable evaluation of the Tact agent.
//!
//! Two runners produce one [`RunArtifact`]. [`HostRunner`] runs a case in
//! process and sees the typed [`tact::AgentLoopStop`]. [`CliRunner`] spawns
//! `tact-ui headless` and sees only what that process writes. Graders read a
//! [`RunArtifact`] and a work directory and never learn which runner produced
//! them, so a suite scores the same way either way.
//!
//! The public surface is three verbs: load a suite, run it, grade it. The host
//! path has a fourth step before the first verb, because it reads process-global
//! settings: `tact::config::init_from_args` must run once, and `HostRunner` says
//! so rather than failing per case.
//!
//! `process`, `suite`, and the runner modules stay private; their types reach
//! callers through the re-exports below.

mod case;
#[cfg(feature = "agent-run")]
mod cli_runner;
mod compare;
mod grader;
#[cfg(feature = "agent-run")]
mod host_runner;
#[cfg(feature = "agent-run")]
mod options;
mod process;
mod report;
mod runner;
mod suite;

pub use case::{EvalCase, FileAssertion, GraderSpec, Isolation, MAX_TIMEOUT_SECS, TextAssertion};
#[cfg(feature = "agent-run")]
pub use cli_runner::{CliRunner, default_cli_binary};
pub use compare::{
    COMPARISON_SCHEMA_VERSION, CaseComparisonStatus, CaseCounts, CaseDelta, ComparisonReason,
    ComparisonReasonKind, ComparisonReport, ComparisonStatus, ReportHeader, compare_reports,
    comparison_exit_code,
};
pub use grader::{Grade, Verdict, grade};
#[cfg(feature = "agent-run")]
pub use host_runner::HostRunner;
#[cfg(feature = "agent-run")]
pub use options::RunOptions;
pub use report::{
    CaseCost, CaseOutcome, CaseSummary, EvalReport, REPORT_SCHEMA_VERSION, ReportMeta,
    suite_fingerprint,
};
pub use runner::{
    RunArtifact, RunFailure, RunIdentity, RunOutcome, Runner, RunnerKind, StopView, TokenTotals,
    ToolRecord, ToolStatus,
};
pub use suite::EvalSuite;
pub use tact_protocol::PermissionMode;

/// Run every case `attempts` times and grade each attempt.
///
/// Cases run sequentially. Two at once would be faster, but each would need its
/// own process-global config, and the in-process runner cannot have that; the
/// parallelism belongs one level up, in a caller that runs several `tact-eval`
/// processes.
pub async fn run_suite(
    suite: &EvalSuite,
    runner: &dyn Runner,
    attempts: u32,
) -> anyhow::Result<EvalReport> {
    // Not clamped. `0` would produce a report with no outcomes, which reads as
    // "every case passed nothing" and compares as an empty suite; the CLI
    // rejects the flag, and a library caller gets the same refusal instead of a
    // silently different run.
    if attempts == 0 {
        anyhow::bail!("attempts must be at least 1");
    }
    let started_at = chrono::Utc::now().to_rfc3339();
    let identity = runner.identity();
    let suite_fingerprint = suite.fingerprint()?;
    let mut outcomes = Vec::with_capacity(suite.cases().len() * attempts as usize);

    for case in suite.cases() {
        for attempt in 1..=attempts {
            let outcome = runner.run(case, attempt).await;
            let grades = grader::grade(case, &outcome.work_dir, &outcome.artifact).await;
            let judged_on_answer = case
                .graders
                .iter()
                .any(|grader| matches!(grader, GraderSpec::Answer { .. }));
            outcomes.push(CaseOutcome::from_parts(
                outcome.artifact,
                grades,
                judged_on_answer,
            ));

            // The work dir is scratch: a run leaves a full source tree plus a
            // SQLite session behind, and a suite of any size would fill the
            // disk. The report keeps the file-level grades, not the tree.
            let _ = std::fs::remove_dir_all(&outcome.work_dir);
        }
    }

    Ok(EvalReport::new(
        ReportMeta {
            runner: runner.kind(),
            identity,
            attempts,
            suite_fingerprint,
            harness_version: Some(env!("CARGO_PKG_VERSION").to_string()),
            started_at,
            finished_at: chrono::Utc::now().to_rfc3339(),
        },
        outcomes,
    ))
}
