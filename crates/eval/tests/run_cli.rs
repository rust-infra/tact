//! The `run` subcommand's process contract, against a stand-in `tact-ui`.
//!
//! `compare` had end-to-end CLI coverage and `run` had none, so the failures
//! this file pins were found by hand: a `--json-out` whose parent directory did
//! not exist failed *after* the paid run, and a run where nothing could be
//! judged exited 0 and read as a green suite.
//!
//! A shell script stands in for `tact-ui`. The point is the harness's own
//! behaviour — where it writes, what it refuses, and what it exits with — not
//! the agent, which `host_runner_end_to_end` covers against a wiremock server.

#![allow(clippy::result_large_err)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// Run `tact-eval` with `args`, from the workspace root.
fn tact_eval(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_tact-eval"))
        .args(args)
        .output()
        .expect("tact-eval runs")
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// Write an executable stand-in binary and return its path.
fn fake_binary(dir: &Path, name: &str, script: &str) -> PathBuf {
    let binary = dir.join(name);
    std::fs::write(&binary, script).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    binary
}

/// A suite of one case that passes against a binary printing `ready`.
fn suite(dir: &Path) -> PathBuf {
    let suite = dir.join("suite");
    std::fs::create_dir_all(&suite).unwrap();
    std::fs::write(
        suite.join("answers.toml"),
        r#"
prompt = "say ready"
timeout_secs = 30

[[graders]]
kind = "answer"
name = "says-ready"

[[graders.assertions]]
kind = "contains"
text = "ready"
"#,
    )
    .unwrap();
    suite
}

#[cfg(unix)]
#[test]
fn json_out_creates_a_missing_parent_directory() {
    // Every documented example writes to `target/eval/...`, and a fresh
    // checkout has no `target/eval`. Failing after the model run was paid for
    // is the worst possible moment to discover that.
    let dir = tempfile::tempdir().unwrap();
    let suite = suite(dir.path());
    let binary = fake_binary(dir.path(), "fake-tact-ui", "#!/bin/sh\necho ready\n");
    let report = dir.path().join("nested/deeper/report.json");

    let output = tact_eval(&[
        "run",
        suite.to_str().unwrap(),
        "--runner",
        "cli",
        "--binary",
        binary.to_str().unwrap(),
        "--json-out",
        report.to_str().unwrap(),
    ]);

    assert!(output.status.success(), "{}", stderr(&output));
    let written = std::fs::read_to_string(&report).expect("the report was written");
    assert!(written.contains("\"schema_version\""), "{written}");
    assert!(
        stdout(&output).trim().is_empty(),
        "--json-out must keep stdout empty, or a caller capturing it gets JSON twice"
    );
}

#[cfg(unix)]
#[test]
fn zero_attempts_is_refused_instead_of_silently_becoming_one() {
    // `0` used to be clamped to 1, so a caller asking for "no attempts" got a
    // full run and a report claiming one attempt. Refusing is the honest
    // answer, and clap reports it before anything is spent.
    let dir = tempfile::tempdir().unwrap();
    let suite = suite(dir.path());
    let binary = fake_binary(dir.path(), "fake-tact-ui", "#!/bin/sh\necho ready\n");

    let output = tact_eval(&[
        "run",
        suite.to_str().unwrap(),
        "--runner",
        "cli",
        "--binary",
        binary.to_str().unwrap(),
        "--attempts",
        "0",
    ]);

    assert_eq!(output.status.code(), Some(2), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("--attempts"),
        "the error must name the flag:\n{}",
        stderr(&output)
    );
}

#[cfg(unix)]
#[test]
fn a_run_where_nothing_could_be_judged_is_not_a_success() {
    // A missing binary makes every attempt a startup failure, so no verdict
    // exists. Exiting 0 there is the shape of a silently dead gate.
    let dir = tempfile::tempdir().unwrap();
    let suite = suite(dir.path());
    let missing = dir.path().join("does-not-exist");

    let output = tact_eval(&[
        "run",
        suite.to_str().unwrap(),
        "--runner",
        "cli",
        "--binary",
        missing.to_str().unwrap(),
    ]);

    assert_eq!(
        output.status.code(),
        Some(4),
        "expected the no-verdict exit; stderr was:\n{}",
        stderr(&output)
    );
    assert!(
        stderr(&output).contains("no attempt reached a verdict"),
        "{}",
        stderr(&output)
    );
}

#[cfg(unix)]
#[test]
fn a_decisive_failure_still_exits_zero_without_strict() {
    // The complement of the test above: a run that judged and failed is a
    // measurement, and `--strict` is what turns that into a non-zero exit.
    // Without this, a broken `if` could make every failing suite look like a
    // harness error.
    let dir = tempfile::tempdir().unwrap();
    let suite = suite(dir.path());
    let binary = fake_binary(
        dir.path(),
        "fake-tact-ui",
        "#!/bin/sh\necho nothing useful\n",
    );

    let output = tact_eval(&[
        "run",
        suite.to_str().unwrap(),
        "--runner",
        "cli",
        "--binary",
        binary.to_str().unwrap(),
    ]);

    assert!(output.status.success(), "{}", stderr(&output));
    assert!(stderr(&output).contains("fail 1"), "{}", stderr(&output));
}

#[cfg(unix)]
#[test]
fn a_case_id_that_would_escape_the_work_root_fails_validation() {
    // `validate` is the check CI runs on every pull request, so it is the
    // right place to refuse an id that would aim the run's cleanup outside the
    // scratch directory.
    let dir = tempfile::tempdir().unwrap();
    let suite = dir.path().join("suite");
    std::fs::create_dir_all(&suite).unwrap();
    std::fs::write(
        suite.join("escaping.toml"),
        "id = \"../../victim\"\nprompt = \"p\"\n",
    )
    .unwrap();

    let output = tact_eval(&["validate", suite.to_str().unwrap()]);

    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("single path component"),
        "{}",
        stderr(&output)
    );
}

#[cfg(unix)]
#[test]
fn a_filter_that_matches_nothing_fails_instead_of_running_nothing() {
    // A typo would otherwise look like a suite that passed everything at no
    // cost, which is the most expensive way to be wrong.
    let dir = tempfile::tempdir().unwrap();
    let suite = suite(dir.path());
    let binary = fake_binary(dir.path(), "fake-tact-ui", "#!/bin/sh\necho ready\n");

    let output = tact_eval(&[
        "run",
        suite.to_str().unwrap(),
        "--runner",
        "cli",
        "--binary",
        binary.to_str().unwrap(),
        "--filter",
        "no-such-case",
    ]);

    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("no case named"),
        "{}",
        stderr(&output)
    );
}
