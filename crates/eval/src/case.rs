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
        case.validate(path)?;
        Ok(case)
    }

    fn validate(&self, path: &Path) -> Result<()> {
        if self.prompt.trim().is_empty() {
            anyhow::bail!("{}: prompt is empty", path.display());
        }
        if self.timeout_secs == 0 {
            anyhow::bail!("{}: timeout_secs must be positive", path.display());
        }
        if let Isolation::Copy { fixture } = &self.isolation
            && !fixture.is_dir()
        {
            anyhow::bail!(
                "{}: fixture {} is not a directory",
                path.display(),
                fixture.display()
            );
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
            }
            Self::Answer { name, assertions } => {
                if name.trim().is_empty() {
                    anyhow::bail!("{}: grader name is empty", path.display());
                }
                if assertions.is_empty() {
                    anyhow::bail!("{}: grader {name} asserts nothing", path.display());
                }
            }
        }
        Ok(())
    }
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
    fn a_missing_fixture_is_rejected_at_load_time() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fixtured.toml");
        std::fs::write(
            &path,
            "prompt = \"x\"\n[isolation]\nkind = \"copy\"\nfixture = \"/nonexistent/fixture\"\n",
        )
        .unwrap();

        let error = EvalCase::load(&path).unwrap_err().to_string();

        assert!(error.contains("not a directory"), "{error}");
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
