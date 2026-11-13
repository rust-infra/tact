//! Turning a finished attempt into evidence.
//!
//! Graders read two things and nothing else: the [`RunArtifact`] a runner
//! produced, and the work directory the run left behind. They never learn which
//! runner ran, and they never ask the model what it thinks it did.
//!
//! A grader that cannot reach a decision returns [`Verdict::Inconclusive`],
//! which the report counts separately from a failure. Conflating "we could not
//! tell" with "it did not work" turns a broken harness into a fake regression.

use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::case::{EvalCase, FileAssertion, GraderSpec, TextAssertion};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "verdict", rename_all = "snake_case")]
pub enum Verdict {
    Pass,
    Fail { reason: String },
    Inconclusive { reason: String },
}

impl Verdict {
    pub fn is_pass(&self) -> bool {
        matches!(self, Self::Pass)
    }
}

/// One grader's decision, kept per grader so a report can name which assertion
/// failed rather than only the case.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Grade {
    pub grader: String,
    pub verdict: Verdict,
}

/// Run every grader for one attempt.
pub async fn grade(case: &EvalCase, work_dir: &Path, artifact: &crate::RunArtifact) -> Vec<Grade> {
    if case.graders.is_empty() {
        return vec![Grade {
            grader: "case".into(),
            verdict: Verdict::Inconclusive {
                reason: "the case declares no graders".into(),
            },
        }];
    }

    let mut grades = Vec::with_capacity(case.graders.len());
    for spec in &case.graders {
        let verdict = match spec {
            GraderSpec::Command { name, command } => {
                match run_command(work_dir, command, case.timeout_secs).await {
                    Ok((true, _)) => Verdict::Pass,
                    Ok((false, output)) => Verdict::Fail {
                        reason: format!("`{name}` failed: {}", tail(&output)),
                    },
                    Err(verdict) => verdict,
                }
            }
            GraderSpec::Files { assertions, .. } => check_files(work_dir, assertions),
            GraderSpec::Answer { assertions, .. } => check_answer(&artifact.answer, assertions),
        };
        grades.push(Grade {
            grader: spec.name().to_string(),
            verdict,
        });
    }

    // A run that crashed cannot be graded on its output, even when the files
    // happen to look right: the harness failed, not the agent.
    if let Some(failure) = &artifact.failure {
        grades.push(Grade {
            grader: "run".into(),
            verdict: Verdict::Inconclusive {
                reason: failure.to_string(),
            },
        });
    }
    grades
}

/// `Ok((passed, output))`, or a verdict when the grader itself could not run.
async fn run_command(
    work_dir: &Path,
    command: &str,
    timeout_secs: u64,
) -> Result<(bool, String), Verdict> {
    let mut child = tokio::process::Command::new("sh");
    child
        .arg("-c")
        .arg(command)
        .current_dir(work_dir)
        .stdin(std::process::Stdio::null())
        // Piped here rather than by `Command::output`, because the timeout has
        // to cover a child that was already spawned.
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);

    // Building the child here, rather than awaiting `Command::output`, is what
    // makes the timeout kill the grader: `kill_on_drop` is set on `child`, so
    // dropping it on timeout terminates the process instead of leaking it.
    let child = match child.spawn() {
        Ok(child) => child,
        Err(error) => {
            return Err(Verdict::Inconclusive {
                reason: format!("grader could not start: {error}"),
            });
        }
    };

    let output =
        match tokio::time::timeout(Duration::from_secs(timeout_secs), child.wait_with_output())
            .await
        {
            Ok(Ok(output)) => output,
            Ok(Err(error)) => {
                return Err(Verdict::Inconclusive {
                    reason: format!("grader failed: {error}"),
                });
            }
            Err(_) => return Ok((false, format!("timed out after {timeout_secs}s"))),
        };

    let mut combined = String::from_utf8_lossy(&output.stdout).into_owned();
    combined.push_str(&String::from_utf8_lossy(&output.stderr));
    Ok((output.status.success(), combined))
}

fn check_files(work_dir: &Path, assertions: &[FileAssertion]) -> Verdict {
    for assertion in assertions {
        if let Some(reason) = check_one(work_dir, assertion) {
            return Verdict::Fail { reason };
        }
    }
    Verdict::Pass
}

fn check_answer(answer: &str, assertions: &[TextAssertion]) -> Verdict {
    for assertion in assertions {
        let reason = match assertion {
            TextAssertion::Contains { text } => (!answer.contains(text.as_str()))
                .then(|| format!("the answer does not contain {text:?}")),
            TextAssertion::NotContains { text } => answer
                .contains(text.as_str())
                .then(|| format!("the answer should not contain {text:?}")),
            TextAssertion::Matches { pattern } => match regex::Regex::new(pattern) {
                Ok(regex) => (!regex.is_match(answer))
                    .then(|| format!("the answer does not match /{pattern}/")),
                Err(error) => Some(format!("bad pattern {pattern:?}: {error}")),
            },
            TextAssertion::NonEmpty => answer
                .trim()
                .is_empty()
                .then(|| "the answer is empty".to_string()),
        };
        if let Some(reason) = reason {
            return Verdict::Fail { reason };
        }
    }
    Verdict::Pass
}

fn check_one(work_dir: &Path, assertion: &FileAssertion) -> Option<String> {
    match assertion {
        FileAssertion::Exists { path } => {
            (!work_dir.join(path).exists()).then(|| format!("{} is missing", path.display()))
        }
        FileAssertion::Absent { path } => work_dir
            .join(path)
            .exists()
            .then(|| format!("{} should not exist", path.display())),
        FileAssertion::Contains { path, text } => {
            let read = match std::fs::read_to_string(work_dir.join(path)) {
                Ok(content) => content,
                Err(error) => return Some(format!("cannot read {}: {error}", path.display())),
            };
            (!read.contains(text.as_str()))
                .then(|| format!("{} does not contain {text:?}", path.display()))
        }
        FileAssertion::Matches { path, pattern } => {
            let read = match std::fs::read_to_string(work_dir.join(path)) {
                Ok(content) => content,
                Err(error) => return Some(format!("cannot read {}: {error}", path.display())),
            };
            let regex = match regex::Regex::new(pattern) {
                Ok(regex) => regex,
                Err(error) => return Some(format!("bad pattern {pattern:?}: {error}")),
            };
            (!regex.is_match(&read))
                .then(|| format!("{} does not match /{pattern}/", path.display()))
        }
    }
}

fn tail(text: &str) -> String {
    let trimmed = text.trim();
    let lines: Vec<&str> = trimmed.lines().take(8).collect();
    lines.join(" | ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn artifact() -> crate::RunArtifact {
        crate::RunArtifact {
            case_id: "c".into(),
            attempt: 1,
            session_id: None,
            answer: "done".into(),
            tools: Vec::new(),
            tokens: Default::default(),
            duration_ms: 1,
            stop: None,
            failure: None,
            turns: None,
        }
    }

    fn case(graders: Vec<GraderSpec>) -> EvalCase {
        EvalCase {
            id: "c".into(),
            prompt: "p".into(),
            timeout_secs: 10,
            isolation: crate::Isolation::Empty,
            graders,
        }
    }

    #[tokio::test]
    async fn an_empty_grader_list_is_inconclusive_not_a_pass() {
        let dir = tempfile::tempdir().unwrap();
        let grades = grade(&case(Vec::new()), dir.path(), &artifact()).await;
        assert!(!grades[0].verdict.is_pass());
        assert!(matches!(grades[0].verdict, Verdict::Inconclusive { .. }));
    }

    #[tokio::test]
    async fn a_command_grader_passes_on_exit_zero() {
        let dir = tempfile::tempdir().unwrap();
        let grades = grade(
            &case(vec![GraderSpec::Command {
                name: "true".into(),
                command: "true".into(),
            }]),
            dir.path(),
            &artifact(),
        )
        .await;
        assert!(grades[0].verdict.is_pass());
    }

    #[tokio::test]
    async fn a_failing_command_reports_its_output() {
        let dir = tempfile::tempdir().unwrap();
        let grades = grade(
            &case(vec![GraderSpec::Command {
                name: "noisy".into(),
                command: "echo bad thing >&2; exit 3".into(),
            }]),
            dir.path(),
            &artifact(),
        )
        .await;
        match &grades[0].verdict {
            Verdict::Fail { reason } => assert!(reason.contains("bad thing"), "{reason}"),
            other => panic!("expected failure, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn file_assertions_read_the_work_dir_not_the_repo() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("out.txt"), "hello world").unwrap();
        let grades = grade(
            &case(vec![GraderSpec::Files {
                name: "files".into(),
                assertions: vec![
                    FileAssertion::Exists {
                        path: PathBuf::from("out.txt"),
                    },
                    FileAssertion::Contains {
                        path: PathBuf::from("out.txt"),
                        text: "world".into(),
                    },
                    FileAssertion::Absent {
                        path: PathBuf::from("nope.txt"),
                    },
                ],
            }]),
            dir.path(),
            &artifact(),
        )
        .await;
        assert!(grades[0].verdict.is_pass(), "{:?}", grades[0]);
    }

    #[tokio::test]
    async fn an_answer_grader_reads_the_final_text() {
        let dir = tempfile::tempdir().unwrap();
        let mut artifact = artifact();
        artifact.answer = "the build is ready".into();
        let grades = grade(
            &case(vec![GraderSpec::Answer {
                name: "speaks".into(),
                assertions: vec![
                    TextAssertion::Contains {
                        text: "ready".into(),
                    },
                    TextAssertion::NotContains {
                        text: "error".into(),
                    },
                    TextAssertion::Matches {
                        pattern: "^the build".into(),
                    },
                    TextAssertion::NonEmpty,
                ],
            }]),
            dir.path(),
            &artifact,
        )
        .await;
        assert!(grades[0].verdict.is_pass(), "{:?}", grades[0]);
    }

    #[tokio::test]
    async fn an_answer_grader_names_the_assertion_that_failed() {
        let dir = tempfile::tempdir().unwrap();
        let mut artifact = artifact();
        artifact.answer = "all good".into();
        let grades = grade(
            &case(vec![GraderSpec::Answer {
                name: "speaks".into(),
                assertions: vec![TextAssertion::Contains {
                    text: "compiled".into(),
                }],
            }]),
            dir.path(),
            &artifact,
        )
        .await;
        match &grades[0].verdict {
            Verdict::Fail { reason } => assert!(reason.contains("compiled"), "{reason}"),
            other => panic!("expected failure, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_crashed_run_is_inconclusive_even_when_files_pass() {
        let dir = tempfile::tempdir().unwrap();
        let mut failed = artifact();
        failed.failure = Some(crate::RunFailure::Timeout { limit_secs: 5 });
        let grades = grade(
            &case(vec![GraderSpec::Command {
                name: "true".into(),
                command: "true".into(),
            }]),
            dir.path(),
            &failed,
        )
        .await;
        assert!(
            grades
                .iter()
                .any(|grade| matches!(grade.verdict, Verdict::Inconclusive { .. }))
        );
    }
}
