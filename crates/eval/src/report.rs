//! The suite result: one row per attempt, plus the reproducibility header.
//!
//! A report that omits the model, endpoint, and permission mode cannot be
//! compared with the next one, so [`ReportMeta`] records them. A runner that
//! genuinely does not know one writes `null`, which reads as "not recorded"
//! rather than as a configured blank; only the text summary spells it `unset`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::case::EvalCase;
use crate::grader::{Grade, Verdict};
use crate::runner::{RunArtifact, TokenTotals};

/// What the run was, so two reports can be compared.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportMeta {
    pub runner: crate::RunnerKind,
    /// `None` when the runner cannot see the model it ran under. Distinct from
    /// a model named "unset", and it serializes as `null` rather than a
    /// string that looks configured.
    pub model: Option<String>,
    pub base_url: Option<String>,
    pub permission_mode: tact_protocol::PermissionMode,
    pub attempts: u32,
    /// Stable hash of the case set, so a changed suite is visible without
    /// diffing two JSON files by eye.
    pub suite_fingerprint: String,
    pub started_at: String,
    pub finished_at: String,
}

/// One attempt's verdict and evidence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaseOutcome {
    pub case_id: String,
    pub attempt: u32,
    /// Whether the case that produced this attempt declared an `answer`
    /// grader. Carried because the fold in [`Self::verdict`] needs it and the
    /// grades alone cannot say (a passing file grader looks the same either
    /// way).
    pub judged_on_answer: bool,
    pub grades: Vec<Grade>,
    pub answer: String,
    pub session_id: Option<String>,
    pub tokens: TokenTotals,
    pub duration_ms: u64,
    pub stop: Option<crate::StopView>,
    pub turns: Option<u32>,
    pub tools: Vec<crate::ToolRecord>,
    pub failure: Option<crate::RunFailure>,
}

impl CaseOutcome {
    pub fn from_parts(artifact: RunArtifact, grades: Vec<Grade>, judged_on_answer: bool) -> Self {
        Self {
            case_id: artifact.case_id,
            attempt: artifact.attempt,
            judged_on_answer,
            grades,
            answer: artifact.answer,
            session_id: artifact.session_id,
            tokens: artifact.tokens,
            duration_ms: artifact.duration_ms,
            stop: artifact.stop,
            turns: artifact.turns,
            tools: artifact.tools,
            failure: artifact.failure,
        }
    }

    /// One verdict per attempt, folded from its graders.
    ///
    /// A run that failed cannot produce a failing grade for the agent: whatever
    /// the graders saw describes a half-finished attempt. So a recorded
    /// [`crate::RunFailure`] wins over every grader and makes the attempt
    /// inconclusive, which keeps a harness problem out of the model's numbers.
    ///
    /// Otherwise a single failing grader fails the attempt, and a grader that
    /// could not decide makes it inconclusive: a case is not passed on partial
    /// evidence.
    ///
    /// An `EmptyTurn` needs an `answer` grader to pass. Such a run wrote no
    /// text, so its file graders may still be right, but nothing shows the
    /// request was answered. Without an `answer` grader a pass would rest on
    /// side effects alone.
    pub fn verdict(&self) -> Verdict {
        if let Some(failure) = &self.failure {
            return Verdict::Inconclusive {
                reason: failure.to_string(),
            };
        }

        if self.stop == Some(crate::StopView::EmptyTurn) && !self.judged_on_answer {
            return Verdict::Inconclusive {
                reason: "the run produced no answer and the case declares no answer grader".into(),
            };
        }

        let mut inconclusive = None;
        for grade in &self.grades {
            match &grade.verdict {
                Verdict::Pass => {}
                Verdict::Fail { reason } => {
                    return Verdict::Fail {
                        reason: format!("{}: {reason}", grade.grader),
                    };
                }
                Verdict::Inconclusive { reason } => {
                    inconclusive.get_or_insert_with(|| reason.clone());
                }
            }
        }
        inconclusive.map_or(Verdict::Pass, |reason| Verdict::Inconclusive { reason })
    }
}

/// Bumped when the on-disk shape changes incompatibly.
///
/// A report is written by one version and read by another (a CI job's artifact,
/// an external platform's ingest), so the reader gets a number to check instead
/// of guessing from which fields happen to be present.
pub const REPORT_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalReport {
    pub schema_version: u32,
    pub meta: ReportMeta,
    pub outcomes: Vec<CaseOutcome>,
}

/// Per-case rollup across attempts.
#[derive(Debug, Clone, PartialEq)]
pub struct CaseSummary {
    pub case_id: String,
    pub passes: u32,
    pub failures: u32,
    pub inconclusive: u32,
    pub attempts: u32,
}

impl CaseSummary {
    /// `passes / attempts`, or `None` when nothing was decisive.
    pub fn pass_rate(&self) -> Option<f64> {
        let decisive = self.passes + self.failures;
        (decisive > 0).then(|| f64::from(self.passes) / f64::from(decisive))
    }
}

impl EvalReport {
    pub fn new(meta: ReportMeta, outcomes: Vec<CaseOutcome>) -> Self {
        Self {
            schema_version: REPORT_SCHEMA_VERSION,
            meta,
            outcomes,
        }
    }

    pub fn summaries(&self) -> Vec<CaseSummary> {
        let mut by_case: BTreeMap<&str, CaseSummary> = BTreeMap::new();
        for outcome in &self.outcomes {
            let summary = by_case
                .entry(outcome.case_id.as_str())
                .or_insert_with(|| CaseSummary {
                    case_id: outcome.case_id.clone(),
                    passes: 0,
                    failures: 0,
                    inconclusive: 0,
                    attempts: 0,
                });
            summary.attempts += 1;
            match outcome.verdict() {
                Verdict::Pass => summary.passes += 1,
                Verdict::Fail { .. } => summary.failures += 1,
                Verdict::Inconclusive { .. } => summary.inconclusive += 1,
            }
        }
        by_case.into_values().collect()
    }

    /// Cases where at least one attempt passed. This is `pass@k` with attempts
    /// as k, which is the number a noisy model run should be judged on.
    pub fn cases_passing_any(&self) -> usize {
        self.summaries()
            .iter()
            .filter(|summary| summary.passes > 0)
            .count()
    }

    /// Cases where every attempt passed. The stricter number, and the one a
    /// regression gate should use.
    pub fn cases_passing_all(&self) -> usize {
        self.summaries()
            .iter()
            .filter(|summary| summary.failures == 0 && summary.inconclusive == 0)
            .count()
    }

    pub fn to_json(&self) -> serde_json::Result<String> {
        serde_json::to_string_pretty(self)
    }

    /// A short terminal summary. Failure and unknown counts are separate lines
    /// so a run with a broken grader cannot read as a clean run.
    pub fn to_text(&self) -> String {
        use std::fmt::Write as _;

        let mut out = String::new();
        let _ = writeln!(
            out,
            "runner={} model={} endpoint={} mode={} attempts={}",
            self.meta.runner,
            self.meta.model.as_deref().unwrap_or("unset"),
            self.meta.base_url.as_deref().unwrap_or("unset"),
            self.meta.permission_mode,
            self.meta.attempts
        );
        let _ = writeln!(out, "suite={}", self.meta.suite_fingerprint);
        let _ = writeln!(out);

        for summary in self.summaries() {
            let rate = summary
                .pass_rate()
                .map(|rate| format!("{:.0}%", rate * 100.0))
                .unwrap_or_else(|| "n/a".into());
            let _ = writeln!(
                out,
                "{:<32} pass {}/{} ({}), fail {}, unknown {}",
                summary.case_id,
                summary.passes,
                summary.attempts,
                rate,
                summary.failures,
                summary.inconclusive
            );
        }

        let _ = writeln!(
            out,
            "\ncases passing any attempt: {}/{}\ncases passing every attempt: {}/{}",
            self.cases_passing_any(),
            self.summaries().len(),
            self.cases_passing_all(),
            self.summaries().len()
        );
        out
    }
}

/// A stable fingerprint over the case set.
///
/// Hashes the whole case, not just its id and prompt. A tightened grader, a
/// changed pattern, a different isolation mode, or a shorter timeout all change
/// what the suite decides, so all of them must change the fingerprint; otherwise
/// two reports look comparable while grading different things.
pub fn suite_fingerprint(cases: &[EvalCase]) -> String {
    use sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    for case in cases {
        // `EvalCase` is `Serialize` and its fields are the whole definition, so
        // the serialized form is the canonical tuple. A field added later is
        // covered without editing this function.
        let encoded = serde_json::to_vec(case).unwrap_or_default();
        hasher.update((encoded.len() as u64).to_le_bytes());
        hasher.update(encoded);
    }
    format!("{:x}", hasher.finalize())[..16].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Grade, Verdict};

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

    fn report(outcomes: Vec<CaseOutcome>) -> EvalReport {
        EvalReport::new(
            ReportMeta {
                runner: crate::RunnerKind::Host,
                model: Some("m".into()),
                base_url: Some("u".into()),
                permission_mode: tact_protocol::PermissionMode::Auto,
                attempts: 2,
                suite_fingerprint: "fp".into(),
                started_at: "s".into(),
                finished_at: "f".into(),
            },
            outcomes,
        )
    }

    #[test]
    fn every_report_carries_its_schema_version() {
        let report = report(Vec::new());
        let json: serde_json::Value = serde_json::from_str(&report.to_json().unwrap()).unwrap();
        assert_eq!(json["schema_version"], REPORT_SCHEMA_VERSION);
    }

    #[test]
    fn an_empty_answer_cannot_pass_a_case_that_does_not_judge_the_answer() {
        // The file grader passed, but the model wrote nothing, so nothing
        // shows the request was answered.
        let mut silent = outcome("c", 1, Verdict::Pass);
        silent.stop = Some(crate::StopView::EmptyTurn);
        silent.judged_on_answer = false;

        assert!(!silent.verdict().is_pass(), "{:?}", silent.verdict());

        // With an answer grader the verdict is that grader's to make.
        let mut judged = silent.clone();
        judged.judged_on_answer = true;
        assert!(judged.verdict().is_pass());
    }

    #[test]
    fn a_failed_run_outranks_a_failing_grader() {
        // A timed-out attempt whose grader also fails is a harness problem, not
        // a model failure, and must not be counted in the pass rate.
        let mut failed = outcome(
            "c",
            1,
            Verdict::Fail {
                reason: "file missing".into(),
            },
        );
        failed.failure = Some(crate::RunFailure::Timeout { limit_secs: 5 });

        match failed.verdict() {
            Verdict::Inconclusive { reason } => assert!(reason.contains("timed out"), "{reason}"),
            other => panic!("expected inconclusive, got {other:?}"),
        }
    }

    #[test]
    fn a_single_failing_grader_fails_the_attempt() {
        let mut one = outcome("c", 1, Verdict::Pass);
        one.grades.push(Grade {
            grader: "second".into(),
            verdict: Verdict::Fail {
                reason: "nope".into(),
            },
        });
        assert!(matches!(one.verdict(), Verdict::Fail { .. }));
    }

    #[test]
    fn an_inconclusive_grader_does_not_pass_the_attempt() {
        let one = outcome(
            "c",
            1,
            Verdict::Inconclusive {
                reason: "no grader".into(),
            },
        );
        assert!(!one.verdict().is_pass());
    }

    #[test]
    fn pass_at_k_and_pass_all_differ_when_attempts_disagree() {
        let report = report(vec![
            outcome("a", 1, Verdict::Pass),
            outcome(
                "a",
                2,
                Verdict::Fail {
                    reason: "flaky".into(),
                },
            ),
            outcome("b", 1, Verdict::Pass),
            outcome("b", 2, Verdict::Pass),
        ]);
        assert_eq!(report.cases_passing_any(), 2);
        assert_eq!(report.cases_passing_all(), 1);
    }

    #[test]
    fn pass_rate_ignores_inconclusive_attempts() {
        let report = report(vec![
            outcome("a", 1, Verdict::Pass),
            outcome(
                "a",
                2,
                Verdict::Inconclusive {
                    reason: "harness".into(),
                },
            ),
        ]);
        let summary = &report.summaries()[0];
        assert_eq!(
            summary.pass_rate(),
            Some(1.0),
            "1 pass, 0 decisive failures"
        );
    }

    #[test]
    fn the_fingerprint_changes_when_a_prompt_changes() {
        let base = EvalCase {
            id: "a".into(),
            prompt: "one".into(),
            timeout_secs: 1,
            isolation: crate::Isolation::Empty,
            graders: Vec::new(),
        };
        let mut edited = base.clone();
        edited.prompt = "two".into();

        assert_ne!(
            suite_fingerprint(std::slice::from_ref(&base)),
            suite_fingerprint(&[edited])
        );
    }

    #[test]
    fn the_fingerprint_changes_when_a_grader_or_timeout_changes() {
        // Both change what the suite decides, so both must invalidate a
        // comparison even though id and prompt are untouched.
        let base = EvalCase {
            id: "a".into(),
            prompt: "one".into(),
            timeout_secs: 1,
            isolation: crate::Isolation::Empty,
            graders: Vec::new(),
        };
        let baseline = suite_fingerprint(std::slice::from_ref(&base));

        let mut timed = base.clone();
        timed.timeout_secs = 2;
        assert_ne!(baseline, suite_fingerprint(&[timed]));

        let mut graded = base.clone();
        graded.graders = vec![crate::GraderSpec::Command {
            name: "g".into(),
            command: "true".into(),
        }];
        assert_ne!(baseline, suite_fingerprint(&[graded]));
    }
}
