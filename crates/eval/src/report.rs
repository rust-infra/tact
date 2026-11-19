//! The suite result: one row per attempt, plus the reproducibility header.
//!
//! A report that omits the model, endpoint, and permission mode cannot be
//! compared with the next one, so [`ReportMeta`] records them. A runner that
//! genuinely does not know one writes `null`, which reads as "not recorded"
//! rather than as a configured blank; only the text summary spells it `unset`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::case::EvalCase;
use crate::grader::{Grade, Verdict};
use crate::runner::{RunArtifact, TokenTotals};

/// What the run was, so two reports can be compared.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportMeta {
    pub runner: crate::RunnerKind,
    /// Embedded rather than copied field by field, so a field added to what a
    /// runner reports cannot be forgotten here.
    pub identity: crate::RunIdentity,
    pub attempts: u32,
    /// Stable hash of the case set, so a changed suite is visible without
    /// diffing two JSON files by eye.
    pub suite_fingerprint: String,
    /// The harness version that produced this report.
    ///
    /// Recorded, not compared. A nightly gate exists to compare two commits, so
    /// making a version change incomparable would disable it exactly when it
    /// matters. What this buys is attribution after the fact: a report from last
    /// month can be told apart from today's when a model alias silently changed
    /// weights, which the schema version alone cannot say.
    ///
    /// One field rather than one per crate: the workspace pins `tact`, `tact-ui`,
    /// and `tact_eval` to a single version, so two numbers would always be the
    /// same number. The exception is the CLI runner's `--binary`, which may be a
    /// release build this harness did not produce; the report cannot see that
    /// binary's version, and the README says so.
    ///
    /// `#[serde(default)]` so a report written before this field existed still
    /// parses; it simply records `None`.
    #[serde(default)]
    pub harness_version: Option<String>,
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
    /// What the attempts cost. Reported, never gated: see [`CaseCost`].
    pub cost: CaseCost,
}

/// What one case's attempts cost, as a median so one outlier does not set it.
///
/// A passing suite whose token spend doubled is a real regression that an
/// unchanged pass rate hides; a single noisy attempt is not. So the median
/// travels with the report and the comparison, and it moves no verdict: a
/// numeric threshold here would need a calibrated noise model, which this
/// harness does not claim to have.
///
/// `tokens` is prompt plus completion. Cache-hit and reasoning counts are
/// recorded on the attempt, but only the host runner can see them, so a total
/// including them would not be comparable across runners.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaseCost {
    pub attempts: u32,
    /// Median prompt+completion tokens per attempt. `None` when no attempt
    /// reported usage, which is different from a measured zero.
    pub median_tokens: Option<u64>,
    /// Median wall clock per attempt.
    pub median_duration_ms: u64,
    /// Every attempt's prompt+completion tokens added up, so a per-attempt
    /// median cannot hide a suite whose cost moved into a second attempt.
    pub total_tokens: u64,
}

impl CaseCost {
    /// Aggregate the attempts of one case.
    ///
    /// Called from [`EvalReport::summaries`], which already has every outcome
    /// for the case, so there is no running fold to keep consistent.
    fn from_attempts(attempts: impl Iterator<Item = (u64, u64)>) -> Self {
        let mut count = 0u32;
        let mut total_tokens = 0u64;
        let mut durations: Vec<u64> = Vec::new();
        let mut tokens: Vec<u64> = Vec::new();
        for (attempt_tokens, duration_ms) in attempts {
            count += 1;
            total_tokens += attempt_tokens;
            durations.push(duration_ms);
            // A zero is a runner that reported no usage for that attempt, not a
            // measured zero, so it does not get to pull the median down. If
            // every attempt reports zero, usage really was absent everywhere and
            // the median stays `None`.
            if attempt_tokens > 0 {
                tokens.push(attempt_tokens);
            }
        }
        tokens.sort_unstable();
        durations.sort_unstable();
        Self {
            attempts: count,
            median_tokens: median(&tokens),
            median_duration_ms: median(&durations).unwrap_or(0),
            total_tokens,
        }
    }
}

/// The middle sample, averaging the two middle ones for an even count.
///
/// The averaging is `a/2 + b/2 + (a%2 + b%2)/2` rather than `(a + b) / 2` so a
/// pair near `u64::MAX` cannot overflow while averaging.
fn median(sorted: &[u64]) -> Option<u64> {
    match sorted.len() {
        0 => None,
        len if !len.is_multiple_of(2) => Some(sorted[len / 2]),
        len => {
            let lower = sorted[len / 2 - 1];
            let upper = sorted[len / 2];
            Some(lower / 2 + upper / 2 + (lower % 2 + upper % 2) / 2)
        }
    }
}

impl CaseSummary {
    /// `passes / attempts`, or `None` when nothing was decisive.
    pub fn pass_rate(&self) -> Option<f64> {
        let decisive = self.passes + self.failures;
        (decisive > 0).then(|| f64::from(self.passes) / f64::from(decisive))
    }

    /// At least one attempt passed: `pass@k` for this case.
    pub fn passed_any(&self) -> bool {
        self.passes > 0
    }

    /// Every attempt passed: the strict reading, and the one a gate uses.
    pub fn passed_all(&self) -> bool {
        self.failures == 0 && self.inconclusive == 0
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
        // Group first, fold second: a median needs every attempt's cost at
        // once, so a running total would have to be re-derived anyway.
        let mut outcomes_by_case: BTreeMap<&str, Vec<&CaseOutcome>> = BTreeMap::new();
        for outcome in &self.outcomes {
            outcomes_by_case
                .entry(outcome.case_id.as_str())
                .or_default()
                .push(outcome);
        }

        outcomes_by_case
            .into_iter()
            .map(|(case_id, outcomes)| {
                let mut summary = CaseSummary {
                    case_id: case_id.to_string(),
                    passes: 0,
                    failures: 0,
                    inconclusive: 0,
                    attempts: 0,
                    cost: CaseCost::from_attempts(
                        outcomes
                            .iter()
                            .map(|outcome| (outcome.tokens.total(), outcome.duration_ms)),
                    ),
                };
                for outcome in &outcomes {
                    summary.attempts += 1;
                    match outcome.verdict() {
                        Verdict::Pass => summary.passes += 1,
                        Verdict::Fail { .. } => summary.failures += 1,
                        Verdict::Inconclusive { .. } => summary.inconclusive += 1,
                    }
                }
                summary
            })
            .collect()
    }

    /// Cases where at least one attempt passed. This is `pass@k` with attempts
    /// as k, which is the number a noisy model run should be judged on.
    pub fn cases_passing_any(&self) -> usize {
        self.summaries()
            .iter()
            .filter(|summary| summary.passed_any())
            .count()
    }

    /// Cases where every attempt passed. The stricter number, and the one a
    /// regression gate should use.
    pub fn cases_passing_all(&self) -> usize {
        self.summaries()
            .iter()
            .filter(|summary| summary.passed_all())
            .count()
    }

    /// Attempts that produced a decision, pass or fail.
    ///
    /// A run with none of these did not evaluate anything: every attempt was a
    /// timeout, a missing binary, or a grader that could not run. That is a
    /// harness failure, and a `0` exit code would report it as a suite that
    /// merely failed nothing.
    pub fn decisive_attempts(&self) -> u32 {
        self.summaries()
            .iter()
            .map(|summary| summary.passes + summary.failures)
            .sum()
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
            self.meta.identity.model.as_deref().unwrap_or("unset"),
            self.meta.identity.base_url.as_deref().unwrap_or("unset"),
            // `as_str`, not `Display`: Display is the long operator sentence
            // ("auto - allow all operations ..."), which does not belong in a
            // one-line header that the JSON spells `"auto"`.
            self.meta.identity.permission_mode.as_str(),
            self.meta.attempts
        );
        let _ = writeln!(out, "suite={}", self.meta.suite_fingerprint);
        let _ = writeln!(out);

        // One fold for the table and both totals. `summaries` sorts a median
        // per case, so calling it four times would do that work four times.
        let summaries = self.summaries();
        for summary in &summaries {
            let rate = summary
                .pass_rate()
                .map(|rate| format!("{:.0}%", rate * 100.0))
                .unwrap_or_else(|| "n/a".into());
            let _ = writeln!(
                out,
                "{:<32} pass {}/{} ({}), fail {}, unknown {}, {} tok/attempt, {}ms",
                summary.case_id,
                summary.passes,
                summary.attempts,
                rate,
                summary.failures,
                summary.inconclusive,
                summary
                    .cost
                    .median_tokens
                    .map_or_else(|| "unset".to_string(), |value| value.to_string()),
                summary.cost.median_duration_ms
            );
        }

        let any = summaries
            .iter()
            .filter(|summary| summary.passed_any())
            .count();
        let all = summaries
            .iter()
            .filter(|summary| summary.passed_all())
            .count();
        let _ = writeln!(
            out,
            "\ncases passing any attempt: {any}/{}\ncases passing every attempt: {all}/{}",
            summaries.len(),
            summaries.len()
        );
        out
    }
}

/// A stable fingerprint over the declared case set alone.
///
/// Hashes the whole case, not just its id and prompt. A tightened grader, a
/// changed pattern, a different isolation mode, or a shorter timeout all change
/// what the suite decides, so all of them must change the fingerprint; otherwise
/// two reports look comparable while grading different things.
///
/// This helper does not include fixture contents; [`crate::EvalSuite::fingerprint`]
/// does, and is what reports record. It remains public for callers that only
/// want the declared-case portion of the identity.
pub fn suite_fingerprint(cases: &[EvalCase]) -> String {
    let mut hasher = Sha256::new();
    hash_declared_cases(&mut hasher, cases);
    finish_fingerprint(hasher)
}

/// The declared-case half of the suite fingerprint.
///
/// Kept separate so [`crate::EvalSuite::fingerprint`] can continue the same
/// hash with fixture contents, and so a suite with no Copy fixtures keeps the
/// declared-only fingerprint.
pub(crate) fn hash_declared_cases(hasher: &mut Sha256, cases: &[EvalCase]) {
    for case in cases {
        // `EvalCase` is `Serialize` and its fields are the whole definition, so
        // the serialized form is the canonical tuple. A field added later is
        // covered without editing this function.
        let encoded = match serde_json::to_vec(case) {
            Ok(encoded) => encoded,
            // Unreachable while `EvalCase` is all plain data. If it ever stops
            // being serializable, hashing the debug form keeps two different
            // broken cases from colliding on an empty input.
            Err(_) => format!("{case:?}").into_bytes(),
        };
        hasher.update((encoded.len() as u64).to_le_bytes());
        hasher.update(encoded);
    }
}

pub(crate) fn finish_fingerprint(hasher: Sha256) -> String {
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
                identity: crate::RunIdentity {
                    model: Some("m".into()),
                    base_url: Some("u".into()),
                    permission_mode: tact_protocol::PermissionMode::Auto,
                },
                attempts: 2,
                suite_fingerprint: "fp".into(),
                harness_version: Some("2.0.6".into()),
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
            prompt: Some("one".into()),
            prompts: Vec::new(),
            timeout_secs: 1,
            isolation: crate::Isolation::Empty,
            graders: Vec::new(),
        };
        let mut edited = base.clone();
        edited.prompt = Some("two".into());

        assert_ne!(
            suite_fingerprint(std::slice::from_ref(&base)),
            suite_fingerprint(&[edited])
        );
    }

    #[test]
    fn the_fingerprint_covers_a_second_turn() {
        // A turn added to a conversation changes what the case decides, so a
        // comparison must not treat the short and long versions as one suite.
        let one_turn = EvalCase {
            id: "a".into(),
            prompt: None,
            prompts: vec!["first".into()],
            timeout_secs: 1,
            isolation: crate::Isolation::Empty,
            graders: Vec::new(),
        };
        let mut two_turns = one_turn.clone();
        two_turns.prompts.push("second".into());

        assert_ne!(
            suite_fingerprint(std::slice::from_ref(&one_turn)),
            suite_fingerprint(&[two_turns])
        );
    }

    #[test]
    fn a_single_prompt_case_keeps_the_fingerprint_it_had_before_turns_existed() {
        // `prompt` became `Option<String>` only so `prompts` could exist. If
        // that changed the serialized form, every recorded baseline would become
        // incomparable overnight, for no reason the suite made.
        let legacy: EvalCase = toml::from_str("prompt = \"hello\"\n").unwrap();
        let encoded = serde_json::to_string(&legacy).unwrap();

        assert!(encoded.contains("\"prompt\":\"hello\""), "{encoded}");
        assert!(
            !encoded.contains("prompts"),
            "an empty turn list must not appear in the hashed form: {encoded}"
        );
    }

    #[test]
    fn the_fingerprint_changes_when_a_grader_or_timeout_changes() {
        // Both change what the suite decides, so both must invalidate a
        // comparison even though id and prompt are untouched.
        let base = EvalCase {
            id: "a".into(),
            prompt: Some("one".into()),
            prompts: Vec::new(),
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

    /// An outcome with explicit cost, so a median test does not depend on the
    /// default `TokenTotals`.
    fn costing(case_id: &str, attempt: u32, tokens: (u64, u64), duration_ms: u64) -> CaseOutcome {
        let mut outcome = outcome(case_id, attempt, Verdict::Pass);
        outcome.tokens = TokenTotals {
            prompt: tokens.0,
            completion: tokens.1,
            ..Default::default()
        };
        outcome.duration_ms = duration_ms;
        outcome
    }

    #[test]
    fn a_case_cost_is_the_median_of_its_attempts() {
        // Median, not mean: one runaway attempt must not become the case's
        // number, and the number a reviewer reads should be the typical one.
        let report = report(vec![
            costing("a", 1, (100, 10), 1_000),
            costing("a", 2, (9_000, 900), 90_000),
            costing("a", 3, (120, 20), 1_200),
        ]);

        let cost = &report.summaries()[0].cost;

        assert_eq!(cost.attempts, 3);
        assert_eq!(cost.median_tokens, Some(140), "the typical attempt");
        assert_eq!(cost.median_duration_ms, 1_200);
        assert_eq!(cost.total_tokens, 100 + 10 + 9_000 + 900 + 120 + 20);
    }

    #[test]
    fn a_case_that_reported_no_usage_has_no_median_rather_than_zero() {
        // "This runner cannot see token counts" is not "the run cost nothing".
        // A zero would average into a suite cost as a measured value.
        let report = report(vec![costing("a", 1, (0, 0), 5)]);

        let cost = &report.summaries()[0].cost;

        assert_eq!(cost.median_tokens, None);
        assert_eq!(cost.total_tokens, 0);
        assert_eq!(cost.median_duration_ms, 5, "duration is still measured");
    }

    #[test]
    fn an_even_attempt_count_averages_the_two_middle_attempts() {
        let report = report(vec![
            costing("a", 1, (10, 0), 10),
            costing("a", 2, (20, 0), 20),
            costing("a", 3, (30, 0), 30),
            costing("a", 4, (100, 0), 100),
        ]);

        let cost = &report.summaries()[0].cost;

        assert_eq!(cost.median_tokens, Some(25));
        assert_eq!(cost.median_duration_ms, 25);
    }

    #[test]
    fn a_run_whose_every_attempt_was_inconclusive_has_no_decisive_attempt() {
        // This is what separates "the suite failed" from "the suite never
        // ran": the CLI turns it into a non-zero exit, because a missing
        // binary must not read as a clean run.
        let report = report(vec![
            outcome(
                "a",
                1,
                Verdict::Inconclusive {
                    reason: "startup failed".into(),
                },
            ),
            outcome(
                "b",
                1,
                Verdict::Inconclusive {
                    reason: "startup failed".into(),
                },
            ),
        ]);

        assert_eq!(report.decisive_attempts(), 0);
    }

    #[test]
    fn a_decisive_failure_still_counts_as_evidence() {
        // A failing attempt measured something: it is not the same as a
        // harness that could not run, and the exit code must differ.
        let report = report(vec![outcome(
            "a",
            1,
            Verdict::Fail {
                reason: "no file".into(),
            },
        )]);

        assert_eq!(report.decisive_attempts(), 1);
    }
}
