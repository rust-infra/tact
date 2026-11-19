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

use crate::case::{
    EvalCase, FileAssertion, GraderSpec, JsonAssertion, TextAssertion, ToolAssertion,
};

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
            GraderSpec::Tools { assertions, .. } => check_tools(&artifact.tools, assertions),
            GraderSpec::Json {
                path: file,
                assertions,
                ..
            } => check_json(work_dir, file, assertions),
        };
        grades.push(Grade {
            grader: spec.name().to_string(),
            verdict,
        });
    }
    grades
}

/// `Ok((passed, output))`, or a verdict when the grader itself could not run.
///
/// The grader runs through the same helper as a case run, so an over-budget
/// `cargo test` is killed with whatever it spawned rather than only the shell
/// that launched it.
async fn run_command(
    work_dir: &Path,
    command: &str,
    timeout_secs: u64,
) -> Result<(bool, String), Verdict> {
    let mut child = tokio::process::Command::new("sh");
    child.arg("-c").arg(command).current_dir(work_dir);

    let outcome = match crate::process::run_child(child, Duration::from_secs(timeout_secs)).await {
        Ok(outcome) => outcome,
        Err(error) => {
            return Err(Verdict::Inconclusive {
                reason: format!("grader could not start: {error}"),
            });
        }
    };

    // A grader that hangs is a harness problem, not a decision about the agent.
    // Reporting it as `Fail` would turn a broken `cargo test` invocation into a
    // fake regression.
    if outcome.timed_out() {
        return Err(Verdict::Inconclusive {
            reason: format!("grader timed out after {timeout_secs}s"),
        });
    }
    let passed = outcome.succeeded();
    let mut combined = outcome.stdout;
    combined.push_str(&outcome.stderr);
    Ok((passed, combined))
}

fn check_files(work_dir: &Path, assertions: &[FileAssertion]) -> Verdict {
    for assertion in assertions {
        match check_one(work_dir, assertion) {
            Ok(None) => {}
            Ok(Some(reason)) => return Verdict::Fail { reason },
            // The file exists but this process cannot read it: non-UTF-8 bytes,
            // a permission that changed under us, a directory where a file was
            // expected. "Cannot read it" is not "the agent wrote the wrong
            // thing", and this module's whole premise is that the two must not
            // be conflated.
            Err(reason) => return Verdict::Inconclusive { reason },
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

/// Assert on the tool calls, by name only.
///
/// A record whose name is `None` is a line the CLI runner could not attribute
/// to a tool (a bare `[tool failed]`). It stays in the artifact as evidence but
/// matches no assertion by name — claiming a tool ran when the log did not say
/// which one would be the same guess the runners refuse to make elsewhere.
///
/// Unattributed records also make some assertions undecidable, and those return
/// [`Verdict::Inconclusive`] rather than a pass. "Never called `bash`" cannot be
/// proven while a failure line might have been `bash`; "the run started with
/// `read`" cannot be proven when no call could be named. A silent pass there
/// would let a CLI run satisfy a tool-selection assertion on a guess, which is
/// exactly the failure mode this grader exists to catch.
fn check_tools(tools: &[crate::ToolRecord], assertions: &[ToolAssertion]) -> Verdict {
    let named: Vec<&str> = tools
        .iter()
        .filter_map(|tool| tool.name.as_deref())
        .collect();
    let unattributed = tools.len() - named.len();
    let cannot_say = |what: &str| Verdict::Inconclusive {
        reason: format!(
            "{what}: {unattributed} tool record(s) named no tool, so this cannot be decided"
        ),
    };

    for assertion in assertions {
        let violation = match assertion {
            // Needing a call: finding it proves the assertion, missing it proves
            // nothing while an unattributed record could be it.
            ToolAssertion::Tool { tool } => {
                if named.contains(&tool.as_str()) {
                    None
                } else if unattributed > 0 {
                    return cannot_say(&format!("the run may have called {tool}"));
                } else {
                    Some(format!("the run never called {tool}"))
                }
            }
            // Needing a result: a named failure proves the call happened but
            // not that it produced anything, so only a success decides it.
            ToolAssertion::ToolSucceeded { tool } => {
                let succeeded = tools.iter().any(|record| {
                    record.name.as_deref() == Some(tool.as_str())
                        && record.status == crate::ToolStatus::Succeeded
                });
                if succeeded {
                    None
                } else if unattributed > 0 {
                    return cannot_say(&format!("the run may have called {tool}"));
                } else if named.contains(&tool.as_str()) {
                    Some(format!("every call to {tool} failed"))
                } else {
                    Some(format!("the run never called {tool}"))
                }
            }
            // Needing an absence: finding the call is a proven violation,
            // whether or not other records were unattributed.
            ToolAssertion::NoTool { tool } => {
                if named.contains(&tool.as_str()) {
                    Some(format!("the run should not call {tool}"))
                } else if unattributed > 0 {
                    return cannot_say(&format!("the run may have called {tool}"));
                } else {
                    None
                }
            }
            // Order matters, so only the first record can decide it. An
            // unattributed first record leaves it unknown.
            ToolAssertion::FirstTool { tool } => match tools.first() {
                Some(crate::ToolRecord {
                    name: Some(first), ..
                }) if first == tool => None,
                Some(crate::ToolRecord { name: None, .. }) => {
                    return cannot_say("the run's first tool call");
                }
                Some(crate::ToolRecord {
                    name: Some(first), ..
                }) => Some(format!("the run started with {first}, not {tool}")),
                None => Some("the run called no tool".to_string()),
            },
            // An upper bound: exceeding it is proven, staying under it is not
            // while unattributed calls could add more.
            ToolAssertion::AtMost { tool, max } => {
                let count = named.iter().filter(|name| **name == tool.as_str()).count();
                if count > *max as usize {
                    Some(format!(
                        "the run called {tool} {count} time(s), at most {max} allowed"
                    ))
                } else if unattributed > 0 {
                    return cannot_say(&format!("the run may have called {tool} more"));
                } else {
                    None
                }
            }
        };
        if let Some(reason) = violation {
            return Verdict::Fail { reason };
        }
    }
    Verdict::Pass
}

/// One step of a dotted JSON path: a key, or an array index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PathStep {
    Key(String),
    Index(usize),
}

/// Parse `a.b[0].c` into steps.
///
/// Kept deliberately small: keys separated by dots, indices in brackets. The
/// error text names the position, because a test file's path is checked once at
/// load time and a vague message there costs more than the parser does.
pub(crate) fn parse_json_path(path: &str) -> Result<Vec<PathStep>, String> {
    let mut steps = Vec::new();
    let mut key = String::new();
    let mut rest = path.chars().peekable();

    while let Some(character) = rest.next() {
        match character {
            '.' => {
                if key.is_empty() {
                    // A dot after `]` is just the separator back to keys;
                    // only a leading or doubled dot has no step to separate.
                    if matches!(steps.last(), Some(PathStep::Index(_))) {
                        continue;
                    }
                    return Err("empty key (a leading or doubled `.`)".into());
                }
                steps.push(PathStep::Key(std::mem::take(&mut key)));
            }
            '[' => {
                if !key.is_empty() {
                    steps.push(PathStep::Key(std::mem::take(&mut key)));
                }
                let mut digits = String::new();
                loop {
                    match rest.next() {
                        Some(']') => break,
                        Some(digit) => digits.push(digit),
                        None => return Err("unclosed `[`".into()),
                    }
                }
                let index = digits
                    .parse::<usize>()
                    .map_err(|_| format!("{digits:?} is not an array index"))?;
                steps.push(PathStep::Index(index));
            }
            ']' => return Err("unexpected `]`".into()),
            other => key.push(other),
        }
    }
    if !key.is_empty() {
        steps.push(PathStep::Key(key));
    } else if path.ends_with('.') {
        // `a.` and `a[0].` would otherwise parse as "the path so far", which
        // hides a typo behind a path that looks intentional.
        return Err("trailing `.`".into());
    }
    Ok(steps)
}

/// Walk `value` down `path`. `None` when any step is missing.
fn json_at<'a>(value: &'a serde_json::Value, path: &[PathStep]) -> Option<&'a serde_json::Value> {
    let mut current = value;
    for step in path {
        current = match step {
            PathStep::Key(key) => current.get(key)?,
            PathStep::Index(index) => current.get(*index)?,
        };
    }
    Some(current)
}

fn check_json(work_dir: &Path, file: &Path, assertions: &[JsonAssertion]) -> Verdict {
    let text = match std::fs::read_to_string(work_dir.join(file)) {
        Ok(text) => text,
        // Not writing the file is the agent's outcome to own.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Verdict::Fail {
                reason: format!("{} is missing", file.display()),
            };
        }
        // Writing something this process cannot read is not: a non-UTF-8 or
        // binary artifact is a legitimate output that this grader cannot judge.
        Err(error) => {
            return Verdict::Inconclusive {
                reason: format!("cannot read {}: {error}", file.display()),
            };
        }
    };
    let document: serde_json::Value = match serde_json::from_str(&text) {
        Ok(document) => document,
        Err(error) => {
            return Verdict::Fail {
                reason: format!("{} is not valid JSON: {error}", file.display()),
            };
        }
    };

    for assertion in assertions {
        let path = match assertion {
            JsonAssertion::Present { path }
            | JsonAssertion::Absent { path }
            | JsonAssertion::Equals { path, .. }
            | JsonAssertion::Contains { path, .. }
            | JsonAssertion::Len { path, .. } => path,
        };
        let steps = match parse_json_path(path) {
            Ok(steps) => steps,
            Err(error) => {
                // Load rejects this, so reaching it means a library caller
                // built the case. Undecidable, not a verdict about the agent.
                return Verdict::Inconclusive {
                    reason: format!("invalid JSON path {path:?}: {error}"),
                };
            }
        };
        let found = json_at(&document, &steps);

        // `Absent` is the one assertion that reads a missing value as a pass.
        // Everything else needs to find something first, and says so when it
        // does not, instead of reporting a shape mismatch on nothing.
        if let JsonAssertion::Absent { path } = assertion {
            if found.is_some() {
                return Verdict::Fail {
                    reason: format!("{} should not have {path:?}", file.display()),
                };
            }
            continue;
        }
        let Some(value) = found else {
            return Verdict::Fail {
                reason: format!("{} has no value at {path:?}", file.display()),
            };
        };
        let reason = match assertion {
            JsonAssertion::Present { .. } => None,
            // Handled above, before this match.
            JsonAssertion::Absent { .. } => None,
            JsonAssertion::Equals { path, value: want } => (value != want)
                .then(|| format!("{} at {path:?} is {value}, expected {want}", file.display())),
            JsonAssertion::Contains { path, text } => match value.as_str() {
                Some(actual) => (!actual.contains(text.as_str())).then(|| {
                    format!(
                        "{} at {path:?} is {actual:?}, not containing {text:?}",
                        file.display()
                    )
                }),
                None => Some(format!(
                    "{} at {path:?} is {value}, not a string",
                    file.display()
                )),
            },
            JsonAssertion::Len { path, len } => match value.as_array() {
                Some(items) => (items.len() != *len).then(|| {
                    format!(
                        "{} at {path:?} has {} items, expected {len}",
                        file.display(),
                        items.len()
                    )
                }),
                None => Some(format!(
                    "{} at {path:?} is {value}, not an array",
                    file.display()
                )),
            },
        };
        if let Some(reason) = reason {
            return Verdict::Fail { reason };
        }
    }
    Verdict::Pass
}

/// `Ok(None)` for a satisfied assertion, `Ok(Some(reason))` for a decided
/// failure, `Err(reason)` when the harness could not look.
fn check_one(work_dir: &Path, assertion: &FileAssertion) -> Result<Option<String>, String> {
    // A missing file is a decision: the case asked for a file and the run did
    // not leave one. A file that exists and cannot be read is not — that is
    // this process failing to look, so it travels as `Err` and becomes
    // inconclusive.
    let content = |path: &Path| match std::fs::read_to_string(work_dir.join(path)) {
        Ok(content) => Ok(Some(content)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("cannot read {}: {error}", path.display())),
    };
    // `Path::exists()` folds a metadata error into `false`, which would pass
    // `absent` and fail `exists` when the harness merely could not look.
    let exists = |path: &Path| match std::fs::symlink_metadata(work_dir.join(path)) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!("cannot stat {}: {error}", path.display())),
    };
    Ok(match assertion {
        FileAssertion::Exists { path } => {
            (!exists(path)?).then(|| format!("{} is missing", path.display()))
        }
        FileAssertion::Absent { path } => {
            exists(path)?.then(|| format!("{} should not exist", path.display()))
        }
        FileAssertion::Contains { path, text } => match content(path)? {
            None => Some(format!("{} is missing", path.display())),
            Some(read) => (!read.contains(text.as_str()))
                .then(|| format!("{} does not contain {text:?}", path.display())),
        },
        FileAssertion::Matches { path, pattern } => {
            let Some(read) = content(path)? else {
                return Ok(Some(format!("{} is missing", path.display())));
            };
            let regex = match regex::Regex::new(pattern) {
                Ok(regex) => regex,
                // A pattern that cannot compile is a broken case, and load
                // already rejects it. Reaching it means a library caller built
                // the case, so there is no decision to make about the agent.
                Err(error) => return Err(format!("bad pattern {pattern:?}: {error}")),
            };
            (!regex.is_match(&read))
                .then(|| format!("{} does not match /{pattern}/", path.display()))
        }
    })
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
            prompt: Some("p".into()),
            prompts: Vec::new(),
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
    async fn grading_does_not_restate_the_run_failure() {
        // The run's failure lives on `CaseOutcome::failure` and is applied by
        // `CaseOutcome::verdict`. A second copy here would be the same fact
        // stored twice, and the two could disagree.
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
        assert_eq!(grades.len(), 1, "only the declared grader is graded");
        assert!(grades[0].verdict.is_pass());
    }

    /// One recorded tool call, by name.
    fn tool(name: &str) -> crate::ToolRecord {
        crate::ToolRecord {
            name: Some(name.into()),
            args: None,
            status: crate::ToolStatus::Succeeded,
            duration_ms: Some(1),
            message: None,
        }
    }

    #[tokio::test]
    async fn a_tools_grader_reads_the_calls_the_run_made() {
        // The whole reason this grader exists: a file and answer grader both
        // pass whether the agent used `read` or `bash cat`.
        let dir = tempfile::tempdir().unwrap();
        let mut artifact = artifact();
        artifact.tools = vec![tool("read"), tool("edit")];

        let grades = grade(
            &case(vec![GraderSpec::Tools {
                name: "used-read".into(),
                assertions: vec![
                    ToolAssertion::Tool {
                        tool: "read".into(),
                    },
                    ToolAssertion::NoTool {
                        tool: "bash".into(),
                    },
                    ToolAssertion::FirstTool {
                        tool: "read".into(),
                    },
                    ToolAssertion::AtMost {
                        tool: "edit".into(),
                        max: 1,
                    },
                ],
            }]),
            dir.path(),
            &artifact,
        )
        .await;

        assert!(grades[0].verdict.is_pass(), "{:?}", grades[0]);
    }

    #[tokio::test]
    async fn a_tools_grader_names_what_the_run_did_instead() {
        let dir = tempfile::tempdir().unwrap();
        let mut artifact = artifact();
        artifact.tools = vec![tool("bash")];

        let grades = grade(
            &case(vec![GraderSpec::Tools {
                name: "used-read".into(),
                assertions: vec![ToolAssertion::Tool {
                    tool: "read".into(),
                }],
            }]),
            dir.path(),
            &artifact,
        )
        .await;

        match &grades[0].verdict {
            Verdict::Fail { reason } => assert!(reason.contains("never called read"), "{reason}"),
            other => panic!("expected failure, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_succeeded_assertion_rejects_a_failed_call() {
        // The call happened, so `tool` would pass. It produced nothing, so the
        // case that needs the result must not.
        let dir = tempfile::tempdir().unwrap();
        let mut artifact = artifact();
        artifact.tools = vec![crate::ToolRecord {
            name: Some("load_memory".into()),
            args: None,
            status: crate::ToolStatus::Failed,
            duration_ms: None,
            message: Some("no memories saved for this repository yet".into()),
        }];

        let grades = grade(
            &case(vec![GraderSpec::Tools {
                name: "loaded".into(),
                assertions: vec![ToolAssertion::ToolSucceeded {
                    tool: "load_memory".into(),
                }],
            }]),
            dir.path(),
            &artifact,
        )
        .await;

        match &grades[0].verdict {
            Verdict::Fail { reason } => {
                assert!(
                    reason.contains("every call to load_memory failed"),
                    "{reason}"
                );
            }
            other => panic!("expected failure, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_succeeded_assertion_accepts_a_retry_that_worked() {
        let dir = tempfile::tempdir().unwrap();
        let mut artifact = artifact();
        artifact.tools = vec![
            crate::ToolRecord {
                name: Some("load_memory".into()),
                args: None,
                status: crate::ToolStatus::Failed,
                duration_ms: None,
                message: Some("no memories saved yet".into()),
            },
            tool("load_memory"),
        ];

        let grades = grade(
            &case(vec![GraderSpec::Tools {
                name: "loaded".into(),
                assertions: vec![ToolAssertion::ToolSucceeded {
                    tool: "load_memory".into(),
                }],
            }]),
            dir.path(),
            &artifact,
        )
        .await;

        assert!(grades[0].verdict.is_pass(), "{:?}", grades[0]);
    }

    #[tokio::test]
    async fn a_succeeded_assertion_stays_undecided_while_a_call_is_unattributed() {
        // The named failure proves one call did not work. The unattributed one
        // might have been a successful retry, so this cannot fail the case.
        let dir = tempfile::tempdir().unwrap();
        let mut artifact = artifact();
        artifact.tools = vec![
            crate::ToolRecord {
                name: Some("load_memory".into()),
                args: None,
                status: crate::ToolStatus::Failed,
                duration_ms: None,
                message: Some("no memories saved yet".into()),
            },
            crate::ToolRecord {
                name: None,
                args: None,
                status: crate::ToolStatus::Failed,
                duration_ms: None,
                message: Some("bare failure".into()),
            },
        ];

        let grades = grade(
            &case(vec![GraderSpec::Tools {
                name: "loaded".into(),
                assertions: vec![ToolAssertion::ToolSucceeded {
                    tool: "load_memory".into(),
                }],
            }]),
            dir.path(),
            &artifact,
        )
        .await;

        assert!(
            matches!(&grades[0].verdict, Verdict::Inconclusive { .. }),
            "{:?}",
            grades[0]
        );
    }

    #[tokio::test]
    async fn an_unattributed_tool_record_satisfies_no_assertion() {
        // The CLI runner cannot name the tool behind a bare `[tool failed]`
        // line, and records `None` rather than guessing. A `None` must not be
        // read as "some tool ran", or a suite could pass on a guess.
        let dir = tempfile::tempdir().unwrap();
        let mut artifact = artifact();
        artifact.tools = vec![crate::ToolRecord {
            name: None,
            args: None,
            status: crate::ToolStatus::Failed,
            duration_ms: None,
            message: Some("provider activity failed".into()),
        }];

        let grades = grade(
            &case(vec![GraderSpec::Tools {
                name: "any-tool".into(),
                assertions: vec![ToolAssertion::FirstTool {
                    tool: "read".into(),
                }],
            }]),
            dir.path(),
            &artifact,
        )
        .await;

        assert!(
            !grades[0].verdict.is_pass(),
            "an unattributed record passed a named assertion: {:?}",
            grades[0]
        );
    }

    #[tokio::test]
    async fn a_json_grader_reads_a_document_by_path() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("out.json"),
            r#"{"status":"ok","items":[{"id":1},{"id":2}],"meta":{"count":2}}"#,
        )
        .unwrap();

        let grades = grade(
            &case(vec![GraderSpec::Json {
                name: "shape".into(),
                path: std::path::PathBuf::from("out.json"),
                assertions: vec![
                    JsonAssertion::Present {
                        path: "status".into(),
                    },
                    JsonAssertion::Equals {
                        path: "status".into(),
                        value: serde_json::json!("ok"),
                    },
                    JsonAssertion::Len {
                        path: "items".into(),
                        len: 2,
                    },
                    JsonAssertion::Equals {
                        path: "items[1].id".into(),
                        value: serde_json::json!(2),
                    },
                    JsonAssertion::Contains {
                        path: "status".into(),
                        text: "o".into(),
                    },
                    JsonAssertion::Absent {
                        path: "error".into(),
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
    async fn a_json_grader_names_the_path_and_the_expected_value() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("out.json"), r#"{"status":"broken"}"#).unwrap();

        let grades = grade(
            &case(vec![GraderSpec::Json {
                name: "shape".into(),
                path: std::path::PathBuf::from("out.json"),
                assertions: vec![JsonAssertion::Equals {
                    path: "status".into(),
                    value: serde_json::json!("ok"),
                }],
            }]),
            dir.path(),
            &artifact(),
        )
        .await;

        match &grades[0].verdict {
            Verdict::Fail { reason } => {
                assert!(reason.contains("\"status\""), "{reason}");
                assert!(reason.contains("expected"), "{reason}");
            }
            other => panic!("expected failure, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_json_grader_fails_a_missing_file_rather_than_passing_on_nothing() {
        let dir = tempfile::tempdir().unwrap();

        let grades = grade(
            &case(vec![GraderSpec::Json {
                name: "shape".into(),
                path: std::path::PathBuf::from("absent.json"),
                assertions: vec![JsonAssertion::Present {
                    path: "status".into(),
                }],
            }]),
            dir.path(),
            &artifact(),
        )
        .await;

        match &grades[0].verdict {
            // Not writing the file is the agent's outcome, so it decides the case.
            Verdict::Fail { reason } => assert!(reason.contains("is missing"), "{reason}"),
            other => panic!("expected failure, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_file_the_grader_cannot_read_is_inconclusive_not_a_failure() {
        // A binary or non-UTF-8 artifact is a legitimate thing for the agent to
        // write. "This process cannot read it" is not "the agent wrote the
        // wrong bytes", and counting it as a failure would make a correct run
        // look like a regression.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("out.bin"), [0xff, 0xfe, 0x00, 0x01]).unwrap();

        let grades = grade(
            &case(vec![GraderSpec::Files {
                name: "files".into(),
                assertions: vec![FileAssertion::Contains {
                    path: std::path::PathBuf::from("out.bin"),
                    text: "x".into(),
                }],
            }]),
            dir.path(),
            &artifact(),
        )
        .await;

        assert!(
            matches!(grades[0].verdict, Verdict::Inconclusive { .. }),
            "expected inconclusive, got {:?}",
            grades[0]
        );
    }

    #[tokio::test]
    async fn a_missing_file_still_fails_a_file_assertion() {
        // The complement: a file the case asked for and the run did not leave is
        // a decision, not an unreadable artifact.
        let dir = tempfile::tempdir().unwrap();

        let grades = grade(
            &case(vec![GraderSpec::Files {
                name: "files".into(),
                assertions: vec![FileAssertion::Contains {
                    path: std::path::PathBuf::from("absent.txt"),
                    text: "x".into(),
                }],
            }]),
            dir.path(),
            &artifact(),
        )
        .await;

        match &grades[0].verdict {
            Verdict::Fail { reason } => assert!(reason.contains("is missing"), "{reason}"),
            other => panic!("expected failure, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_path_the_harness_cannot_stat_is_inconclusive() {
        // A file below a regular file cannot be stat'd. That is this process
        // failing to look, not proof the path is absent.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("regular"), "x").unwrap();

        let grades = grade(
            &case(vec![GraderSpec::Files {
                name: "files".into(),
                assertions: vec![FileAssertion::Absent {
                    path: std::path::PathBuf::from("regular/child"),
                }],
            }]),
            dir.path(),
            &artifact(),
        )
        .await;

        assert!(
            matches!(grades[0].verdict, Verdict::Inconclusive { .. }),
            "expected inconclusive, got {:?}",
            grades[0]
        );
    }

    #[test]
    fn a_json_path_parses_keys_and_indices() {
        assert_eq!(
            parse_json_path("items[1].id").unwrap(),
            vec![
                PathStep::Key("items".into()),
                PathStep::Index(1),
                PathStep::Key("id".into()),
            ]
        );
        assert_eq!(
            parse_json_path("").unwrap(),
            Vec::new(),
            "the document root"
        );
        assert!(parse_json_path("a..b").is_err(), "a doubled dot is a typo");
        assert!(parse_json_path("a[").is_err(), "an unclosed bracket");
        assert!(parse_json_path("a[x]").is_err(), "a non-numeric index");
        assert!(parse_json_path("a.").is_err(), "a trailing dot");
        assert!(
            parse_json_path("a[0].").is_err(),
            "a trailing dot after an index"
        );
        assert_eq!(
            parse_json_path("a[0][1]").unwrap(),
            vec![
                PathStep::Key("a".into()),
                PathStep::Index(0),
                PathStep::Index(1),
            ],
            "nested indices"
        );
    }
}
