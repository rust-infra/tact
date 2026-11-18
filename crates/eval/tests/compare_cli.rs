//! The `tact-eval compare` process contract: inputs, streams, and exit codes.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use tact_eval::{
    CaseOutcome, EvalReport, Grade, PermissionMode, ReportMeta, RunIdentity, RunnerKind,
    TokenTotals, Verdict,
};

fn report(attempts: u32, suite_fingerprint: &str, verdicts: &[Verdict]) -> EvalReport {
    let outcomes = verdicts
        .iter()
        .enumerate()
        .map(|(index, verdict)| CaseOutcome {
            case_id: "case-a".into(),
            attempt: index as u32 + 1,
            judged_on_answer: true,
            grades: vec![Grade {
                grader: "answer".into(),
                verdict: verdict.clone(),
            }],
            answer: "answer".into(),
            session_id: None,
            tokens: TokenTotals::default(),
            duration_ms: 1,
            stop: None,
            turns: None,
            tools: Vec::new(),
            failure: None,
        })
        .collect();

    EvalReport::new(
        ReportMeta {
            runner: RunnerKind::Host,
            identity: RunIdentity {
                model: Some("model".into()),
                base_url: Some("http://localhost".into()),
                permission_mode: PermissionMode::Auto,
            },
            attempts,
            suite_fingerprint: suite_fingerprint.into(),
            started_at: "2026-11-18T00:00:00Z".into(),
            finished_at: "2026-11-18T00:00:01Z".into(),
        },
        outcomes,
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

fn write_report(dir: &Path, name: &str, report: &EvalReport) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, serde_json::to_string(report).unwrap()).unwrap();
    path
}

fn compare(baseline: &Path, candidate: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_tact-eval"))
        .arg("compare")
        .arg(baseline)
        .arg(candidate)
        .output()
        .unwrap()
}

fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).unwrap()
}

fn stderr(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).unwrap()
}

#[test]
fn matching_passing_reports_exit_zero() {
    let dir = tempfile::tempdir().unwrap();
    let baseline = write_report(dir.path(), "baseline.json", &report(1, "suite", &[pass()]));
    let candidate = write_report(dir.path(), "candidate.json", &report(1, "suite", &[pass()]));

    let output = compare(&baseline, &candidate);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("status=pass"),
        "{}",
        stderr(&output)
    );

    let json: serde_json::Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert_eq!(json["status"], "pass");
}

#[test]
fn a_failure_increase_exits_one() {
    let dir = tempfile::tempdir().unwrap();
    let baseline = write_report(dir.path(), "baseline.json", &report(1, "suite", &[pass()]));
    let candidate = write_report(dir.path(), "candidate.json", &report(1, "suite", &[fail()]));

    let output = compare(&baseline, &candidate);
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("status=regression"),
        "{}",
        stderr(&output)
    );

    let json: serde_json::Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert_eq!(json["status"], "regression");
}

#[test]
fn a_candidate_inconclusive_exits_three() {
    let dir = tempfile::tempdir().unwrap();
    let baseline = write_report(
        dir.path(),
        "baseline.json",
        &report(2, "suite", &[pass(), pass()]),
    );
    let candidate = write_report(
        dir.path(),
        "candidate.json",
        &report(2, "suite", &[pass(), inconclusive()]),
    );

    let output = compare(&baseline, &candidate);
    assert_eq!(output.status.code(), Some(3), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("status=inconclusive"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn mismatched_conditions_exit_two() {
    let dir = tempfile::tempdir().unwrap();
    let baseline = write_report(dir.path(), "baseline.json", &report(1, "suite", &[pass()]));
    let candidate = write_report(
        dir.path(),
        "candidate.json",
        &report(1, "other-suite", &[pass()]),
    );

    let output = compare(&baseline, &candidate);
    assert_eq!(output.status.code(), Some(2), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("status=incomparable"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn an_unreadable_report_exits_two_with_a_compare_error() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing.json");
    let valid = write_report(dir.path(), "valid.json", &report(1, "suite", &[pass()]));

    let output = compare(&missing, &valid);
    assert_eq!(output.status.code(), Some(2));
    assert!(
        stderr(&output).starts_with("tact-eval compare: cannot read report"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn a_malformed_report_exits_two_with_a_compare_error() {
    let dir = tempfile::tempdir().unwrap();
    let malformed = dir.path().join("malformed.json");
    std::fs::write(&malformed, "{not json").unwrap();
    let valid = write_report(dir.path(), "valid.json", &report(1, "suite", &[pass()]));

    let output = compare(&valid, &malformed);
    assert_eq!(output.status.code(), Some(2));
    assert!(
        stderr(&output).starts_with("tact-eval compare: cannot parse report"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn json_out_writes_pretty_json_and_keeps_stdout_empty() {
    let dir = tempfile::tempdir().unwrap();
    let baseline = write_report(dir.path(), "baseline.json", &report(1, "suite", &[pass()]));
    let candidate = write_report(dir.path(), "candidate.json", &report(1, "suite", &[pass()]));
    let comparison_path = dir.path().join("comparison.json");

    let output = Command::new(env!("CARGO_BIN_EXE_tact-eval"))
        .arg("compare")
        .arg(&baseline)
        .arg(&candidate)
        .arg("--json-out")
        .arg(&comparison_path)
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(stdout(&output), "", "the report belongs in the file");
    let written = std::fs::read_to_string(&comparison_path).unwrap();
    assert!(written.contains("\n  \"schema_version\": 1"), "{written}");
    let json: serde_json::Value = serde_json::from_str(&written).unwrap();
    assert_eq!(json["status"], "pass");
}

#[test]
fn compare_help_names_its_inputs_and_json_out_only() {
    let output = Command::new(env!("CARGO_BIN_EXE_tact-eval"))
        .args(["compare", "--help"])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(0));
    let help = stdout(&output);
    for expected in ["BASELINE", "CANDIDATE", "--json-out"] {
        assert!(help.contains(expected), "missing {expected}:\n{help}");
    }
    for absent in ["--threshold", "--gate", "--baseline", "--strict"] {
        assert!(!help.contains(absent), "unexpected {absent}:\n{help}");
    }
}
