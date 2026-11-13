//! Repeatable evaluation of the Tact agent.
//!
//! Two runners produce one [`RunArtifact`]. [`HostRunner`] runs a case in
//! process and sees the typed [`tact::AgentLoopStop`]. [`CliRunner`] spawns
//! `tact-ui headless` and sees only what that process writes. Graders read a
//! [`RunArtifact`] and a work directory and never learn which runner produced
//! them, so a suite scores the same way either way.
//!
//! The public surface is three verbs: load a suite, run it, grade it.

pub mod case;
pub mod cli_runner;
pub mod grader;
pub mod host_runner;
pub mod options;
pub mod process;
pub mod report;
pub mod runner;
pub mod suite;

pub use case::{EvalCase, FileAssertion, GraderSpec, Isolation, TextAssertion};
pub use cli_runner::CliRunner;
pub use grader::{Grade, Verdict, grade};
pub use host_runner::HostRunner;
pub use options::{PermissionMode, RunOptions};
pub use report::{CaseOutcome, CaseSummary, EvalReport, ReportMeta, suite_fingerprint};
pub use runner::{
    RunArtifact, RunFailure, RunOutcome, Runner, StopView, TokenTotals, ToolRecord, ToolStatus,
};
pub use suite::EvalSuite;

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
    let attempts = attempts.max(1);
    let started_at = chrono::Utc::now().to_rfc3339();
    let identity = runner.identity();
    let mut outcomes = Vec::with_capacity(suite.cases().len() * attempts as usize);

    for case in suite.cases() {
        for attempt in 1..=attempts {
            let outcome = runner.run(case, attempt).await;
            let grades = grader::grade(case, &outcome.work_dir, &outcome.artifact).await;
            outcomes.push(CaseOutcome::from_parts(outcome.artifact, grades));

            // The work dir is scratch: a run leaves a full source tree plus a
            // SQLite session behind, and a suite of any size would fill the
            // disk. The report keeps the file-level grades, not the tree.
            let _ = std::fs::remove_dir_all(&outcome.work_dir);
        }
    }

    Ok(EvalReport::new(
        ReportMeta {
            runner: runner.name().to_string(),
            model: identity.model.unwrap_or_else(|| "unset".into()),
            base_url: identity.base_url.unwrap_or_else(|| "unset".into()),
            permission_mode: identity.permission_mode,
            attempts,
            suite_fingerprint: suite_fingerprint(suite.cases()),
            started_at,
            finished_at: chrono::Utc::now().to_rfc3339(),
        },
        outcomes,
    ))
}
