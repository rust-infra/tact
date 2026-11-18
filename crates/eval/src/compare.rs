//! Comparison of two evaluation reports at the same declared conditions.
//!
//! A comparison is only meaningful when both reports describe the same suite,
//! runner, attempts, and run identity. When any of those differ the result is
//! [`ComparisonStatus::Incomparable`] rather than a fabricated pass or
//! regression, and every independent mismatch is reported.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::report::{CaseSummary, REPORT_SCHEMA_VERSION};
use crate::{CaseCost, EvalReport, RunIdentity, RunnerKind};

pub const COMPARISON_SCHEMA_VERSION: u32 = 1;

/// The comparison's overall gate result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComparisonStatus {
    Pass,
    Regression,
    Inconclusive,
    Incomparable,
}

impl ComparisonStatus {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Regression => "regression",
            Self::Inconclusive => "inconclusive",
            Self::Incomparable => "incomparable",
        }
    }
}

impl fmt::Display for ComparisonStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The comparison result for one case.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaseComparisonStatus {
    Improved,
    Unchanged,
    Regressed,
    Inconclusive,
}

impl CaseComparisonStatus {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Improved => "improved",
            Self::Unchanged => "unchanged",
            Self::Regressed => "regressed",
            Self::Inconclusive => "inconclusive",
        }
    }
}

impl fmt::Display for CaseComparisonStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Why a comparison is incomparable, or why one case did not compare cleanly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComparisonReasonKind {
    SchemaMismatch,
    SuiteFingerprintMismatch,
    AttemptMismatch,
    RunnerMismatch,
    IdentityMismatch,
    UnknownIdentity,
    CaseSetMismatch,
    BaselineInconclusive,
    CandidateInconclusive,
    FailureIncrease,
}

impl ComparisonReasonKind {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::SchemaMismatch => "schema_mismatch",
            Self::SuiteFingerprintMismatch => "suite_fingerprint_mismatch",
            Self::AttemptMismatch => "attempt_mismatch",
            Self::RunnerMismatch => "runner_mismatch",
            Self::IdentityMismatch => "identity_mismatch",
            Self::UnknownIdentity => "unknown_identity",
            Self::CaseSetMismatch => "case_set_mismatch",
            Self::BaselineInconclusive => "baseline_inconclusive",
            Self::CandidateInconclusive => "candidate_inconclusive",
            Self::FailureIncrease => "failure_increase",
        }
    }
}

impl fmt::Display for ComparisonReasonKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The reproducibility fields copied from a report into a comparison.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportHeader {
    pub report_schema_version: u32,
    pub runner: RunnerKind,
    pub identity: RunIdentity,
    pub attempts: u32,
    pub suite_fingerprint: String,
    /// Recorded for attribution, and deliberately not a comparability
    /// condition: a version bump is what a nightly gate is usually measuring.
    #[serde(default)]
    pub harness_version: Option<String>,
    pub started_at: String,
    pub finished_at: String,
}

impl From<&EvalReport> for ReportHeader {
    fn from(report: &EvalReport) -> Self {
        Self {
            // The report's schema version is separate from the comparison's.
            report_schema_version: report.schema_version,
            runner: report.meta.runner,
            identity: report.meta.identity.clone(),
            attempts: report.meta.attempts,
            suite_fingerprint: report.meta.suite_fingerprint.clone(),
            harness_version: report.meta.harness_version.clone(),
            started_at: report.meta.started_at.clone(),
            finished_at: report.meta.finished_at.clone(),
        }
    }
}

/// Attempt totals and their verdict split.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaseCounts {
    pub attempts: u32,
    pub passes: u32,
    pub failures: u32,
    pub inconclusive: u32,
}

/// One case's before/after counts and signed differences.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaseDelta {
    pub case_id: String,
    pub status: CaseComparisonStatus,
    pub baseline: CaseCounts,
    pub candidate: CaseCounts,
    pub passes_delta: i64,
    pub failures_delta: i64,
    pub inconclusive_delta: i64,
    pub baseline_cost: CaseCost,
    pub candidate_cost: CaseCost,
}

/// A mismatch or a case-level explanation attached to the comparison.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComparisonReason {
    pub kind: ComparisonReasonKind,
    pub case_id: Option<String>,
    pub message: String,
}

/// The complete, self-describing result of comparing two reports.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComparisonReport {
    pub schema_version: u32,
    pub status: ComparisonStatus,
    pub baseline: ReportHeader,
    pub candidate: ReportHeader,
    pub baseline_case_count: usize,
    pub candidate_case_count: usize,
    pub baseline_counts: CaseCounts,
    pub candidate_counts: CaseCounts,
    /// Suite-level cost on each side, for the run-by-run number a reviewer
    /// reads first.
    pub baseline_cost: CaseCost,
    pub candidate_cost: CaseCost,
    pub cases: Vec<CaseDelta>,
    pub reasons: Vec<ComparisonReason>,
}

impl ComparisonReport {
    /// A terminal summary with stable labels for scripts and people.
    pub fn to_text(&self) -> String {
        use std::fmt::Write as _;

        let mut out = String::new();
        let _ = writeln!(
            out,
            "comparison schema={} status={}",
            self.schema_version, self.status
        );
        let _ = writeln!(out, "baseline: {}", header_text(&self.baseline));
        let _ = writeln!(out, "candidate: {}", header_text(&self.candidate));
        let _ = writeln!(
            out,
            "cases: baseline={} candidate={}",
            self.baseline_case_count, self.candidate_case_count
        );
        let _ = writeln!(
            out,
            "counts: baseline attempts={} passes={} failures={} inconclusive={}",
            self.baseline_counts.attempts,
            self.baseline_counts.passes,
            self.baseline_counts.failures,
            self.baseline_counts.inconclusive
        );
        let _ = writeln!(
            out,
            "        candidate attempts={} passes={} failures={} inconclusive={}",
            self.candidate_counts.attempts,
            self.candidate_counts.passes,
            self.candidate_counts.failures,
            self.candidate_counts.inconclusive
        );
        // Cost is reported, never gated. Its own lines so a script parsing the
        // stable `counts:` labels is unaffected.
        let _ = writeln!(
            out,
            "cost: baseline tokens={} per-case-median={} median-ms={}",
            self.baseline_cost.total_tokens,
            optional_count(self.baseline_cost.median_tokens),
            self.baseline_cost.median_duration_ms
        );
        let _ = writeln!(
            out,
            "      candidate tokens={} per-case-median={} median-ms={}",
            self.candidate_cost.total_tokens,
            optional_count(self.candidate_cost.median_tokens),
            self.candidate_cost.median_duration_ms
        );

        for case in &self.cases {
            let _ = writeln!(
                out,
                "case {}: {} passes {}->{} ({:+}), failures {}->{} ({:+}), inconclusive {}->{} ({:+}), tokens {}->{} total, {}->{}ms median",
                case.case_id,
                case.status,
                case.baseline.passes,
                case.candidate.passes,
                case.passes_delta,
                case.baseline.failures,
                case.candidate.failures,
                case.failures_delta,
                case.baseline.inconclusive,
                case.candidate.inconclusive,
                case.inconclusive_delta,
                optional_count(case.baseline_cost.median_tokens),
                optional_count(case.candidate_cost.median_tokens),
                case.baseline_cost.median_duration_ms,
                case.candidate_cost.median_duration_ms
            );
        }

        if !self.reasons.is_empty() {
            let _ = writeln!(out, "reasons:");
            for reason in &self.reasons {
                let _ = writeln!(
                    out,
                    "  {} [{}]: {}",
                    reason.kind,
                    reason.case_id.as_deref().unwrap_or("report"),
                    reason.message
                );
            }
        }

        out
    }
}

/// A count that the runner may not have measured, spelled `unset` when absent.
fn optional_count(value: Option<u64>) -> String {
    value.map_or_else(|| "unset".into(), |value| value.to_string())
}

fn header_text(header: &ReportHeader) -> String {
    format!(
        "schema={} runner={} model={} endpoint={} mode={} attempts={} suite={} started={} finished={}",
        header.report_schema_version,
        header.runner,
        header.identity.model.as_deref().unwrap_or("unset"),
        header.identity.base_url.as_deref().unwrap_or("unset"),
        header.identity.permission_mode.as_str(),
        header.attempts,
        header.suite_fingerprint,
        header.started_at,
        header.finished_at
    )
}

/// Compare a baseline report with a candidate report under the declared policy.
///
/// Incomparable inputs return no case deltas, even when some cases could be
/// compared in isolation: a partial comparison would report a trustworthy
/// number for a run that was not actually comparable.
pub fn compare_reports(baseline: &EvalReport, candidate: &EvalReport) -> ComparisonReport {
    let baseline_summaries = baseline.summaries();
    let candidate_summaries = candidate.summaries();
    let baseline_counts = counts(&baseline_summaries);
    let candidate_counts = counts(&candidate_summaries);
    let mut reasons = Vec::new();

    if baseline.schema_version != REPORT_SCHEMA_VERSION
        || candidate.schema_version != REPORT_SCHEMA_VERSION
    {
        reasons.push(ComparisonReason {
            kind: ComparisonReasonKind::SchemaMismatch,
            case_id: None,
            message: format!(
                "baseline report schema {} and candidate report schema {} must both be {}",
                baseline.schema_version, candidate.schema_version, REPORT_SCHEMA_VERSION
            ),
        });
    }

    if baseline.meta.suite_fingerprint != candidate.meta.suite_fingerprint {
        reasons.push(ComparisonReason {
            kind: ComparisonReasonKind::SuiteFingerprintMismatch,
            case_id: None,
            message: format!(
                "baseline suite {} differs from candidate suite {}",
                baseline.meta.suite_fingerprint, candidate.meta.suite_fingerprint
            ),
        });
    }

    if attempts_mismatch(
        baseline,
        candidate,
        &baseline_summaries,
        &candidate_summaries,
    ) {
        reasons.push(ComparisonReason {
            kind: ComparisonReasonKind::AttemptMismatch,
            case_id: None,
            message: format!(
                "baseline attempts {} and candidate attempts {} differ from each other or observed case counts",
                baseline.meta.attempts, candidate.meta.attempts
            ),
        });
    }

    if baseline.meta.runner != candidate.meta.runner {
        reasons.push(ComparisonReason {
            kind: ComparisonReasonKind::RunnerMismatch,
            case_id: None,
            message: format!(
                "baseline runner {} differs from candidate runner {}",
                baseline.meta.runner, candidate.meta.runner
            ),
        });
    }

    if identity_mismatch(&baseline.meta.identity, &candidate.meta.identity) {
        reasons.push(ComparisonReason {
            kind: ComparisonReasonKind::IdentityMismatch,
            case_id: None,
            message: "baseline and candidate permission, model, or endpoint differ".into(),
        });
    }

    if identity_unknown(&baseline.meta.identity) || identity_unknown(&candidate.meta.identity) {
        reasons.push(ComparisonReason {
            kind: ComparisonReasonKind::UnknownIdentity,
            case_id: None,
            message: "baseline or candidate does not record both model and endpoint".into(),
        });
    }

    let baseline_ids: Vec<&str> = baseline_summaries
        .iter()
        .map(|summary| summary.case_id.as_str())
        .collect();
    let candidate_ids: Vec<&str> = candidate_summaries
        .iter()
        .map(|summary| summary.case_id.as_str())
        .collect();
    if baseline_ids != candidate_ids {
        reasons.push(ComparisonReason {
            kind: ComparisonReasonKind::CaseSetMismatch,
            case_id: None,
            message: format!(
                "baseline cases {:?} differ from candidate cases {:?}",
                baseline_ids, candidate_ids
            ),
        });
    }

    let baseline_inconclusive: u32 = baseline_summaries
        .iter()
        .map(|summary| summary.inconclusive)
        .sum();
    if baseline_inconclusive > 0 {
        reasons.push(ComparisonReason {
            kind: ComparisonReasonKind::BaselineInconclusive,
            case_id: None,
            message: format!(
                "baseline has {baseline_inconclusive} inconclusive attempt(s), so its verdict is unknown"
            ),
        });
    }

    let incomparable = !reasons.is_empty();
    let mut cases = Vec::new();

    if !incomparable {
        let candidate_by_id: BTreeMap<&str, &CaseSummary> = candidate_summaries
            .iter()
            .map(|summary| (summary.case_id.as_str(), summary))
            .collect();

        for baseline_case in &baseline_summaries {
            let candidate_case = candidate_by_id[baseline_case.case_id.as_str()];
            let baseline_case_counts = case_counts(baseline_case);
            let candidate_case_counts = case_counts(candidate_case);

            let status = if candidate_case.failures > baseline_case.failures {
                reasons.push(ComparisonReason {
                    kind: ComparisonReasonKind::FailureIncrease,
                    case_id: Some(baseline_case.case_id.clone()),
                    message: format!(
                        "failures increased from {} to {}",
                        baseline_case.failures, candidate_case.failures
                    ),
                });
                CaseComparisonStatus::Regressed
            } else if candidate_case.inconclusive > 0 {
                reasons.push(ComparisonReason {
                    kind: ComparisonReasonKind::CandidateInconclusive,
                    case_id: Some(baseline_case.case_id.clone()),
                    message: format!(
                        "candidate has {} inconclusive attempt(s)",
                        candidate_case.inconclusive
                    ),
                });
                CaseComparisonStatus::Inconclusive
            } else if candidate_case.failures < baseline_case.failures {
                CaseComparisonStatus::Improved
            } else {
                CaseComparisonStatus::Unchanged
            };

            cases.push(CaseDelta {
                case_id: baseline_case.case_id.clone(),
                status,
                passes_delta: delta(candidate_case.passes, baseline_case.passes),
                failures_delta: delta(candidate_case.failures, baseline_case.failures),
                inconclusive_delta: delta(candidate_case.inconclusive, baseline_case.inconclusive),
                baseline: baseline_case_counts,
                candidate: candidate_case_counts,
                baseline_cost: baseline_case.cost.clone(),
                candidate_cost: candidate_case.cost.clone(),
            });
        }
    }

    let status = if incomparable {
        ComparisonStatus::Incomparable
    } else if cases
        .iter()
        .any(|case| case.status == CaseComparisonStatus::Regressed)
    {
        ComparisonStatus::Regression
    } else if cases
        .iter()
        .any(|case| case.status == CaseComparisonStatus::Inconclusive)
    {
        ComparisonStatus::Inconclusive
    } else {
        ComparisonStatus::Pass
    };

    ComparisonReport {
        schema_version: COMPARISON_SCHEMA_VERSION,
        status,
        baseline: baseline.into(),
        candidate: candidate.into(),
        baseline_case_count: baseline_summaries.len(),
        candidate_case_count: candidate_summaries.len(),
        baseline_counts,
        candidate_counts,
        baseline_cost: suite_cost(&baseline_summaries),
        candidate_cost: suite_cost(&candidate_summaries),
        cases,
        reasons,
    }
}

/// Suite-level cost: the cases' medians added up, plus their token totals.
///
/// Summing medians rather than re-medianing every attempt keeps the suite
/// number a sum of case costs, which is what a reader adds up on the per-case
/// lines above it.
fn suite_cost(summaries: &[CaseSummary]) -> CaseCost {
    let mut cost = CaseCost::default();
    for summary in summaries {
        cost.attempts += summary.cost.attempts;
        cost.total_tokens += summary.cost.total_tokens;
        cost.median_tokens = match (cost.median_tokens, summary.cost.median_tokens) {
            (Some(total), Some(case)) => Some(total + case),
            (Some(total), None) => Some(total),
            (None, case) => case,
        };
        cost.median_duration_ms += summary.cost.median_duration_ms;
    }
    cost
}

/// The process exit code corresponding to a comparison result.
pub const fn comparison_exit_code(status: ComparisonStatus) -> i32 {
    match status {
        ComparisonStatus::Pass => 0,
        ComparisonStatus::Regression => 1,
        ComparisonStatus::Inconclusive => 3,
        ComparisonStatus::Incomparable => 2,
    }
}

fn delta(candidate: u32, baseline: u32) -> i64 {
    i64::from(candidate) - i64::from(baseline)
}

fn case_counts(summary: &CaseSummary) -> CaseCounts {
    CaseCounts {
        attempts: summary.attempts,
        passes: summary.passes,
        failures: summary.failures,
        inconclusive: summary.inconclusive,
    }
}

fn counts(summaries: &[CaseSummary]) -> CaseCounts {
    summaries.iter().fold(
        CaseCounts {
            attempts: 0,
            passes: 0,
            failures: 0,
            inconclusive: 0,
        },
        |mut total, summary| {
            total.attempts += summary.attempts;
            total.passes += summary.passes;
            total.failures += summary.failures;
            total.inconclusive += summary.inconclusive;
            total
        },
    )
}

fn attempts_mismatch(
    baseline: &EvalReport,
    candidate: &EvalReport,
    baseline_summaries: &[CaseSummary],
    candidate_summaries: &[CaseSummary],
) -> bool {
    if baseline.meta.attempts != candidate.meta.attempts {
        return true;
    }
    if baseline_summaries
        .iter()
        .any(|summary| summary.attempts != baseline.meta.attempts)
        || candidate_summaries
            .iter()
            .any(|summary| summary.attempts != candidate.meta.attempts)
    {
        return true;
    }

    let candidate_by_id: BTreeMap<&str, &CaseSummary> = candidate_summaries
        .iter()
        .map(|summary| (summary.case_id.as_str(), summary))
        .collect();
    baseline_summaries.iter().any(|summary| {
        candidate_by_id
            .get(summary.case_id.as_str())
            .is_some_and(|candidate| candidate.attempts != summary.attempts)
    })
}

fn identity_mismatch(baseline: &RunIdentity, candidate: &RunIdentity) -> bool {
    if baseline.permission_mode != candidate.permission_mode {
        return true;
    }

    let model_differs = matches!(
        (&baseline.model, &candidate.model),
        (Some(baseline), Some(candidate)) if baseline != candidate
    );
    let endpoint_differs = matches!(
        (&baseline.base_url, &candidate.base_url),
        (Some(baseline), Some(candidate)) if baseline != candidate
    );
    model_differs || endpoint_differs
}

fn identity_unknown(identity: &RunIdentity) -> bool {
    identity.model.is_none() || identity.base_url.is_none()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CaseOutcome, Grade, ReportMeta, TokenTotals, Verdict};
    use tact_protocol::PermissionMode;

    fn outcome(case_id: &str, attempt: u32, verdict: Verdict) -> CaseOutcome {
        CaseOutcome {
            case_id: case_id.into(),
            attempt,
            judged_on_answer: true,
            grades: vec![Grade {
                grader: "g".into(),
                verdict,
            }],
            answer: "a".into(),
            session_id: None,
            tokens: TokenTotals::default(),
            duration_ms: 1,
            stop: None,
            turns: None,
            tools: Vec::new(),
            failure: None,
        }
    }

    fn report(attempts: u32, outcomes: Vec<CaseOutcome>) -> EvalReport {
        EvalReport::new(
            ReportMeta {
                runner: RunnerKind::Host,
                identity: RunIdentity {
                    model: Some("m".into()),
                    base_url: Some("u".into()),
                    permission_mode: PermissionMode::Auto,
                },
                attempts,
                suite_fingerprint: "fp".into(),
                harness_version: Some("2.0.6".into()),
                started_at: "s".into(),
                finished_at: "f".into(),
            },
            outcomes,
        )
    }

    fn case_report(case_id: &str, verdicts: &[Verdict]) -> EvalReport {
        report(
            verdicts.len() as u32,
            verdicts
                .iter()
                .enumerate()
                .map(|(index, verdict)| outcome(case_id, index as u32 + 1, verdict.clone()))
                .collect(),
        )
    }

    fn pass() -> Verdict {
        Verdict::Pass
    }

    fn fail() -> Verdict {
        Verdict::Fail {
            reason: "failed".into(),
        }
    }

    fn inconclusive() -> Verdict {
        Verdict::Inconclusive {
            reason: "unknown".into(),
        }
    }

    #[test]
    fn exit_codes_are_distinct_and_stable() {
        assert_eq!(comparison_exit_code(ComparisonStatus::Pass), 0);
        assert_eq!(comparison_exit_code(ComparisonStatus::Regression), 1);
        assert_eq!(comparison_exit_code(ComparisonStatus::Inconclusive), 3);
        assert_eq!(comparison_exit_code(ComparisonStatus::Incomparable), 2);
    }

    #[test]
    fn enums_serialize_as_snake_case() {
        for (value, expected) in [
            (
                serde_json::to_value(ComparisonStatus::Pass).unwrap(),
                "pass",
            ),
            (
                serde_json::to_value(ComparisonStatus::Regression).unwrap(),
                "regression",
            ),
            (
                serde_json::to_value(ComparisonStatus::Inconclusive).unwrap(),
                "inconclusive",
            ),
            (
                serde_json::to_value(ComparisonStatus::Incomparable).unwrap(),
                "incomparable",
            ),
            (
                serde_json::to_value(CaseComparisonStatus::Improved).unwrap(),
                "improved",
            ),
            (
                serde_json::to_value(CaseComparisonStatus::Unchanged).unwrap(),
                "unchanged",
            ),
            (
                serde_json::to_value(CaseComparisonStatus::Regressed).unwrap(),
                "regressed",
            ),
            (
                serde_json::to_value(CaseComparisonStatus::Inconclusive).unwrap(),
                "inconclusive",
            ),
            (
                serde_json::to_value(ComparisonReasonKind::SchemaMismatch).unwrap(),
                "schema_mismatch",
            ),
            (
                serde_json::to_value(ComparisonReasonKind::SuiteFingerprintMismatch).unwrap(),
                "suite_fingerprint_mismatch",
            ),
            (
                serde_json::to_value(ComparisonReasonKind::AttemptMismatch).unwrap(),
                "attempt_mismatch",
            ),
            (
                serde_json::to_value(ComparisonReasonKind::RunnerMismatch).unwrap(),
                "runner_mismatch",
            ),
            (
                serde_json::to_value(ComparisonReasonKind::IdentityMismatch).unwrap(),
                "identity_mismatch",
            ),
            (
                serde_json::to_value(ComparisonReasonKind::UnknownIdentity).unwrap(),
                "unknown_identity",
            ),
            (
                serde_json::to_value(ComparisonReasonKind::CaseSetMismatch).unwrap(),
                "case_set_mismatch",
            ),
            (
                serde_json::to_value(ComparisonReasonKind::BaselineInconclusive).unwrap(),
                "baseline_inconclusive",
            ),
            (
                serde_json::to_value(ComparisonReasonKind::CandidateInconclusive).unwrap(),
                "candidate_inconclusive",
            ),
            (
                serde_json::to_value(ComparisonReasonKind::FailureIncrease).unwrap(),
                "failure_increase",
            ),
        ] {
            assert_eq!(value, expected);
        }
    }

    #[test]
    fn identical_passing_reports_pass() {
        let baseline = case_report("a", &[pass(), pass()]);
        let comparison = compare_reports(&baseline, &baseline);

        assert_eq!(comparison.status, ComparisonStatus::Pass);
        assert_eq!(comparison.cases.len(), 1);
        assert_eq!(comparison.cases[0].status, CaseComparisonStatus::Unchanged);
        assert_eq!(comparison.baseline_case_count, 1);
        assert_eq!(comparison.candidate_case_count, 1);
        assert!(comparison.reasons.is_empty());
    }

    #[test]
    fn failure_regression_wins_over_candidate_inconclusive() {
        let baseline = case_report("a", &[pass(), pass()]);
        let candidate = case_report("a", &[fail(), inconclusive()]);
        let comparison = compare_reports(&baseline, &candidate);

        assert_eq!(comparison.cases[0].status, CaseComparisonStatus::Regressed);
        assert_eq!(comparison.status, ComparisonStatus::Regression);
        assert_eq!(
            comparison.reasons[0].kind,
            ComparisonReasonKind::FailureIncrease
        );
    }

    #[test]
    fn a_lone_candidate_inconclusive_is_inconclusive() {
        let baseline = case_report("a", &[pass(), pass()]);
        let candidate = case_report("a", &[pass(), inconclusive()]);
        let comparison = compare_reports(&baseline, &candidate);

        assert_eq!(
            comparison.cases[0].status,
            CaseComparisonStatus::Inconclusive
        );
        assert_eq!(comparison.status, ComparisonStatus::Inconclusive);
        assert_eq!(
            comparison.reasons[0].kind,
            ComparisonReasonKind::CandidateInconclusive
        );
    }

    #[test]
    fn fewer_failures_are_improved() {
        let baseline = case_report("a", &[fail(), pass()]);
        let candidate = case_report("a", &[pass(), pass()]);
        let comparison = compare_reports(&baseline, &candidate);

        assert_eq!(comparison.cases[0].status, CaseComparisonStatus::Improved);
        assert_eq!(comparison.status, ComparisonStatus::Pass);
    }

    #[test]
    fn baseline_inconclusive_is_incomparable() {
        let baseline = case_report("a", &[pass(), inconclusive()]);
        let candidate = case_report("a", &[pass(), pass()]);
        let comparison = compare_reports(&baseline, &candidate);

        assert_eq!(comparison.status, ComparisonStatus::Incomparable);
        assert!(comparison.cases.is_empty());
        assert_eq!(
            comparison.reasons[0].kind,
            ComparisonReasonKind::BaselineInconclusive
        );
    }

    #[test]
    fn schema_mismatch_is_incomparable() {
        let baseline = case_report("a", &[pass()]);
        let mut candidate = baseline.clone();
        candidate.schema_version = 0;
        let comparison = compare_reports(&baseline, &candidate);

        assert_eq!(comparison.status, ComparisonStatus::Incomparable);
        assert_eq!(
            comparison.reasons[0].kind,
            ComparisonReasonKind::SchemaMismatch
        );
    }

    #[test]
    fn suite_fingerprint_mismatch_is_incomparable() {
        let baseline = case_report("a", &[pass()]);
        let mut candidate = baseline.clone();
        candidate.meta.suite_fingerprint = "other".into();
        let comparison = compare_reports(&baseline, &candidate);

        assert_eq!(
            comparison.reasons[0].kind,
            ComparisonReasonKind::SuiteFingerprintMismatch
        );
    }

    #[test]
    fn attempt_mismatch_is_incomparable_for_meta_and_observed_counts() {
        let baseline = case_report("a", &[pass()]);

        let mut meta_mismatch = baseline.clone();
        meta_mismatch.meta.attempts = 2;
        assert_eq!(
            compare_reports(&baseline, &meta_mismatch).reasons[0].kind,
            ComparisonReasonKind::AttemptMismatch
        );

        let mut observed_mismatch = baseline.clone();
        observed_mismatch.meta.attempts = 2;
        observed_mismatch.outcomes.push(outcome("a", 2, pass()));
        let mut candidate = observed_mismatch.clone();
        candidate.meta.attempts = 2;
        candidate.outcomes.pop();
        assert_eq!(
            compare_reports(&observed_mismatch, &candidate).reasons[0].kind,
            ComparisonReasonKind::AttemptMismatch
        );
    }

    #[test]
    fn runner_mismatch_is_incomparable() {
        let baseline = case_report("a", &[pass()]);
        let mut candidate = baseline.clone();
        candidate.meta.runner = RunnerKind::Cli;
        let comparison = compare_reports(&baseline, &candidate);

        assert_eq!(
            comparison.reasons[0].kind,
            ComparisonReasonKind::RunnerMismatch
        );
    }

    #[test]
    fn identity_mismatch_is_incomparable() {
        let baseline = case_report("a", &[pass()]);

        let mut permission = baseline.clone();
        permission.meta.identity.permission_mode = PermissionMode::Plan;
        assert_eq!(
            compare_reports(&baseline, &permission).reasons[0].kind,
            ComparisonReasonKind::IdentityMismatch
        );

        let mut model = baseline.clone();
        model.meta.identity.model = Some("other".into());
        assert_eq!(
            compare_reports(&baseline, &model).reasons[0].kind,
            ComparisonReasonKind::IdentityMismatch
        );

        let mut endpoint = baseline.clone();
        endpoint.meta.identity.base_url = Some("other".into());
        assert_eq!(
            compare_reports(&baseline, &endpoint).reasons[0].kind,
            ComparisonReasonKind::IdentityMismatch
        );

        let mut unknown = baseline.clone();
        unknown.meta.identity.model = None;
        assert_eq!(
            compare_reports(&baseline, &unknown).reasons[0].kind,
            ComparisonReasonKind::UnknownIdentity
        );
    }

    #[test]
    fn case_set_mismatch_is_incomparable() {
        let baseline = case_report("a", &[pass()]);
        let candidate = case_report("b", &[pass()]);
        let comparison = compare_reports(&baseline, &candidate);

        assert_eq!(
            comparison.reasons[0].kind,
            ComparisonReasonKind::CaseSetMismatch
        );
    }

    #[test]
    fn all_independent_global_reasons_are_collected_in_order() {
        let baseline = report(
            2,
            vec![
                outcome("a", 1, pass()),
                outcome("a", 2, fail()),
                outcome("b", 1, pass()),
                outcome("b", 2, inconclusive()),
            ],
        );
        let mut baseline = baseline;
        baseline.schema_version = 0;

        let mut candidate = report(
            3,
            vec![
                outcome("other", 1, pass()),
                outcome("other", 2, pass()),
                outcome("other", 3, pass()),
            ],
        );
        candidate.meta.suite_fingerprint = "other".into();
        candidate.meta.runner = RunnerKind::Cli;
        candidate.meta.identity.permission_mode = PermissionMode::Plan;
        candidate.meta.identity.model = None;
        candidate.meta.identity.base_url = Some("other".into());

        let comparison = compare_reports(&baseline, &candidate);
        let kinds: Vec<_> = comparison
            .reasons
            .iter()
            .map(|reason| reason.kind)
            .collect();

        assert_eq!(
            kinds,
            [
                ComparisonReasonKind::SchemaMismatch,
                ComparisonReasonKind::SuiteFingerprintMismatch,
                ComparisonReasonKind::AttemptMismatch,
                ComparisonReasonKind::RunnerMismatch,
                ComparisonReasonKind::IdentityMismatch,
                ComparisonReasonKind::UnknownIdentity,
                ComparisonReasonKind::CaseSetMismatch,
                ComparisonReasonKind::BaselineInconclusive,
            ]
        );
        assert!(comparison.cases.is_empty());
    }

    #[test]
    fn counts_and_signed_deltas_are_exact() {
        let baseline = report(
            3,
            vec![
                outcome("a", 1, pass()),
                outcome("a", 2, pass()),
                outcome("a", 3, fail()),
                outcome("b", 1, pass()),
                outcome("b", 2, pass()),
                outcome("b", 3, pass()),
                outcome("c", 1, pass()),
                outcome("c", 2, pass()),
                outcome("c", 3, pass()),
            ],
        );
        let candidate = report(
            3,
            vec![
                outcome("a", 1, pass()),
                outcome("a", 2, fail()),
                outcome("a", 3, fail()),
                outcome("b", 1, pass()),
                outcome("b", 2, pass()),
                outcome("b", 3, fail()),
                outcome("c", 1, pass()),
                outcome("c", 2, pass()),
                outcome("c", 3, inconclusive()),
            ],
        );
        let comparison = compare_reports(&baseline, &candidate);

        assert_eq!(
            comparison.baseline_counts,
            CaseCounts {
                attempts: 9,
                passes: 8,
                failures: 1,
                inconclusive: 0,
            }
        );
        assert_eq!(
            comparison.candidate_counts,
            CaseCounts {
                attempts: 9,
                passes: 5,
                failures: 3,
                inconclusive: 1,
            }
        );
        assert_eq!(comparison.cases[0].passes_delta, -1);
        assert_eq!(comparison.cases[0].failures_delta, 1);
        assert_eq!(comparison.cases[0].inconclusive_delta, 0);
        assert_eq!(
            comparison.cases[2].status,
            CaseComparisonStatus::Inconclusive
        );
        assert_eq!(comparison.cases[2].passes_delta, -1);
        assert_eq!(comparison.cases[2].failures_delta, 0);
        assert_eq!(comparison.cases[2].inconclusive_delta, 1);
        assert_eq!(comparison.status, ComparisonStatus::Regression);
    }

    #[test]
    fn cases_are_sorted_by_id() {
        let baseline = report(
            1,
            vec![
                outcome("z", 1, pass()),
                outcome("a", 1, pass()),
                outcome("m", 1, pass()),
            ],
        );
        let comparison = compare_reports(&baseline, &baseline);
        let ids: Vec<&str> = comparison
            .cases
            .iter()
            .map(|case| case.case_id.as_str())
            .collect();

        assert_eq!(ids, ["a", "m", "z"]);
    }

    #[test]
    fn comparison_carries_its_own_schema_version() {
        let baseline = case_report("a", &[pass()]);
        let comparison = compare_reports(&baseline, &baseline);
        let json = serde_json::to_value(&comparison).unwrap();

        assert_eq!(comparison.schema_version, COMPARISON_SCHEMA_VERSION);
        assert_eq!(json["schema_version"], COMPARISON_SCHEMA_VERSION);
        assert_eq!(
            json["baseline"]["report_schema_version"],
            REPORT_SCHEMA_VERSION
        );
    }

    #[test]
    fn text_summary_has_stable_status_reason_and_case_tokens() {
        let baseline = case_report("a", &[pass(), pass()]);
        let candidate = case_report("a", &[fail(), inconclusive()]);
        let text = compare_reports(&baseline, &candidate).to_text();

        assert!(text.contains("status=regression"), "{text}");
        assert!(text.contains("failure_increase"), "{text}");
        assert!(text.contains("case a: regressed"), "{text}");
        assert!(text.contains("failures 0->1"), "{text}");
    }

    #[test]
    fn a_version_bump_does_not_make_two_runs_incomparable() {
        // The whole point of a nightly gate is comparing two commits, so the
        // recorded harness version must stay out of the comparability rules.
        // It is there for attribution, not for gating.
        let baseline = case_report("a", &[pass()]);
        let mut candidate = baseline.clone();
        candidate.meta.harness_version = Some("9.9.9".into());

        let comparison = compare_reports(&baseline, &candidate);

        assert_eq!(comparison.status, ComparisonStatus::Pass);
        assert_eq!(
            comparison.candidate.harness_version.as_deref(),
            Some("9.9.9"),
            "the comparison still records which build the candidate was"
        );
    }

    #[test]
    fn cost_moves_no_status_but_is_reported() {
        // A suite that passes while spending twice the tokens is a real
        // regression a pass-rate gate hides. It is recorded, and it stays a
        // `pass`: a numeric threshold here would redden the build on a noisy
        // model.
        let baseline = case_report("a", &[pass()]);
        let mut candidate = baseline.clone();
        for outcome in &mut candidate.outcomes {
            outcome.tokens = TokenTotals {
                prompt: 10_000,
                completion: 1_000,
                ..Default::default()
            };
            outcome.duration_ms = 5_000;
        }

        let comparison = compare_reports(&baseline, &candidate);

        assert_eq!(comparison.status, ComparisonStatus::Pass);
        assert_eq!(comparison.baseline_cost.total_tokens, 0);
        assert_eq!(comparison.candidate_cost.total_tokens, 11_000);
        assert_eq!(
            comparison.cases[0].candidate_cost.median_tokens,
            Some(11_000)
        );
        assert!(
            comparison.to_text().contains("cost: baseline tokens=0"),
            "{}",
            comparison.to_text()
        );
    }

    #[test]
    fn suite_cost_adds_up_the_case_medians() {
        let baseline = case_report("a", &[pass()]);
        let mut two_cases = baseline.clone();
        two_cases.outcomes.push({
            let mut second = baseline.outcomes[0].clone();
            second.case_id = "b".into();
            second.tokens = TokenTotals {
                prompt: 100,
                completion: 0,
                ..Default::default()
            };
            second
        });

        let comparison = compare_reports(&baseline, &two_cases);

        assert_eq!(comparison.candidate_cost.median_tokens, Some(100));
        assert_eq!(comparison.candidate_cost.total_tokens, 100);
    }
}
