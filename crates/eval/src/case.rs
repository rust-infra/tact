//! One evaluation case, loaded from `cases/<name>.toml`.
//!
//! A case says what to ask and what counts as an answer. It deliberately does
//! not say which model or permission mode to use: those are process-global in
//! Tact (`config::install` runs once), so a suite declares them once for the
//! whole run. A per-case model would be a knob the in-process runner could not
//! honour, and a knob that lies is worse than no knob.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// How the case's working directory is prepared before the run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Isolation {
    /// A copy of `fixture`, made fresh for every attempt.
    ///
    /// A copy rather than a git worktree because grading compares files, and a
    /// worktree would carry the repository's own `.tact/` state and index into
    /// the comparison.
    Copy { fixture: PathBuf },
    /// An empty directory. The prompt must be answerable from nothing.
    Empty,
}

/// A misspelled key must fail the load, not be dropped.
///
/// `timeout_sec` instead of `timeout_secs` would otherwise silently fall back
/// to the default, and a case that meant to bound a slow run would grade
/// against the wrong budget without a word.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvalCase {
    /// Defaults to the file stem, so a suite never carries two cases under one
    /// name by accident.
    #[serde(default)]
    pub id: String,
    pub prompt: String,
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
    #[serde(default = "default_isolation")]
    pub isolation: Isolation,
    /// What decides the case. An empty list means the run itself is the
    /// evidence: the case grades [`crate::Verdict::Inconclusive`] rather than a
    /// hollow pass.
    #[serde(default)]
    pub graders: Vec<GraderSpec>,
}

fn default_timeout_secs() -> u64 {
    300
}

fn default_isolation() -> Isolation {
    Isolation::Empty
}

impl EvalCase {
    /// Load one case file. `id` defaults to the file stem so a suite never
    /// carries two cases under one name by accident.
    ///
    /// A fixture path is not checked here: a relative one only means something
    /// next to the suite directory, which this function does not know.
    /// `EvalSuite::load` resolves it and then validates.
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("cannot read case {}", path.display()))?;
        let mut case: Self = toml::from_str(&text)
            .with_context(|| format!("cannot parse case {}", path.display()))?;
        if case.id.trim().is_empty() {
            case.id = path
                .file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
                .context("case file has no stem")?;
        }
        case.validate_shape(path)?;
        Ok(case)
    }

    /// Everything that can be decided from the file alone.
    pub(crate) fn validate_shape(&self, path: &Path) -> Result<()> {
        if self.prompt.trim().is_empty() {
            anyhow::bail!("{}: prompt is empty", path.display());
        }
        if self.timeout_secs == 0 {
            anyhow::bail!("{}: timeout_secs must be positive", path.display());
        }
        for grader in &self.graders {
            grader.validate(path)?;
        }
        Ok(())
    }
}

/// One thing that decides a case.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum GraderSpec {
    /// Run a shell command in the case workdir. Exit status 0 passes.
    Command {
        /// Short name for the report; the command itself can be long.
        name: String,
        command: String,
    },
    /// Assert on files in the case workdir.
    Files {
        name: String,
        assertions: Vec<FileAssertion>,
    },
    /// Assert on the final assistant text.
    ///
    /// Substring and regex only. An LLM judge would be a fourth grader with its
    /// own calibration burden; it belongs here once there is a suite to
    /// calibrate it against, not before.
    Answer {
        name: String,
        assertions: Vec<TextAssertion>,
    },
}

impl GraderSpec {
    pub fn name(&self) -> &str {
        match self {
            Self::Command { name, .. } | Self::Files { name, .. } | Self::Answer { name, .. } => {
                name
            }
        }
    }

    fn validate(&self, path: &Path) -> Result<()> {
        match self {
            Self::Command { name, command } => {
                if name.trim().is_empty() {
                    anyhow::bail!("{}: grader name is empty", path.display());
                }
                if command.trim().is_empty() {
                    anyhow::bail!("{}: grader {name} has an empty command", path.display());
                }
            }
            Self::Files { name, assertions } => {
                if name.trim().is_empty() {
                    anyhow::bail!("{}: grader name is empty", path.display());
                }
                if assertions.is_empty() {
                    anyhow::bail!("{}: grader {name} asserts nothing", path.display());
                }
                for assertion in assertions {
                    if let FileAssertion::Matches {
                        pattern,
                        path: file,
                    } = assertion
                    {
                        check_pattern(path, name, &file.display().to_string(), pattern)?;
                    }
                }
            }
            Self::Answer { name, assertions } => {
                if name.trim().is_empty() {
                    anyhow::bail!("{}: grader name is empty", path.display());
                }
                if assertions.is_empty() {
                    anyhow::bail!("{}: grader {name} asserts nothing", path.display());
                }
                for assertion in assertions {
                    if let TextAssertion::Matches { pattern } = assertion {
                        check_pattern(path, name, "the answer", pattern)?;
                    }
                }
            }
        }
        Ok(())
    }
}

/// A pattern that does not compile is a broken case, not a failed run.
///
/// Rejecting it at load time keeps it from surfacing as `Verdict::Fail`, where
/// it would read as "the agent did not do this" when the truth is "the suite
/// asked an impossible question".
fn check_pattern(case: &Path, grader: &str, subject: &str, pattern: &str) -> Result<()> {
    regex::Regex::new(pattern).map_err(|error| {
        anyhow::anyhow!(
            "{}: grader {grader} has an invalid pattern for {subject}: {error}",
            case.display()
        )
    })?;
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum TextAssertion {
    Contains {
        text: String,
    },
    NotContains {
        text: String,
    },
    Matches {
        pattern: String,
    },
    /// The answer is not empty after trimming.
    NonEmpty,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum FileAssertion {
    Exists { path: PathBuf },
    Absent { path: PathBuf },
    Contains { path: PathBuf, text: String },
    Matches { path: PathBuf, pattern: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_case_without_an_id_takes_its_file_stem() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rename-helper.toml");
        std::fs::write(&path, "prompt = \"do it\"\n").unwrap();

        let case = EvalCase::load(&path).unwrap();

        assert_eq!(case.id, "rename-helper");
        assert_eq!(case.timeout_secs, 300);
        assert_eq!(case.isolation, Isolation::Empty);
    }

    #[test]
    fn an_empty_prompt_is_rejected_at_load_time() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("blank.toml");
        std::fs::write(&path, "prompt = \"   \"\n").unwrap();

        let error = EvalCase::load(&path).unwrap_err().to_string();

        assert!(error.contains("prompt is empty"), "{error}");
    }

    #[test]
    fn a_case_file_does_not_resolve_its_own_fixture() {
        // A relative fixture only means something next to the suite directory,
        // which a single case file does not know. `EvalSuite::load` resolves and
        // checks it, and its own tests cover that.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fixtured.toml");
        std::fs::write(
            &path,
            "prompt = \"x\"\n[isolation]\nkind = \"copy\"\nfixture = \"fixtures/nope\"\n",
        )
        .unwrap();

        let case = EvalCase::load(&path).expect("shape is valid");

        assert_eq!(
            case.isolation,
            Isolation::Copy {
                fixture: PathBuf::from("fixtures/nope")
            },
            "the path stays as written until the suite resolves it"
        );
    }

    #[test]
    fn an_invalid_regex_is_rejected_at_load_time_not_as_a_failed_run() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad-regex.toml");
        std::fs::write(
            &path,
            r#"
prompt = "x"

[[graders]]
kind = "answer"
name = "shape"

[[graders.assertions]]
kind = "matches"
pattern = "(unclosed"
"#,
        )
        .unwrap();

        let error = EvalCase::load(&path).unwrap_err().to_string();

        assert!(error.contains("invalid pattern"), "{error}");
    }

    #[test]
    fn a_grader_with_no_assertions_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hollow.toml");
        std::fs::write(
            &path,
            "prompt = \"x\"\n[[graders]]\nkind = \"files\"\nname = \"files\"\nassertions = []\n",
        )
        .unwrap();

        let error = EvalCase::load(&path).unwrap_err().to_string();

        assert!(error.contains("asserts nothing"), "{error}");
    }
}
