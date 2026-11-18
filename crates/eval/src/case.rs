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
    /// A single-prompt case, spelled `prompt = "..."`.
    ///
    /// Kept as its own field rather than folded into [`Self::prompts`] because
    /// it is the spelling every existing suite uses, and because `prompt` and
    /// `prompts` together would be a second way to say the same thing. Exactly
    /// one of the two must be present; [`Self::validate_shape`] refuses both or
    /// neither.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    /// A multi-turn case: every prompt is sent to the same session, in order.
    ///
    /// This is what a conversation regression needs. "Change the file you just
    /// wrote", "now summarise what you did", and a compaction case that must
    /// survive more than one turn are all unexpressible with one prompt, and
    /// they are where the agent loop actually breaks.
    ///
    /// The graders still judge the final state: the answer is the last turn's
    /// text, and the files are whatever the last turn left. A case that needs
    /// to judge an intermediate turn belongs in its own case, because a
    /// transcript grader is a different (and much less stable) thing.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub prompts: Vec<String>,
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

/// The largest wall-clock budget a case may declare, in seconds.
///
/// A day. Nothing a suite does takes longer, and the ceiling is load-bearing:
/// both runners compute a deadline as `Instant::now() + Duration`, which panics
/// when the value overflows that platform's `Instant`. `timeout_secs =
/// 9223372036854775807` parses and used to validate, then took the process down
/// before the first turn — a case file crashing the harness instead of failing
/// the case.
pub const MAX_TIMEOUT_SECS: u64 = 86_400;

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

    /// The turns this case sends, in order.
    ///
    /// One accessor rather than two fields at every call site, so a runner
    /// cannot forget that a case may have more than one prompt.
    pub fn turns(&self) -> Vec<&str> {
        match (&self.prompt, self.prompts.is_empty()) {
            (Some(prompt), true) => vec![prompt.as_str()],
            (None, false) => self.prompts.iter().map(String::as_str).collect(),
            // Unreachable for a loaded case; `validate_shape` refuses this. A
            // constructed one gets no turns rather than a panic, and the run
            // then fails as an empty turn instead of taking the process down.
            (Some(prompt), false) => vec![prompt.as_str()],
            (None, true) => Vec::new(),
        }
    }

    /// Everything that can be decided from the file alone.
    pub(crate) fn validate_shape(&self, path: &Path) -> Result<()> {
        self.validate_turns(path)?;
        self.validate_id(path)?;
        if self.timeout_secs == 0 {
            anyhow::bail!("{}: timeout_secs must be positive", path.display());
        }
        if self.timeout_secs > MAX_TIMEOUT_SECS {
            anyhow::bail!(
                "{}: timeout_secs {} exceeds the {MAX_TIMEOUT_SECS}s limit",
                path.display(),
                self.timeout_secs
            );
        }
        for grader in &self.graders {
            grader.validate(path)?;
        }
        Ok(())
    }

    /// Exactly one of `prompt` and `prompts` must carry the case's turns.
    ///
    /// Both is ambiguous — which one wins? — and neither is an empty case that
    /// would run nothing and grade an empty answer. `prompts` must also be
    /// non-empty and carry no blank turn: a blank turn is a prompt the model is
    /// asked to answer with nothing, which no suite means.
    fn validate_turns(&self, path: &Path) -> Result<()> {
        match (&self.prompt, self.prompts.is_empty()) {
            (Some(prompt), true) => {
                if prompt.trim().is_empty() {
                    anyhow::bail!("{}: prompt is empty", path.display());
                }
            }
            (None, false) => {
                for (index, prompt) in self.prompts.iter().enumerate() {
                    if prompt.trim().is_empty() {
                        anyhow::bail!("{}: prompts[{}] is empty", path.display(), index);
                    }
                }
            }
            (Some(_), false) => anyhow::bail!(
                "{}: a case declares `prompt` or `prompts`, not both",
                path.display()
            ),
            (None, true) => anyhow::bail!("{}: a case needs `prompt` or `prompts`", path.display()),
        }
        Ok(())
    }

    /// `id` becomes a directory name under the work root, so it is a path.
    ///
    /// `prepare_work_dir` joins it onto `$TMPDIR/tact-eval`, and the run then
    /// deletes that directory. An `id` of `"../../victim"` or an absolute path
    /// would therefore point the run's `remove_dir_all` outside the scratch
    /// root, at a directory the case does not own.
    ///
    /// `prepare_work_dir` also refuses to let the id escape, so this check is
    /// the message rather than the guarantee: it names the case file and the
    /// offending id instead of silently renaming the directory. The guarantee
    /// has to live where the path is built, because an `EvalCase` can also be
    /// constructed by a library caller instead of loaded.
    fn validate_id(&self, path: &Path) -> Result<()> {
        let id = self.id.trim();
        if id.is_empty() {
            return Ok(());
        }
        let offending = Path::new(id)
            .components()
            .find(|component| !matches!(component, std::path::Component::Normal(_)))
            .is_some()
            || id.contains('/')
            || id.contains('\\');
        if offending {
            anyhow::bail!(
                "{}: id {id:?} must be a single path component (no separators, no `..`, not absolute), \
                 because it names the case's work directory",
                path.display()
            );
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
    /// Assert on a JSON file the run wrote.
    ///
    /// A structured output is what an agent case most often produces, and the
    /// only way to check one was `matches` with a regex over the raw text —
    /// brittle under key order and whitespace, and unreadable when it fails.
    Json {
        name: String,
        /// The JSON file to read, relative to the case workdir.
        path: PathBuf,
        assertions: Vec<JsonAssertion>,
    },
    /// Assert on the tool calls the run made, in order.
    ///
    /// The other three graders cannot see this. "Use `read` rather than `bash
    /// cat`", "call `grep` at most once", "never write before reading" are all
    /// tool-selection regressions a file or answer grader reports as a pass.
    ///
    /// It reads names only, never arguments: a finished-step log line carries
    /// the step message, not the argument summary, so the CLI runner cannot see
    /// arguments and a grader that required them would be host-only by
    /// accident.
    Tools {
        name: String,
        assertions: Vec<ToolAssertion>,
    },
}

impl GraderSpec {
    pub fn name(&self) -> &str {
        match self {
            Self::Command { name, .. }
            | Self::Files { name, .. }
            | Self::Answer { name, .. }
            | Self::Json { name, .. }
            | Self::Tools { name, .. } => name,
        }
    }

    fn validate(&self, case_path: &Path) -> Result<()> {
        match self {
            Self::Command { name, command } => {
                if name.trim().is_empty() {
                    anyhow::bail!("{}: grader name is empty", case_path.display());
                }
                if command.trim().is_empty() {
                    anyhow::bail!(
                        "{}: grader {name} has an empty command",
                        case_path.display()
                    );
                }
            }
            Self::Files { name, assertions } => {
                if name.trim().is_empty() {
                    anyhow::bail!("{}: grader name is empty", case_path.display());
                }
                if assertions.is_empty() {
                    anyhow::bail!("{}: grader {name} asserts nothing", case_path.display());
                }
                for assertion in assertions {
                    if let FileAssertion::Matches {
                        pattern,
                        path: file,
                    } = assertion
                    {
                        check_pattern(case_path, name, &file.display().to_string(), pattern)?;
                    }
                }
            }
            Self::Answer { name, assertions } => {
                if name.trim().is_empty() {
                    anyhow::bail!("{}: grader name is empty", case_path.display());
                }
                if assertions.is_empty() {
                    anyhow::bail!("{}: grader {name} asserts nothing", case_path.display());
                }
                for assertion in assertions {
                    if let TextAssertion::Matches { pattern } = assertion {
                        check_pattern(case_path, name, "the answer", pattern)?;
                    }
                }
            }
            Self::Tools { name, assertions } => {
                if name.trim().is_empty() {
                    anyhow::bail!("{}: grader name is empty", case_path.display());
                }
                if assertions.is_empty() {
                    anyhow::bail!("{}: grader {name} asserts nothing", case_path.display());
                }
                for assertion in assertions {
                    if let ToolAssertion::Tool { tool }
                    | ToolAssertion::NoTool { tool }
                    | ToolAssertion::FirstTool { tool }
                    | ToolAssertion::AtMost { tool, .. } = assertion
                        && tool.trim().is_empty()
                    {
                        anyhow::bail!("{}: grader {name} names an empty tool", case_path.display());
                    }
                }
            }
            Self::Json {
                name,
                path: file,
                assertions,
            } => {
                if name.trim().is_empty() {
                    anyhow::bail!("{}: grader name is empty", case_path.display());
                }
                if file.as_os_str().is_empty() {
                    anyhow::bail!("{}: grader {name} names no file", case_path.display());
                }
                if assertions.is_empty() {
                    anyhow::bail!("{}: grader {name} asserts nothing", case_path.display());
                }
                // A malformed path would otherwise fail every attempt at run
                // time, where it reads as "the agent wrote the wrong shape".
                for assertion in assertions {
                    let json_path = match assertion {
                        JsonAssertion::Present { path }
                        | JsonAssertion::Absent { path }
                        | JsonAssertion::Equals { path, .. }
                        | JsonAssertion::Contains { path, .. }
                        | JsonAssertion::Len { path, .. } => path,
                    };
                    crate::grader::parse_json_path(json_path).map_err(|error| {
                        anyhow::anyhow!(
                            "{}: grader {name} has an invalid JSON path {json_path:?}: {error}",
                            case_path.display()
                        )
                    })?;
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

/// One assertion about a JSON file, addressed by a dotted path.
///
/// The path is `a.b[0].c`-style: keys separated by dots, array indices in
/// brackets. It is a subset on purpose — no wildcards, no filters — because a
/// query language in a test file is another thing to learn and to get wrong.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum JsonAssertion {
    /// The path exists. `path` is empty for the document root.
    Present { path: String },
    /// The path does not exist.
    Absent { path: String },
    /// The value at `path` equals this JSON literal.
    Equals {
        path: String,
        value: serde_json::Value,
    },
    /// The value at `path` is a string containing `text`.
    Contains { path: String, text: String },
    /// The value at `path` is an array with this length.
    Len { path: String, len: usize },
}

/// One assertion about the run's tool calls.
///
/// Every variant names a tool, because a runner that cannot attribute a line to
/// a tool records `None` (a bare `[tool failed]` in the headless log). An
/// unattributed record therefore matches no assertion and can only ever fail
/// one, which is the honest reading: nothing shows which tool ran.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ToolAssertion {
    /// The tool was called at least once.
    Tool { tool: String },
    /// The tool was never called.
    NoTool { tool: String },
    /// The first tool call was this one.
    ///
    /// For "read before write": `kind = "first_tool", tool = "read"` says the
    /// run started by reading rather than by overwriting.
    FirstTool { tool: String },
    /// The tool was called at most `max` times. A budget on wasted turns.
    AtMost { tool: String, max: u32 },
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
    fn an_id_that_would_escape_the_work_root_is_rejected_at_load_time() {
        // The id names a directory the run later deletes. `../victim` would aim
        // that deletion at a directory the case does not own, so it must not
        // reach a runner at all.
        for id in ["../victim", "/tmp/absolute", "a/b", "..", "."] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("escaping.toml");
            std::fs::write(&path, format!("id = {id:?}\nprompt = \"x\"\n")).unwrap();

            let error = EvalCase::load(&path).unwrap_err().to_string();

            assert!(
                error.contains("single path component"),
                "id {id:?} was accepted: {error}"
            );
        }
    }

    #[test]
    fn a_plain_id_is_still_accepted() {
        // The rule must not reject the ids a suite actually uses.
        for id in ["write-a-file", "case_1", "case.v2"] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("plain.toml");
            std::fs::write(&path, format!("id = {id:?}\nprompt = \"x\"\n")).unwrap();

            assert_eq!(EvalCase::load(&path).unwrap().id, id);
        }
    }

    #[test]
    fn a_single_prompt_case_still_loads_and_reports_one_turn() {
        // The spelling every existing suite uses must keep working: `prompt` is
        // now optional so `prompts` can exist, not so suites have to change.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("single.toml");
        std::fs::write(&path, "prompt = \"write it\"\n").unwrap();

        let case = EvalCase::load(&path).unwrap();

        assert_eq!(case.turns(), ["write it"]);
    }

    #[test]
    fn a_multi_turn_case_reports_its_prompts_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("multi.toml");
        std::fs::write(&path, "prompts = [\"write it\", \"now read it back\"]\n").unwrap();

        let case = EvalCase::load(&path).unwrap();

        assert_eq!(case.turns(), ["write it", "now read it back"]);
        assert_eq!(case.prompt, None);
    }

    #[test]
    fn a_case_declaring_both_prompt_and_prompts_is_refused() {
        // Two spellings of the same thing is ambiguous: which one runs? Refusing
        // at load beats a runner silently picking one.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("both.toml");
        std::fs::write(&path, "prompt = \"a\"\nprompts = [\"b\"]\n").unwrap();

        let error = EvalCase::load(&path).unwrap_err().to_string();

        assert!(error.contains("not both"), "{error}");
    }

    #[test]
    fn a_case_with_neither_prompt_form_is_refused() {
        // Without this, the case runs nothing and grades an empty answer, which
        // looks like a model failure rather than a broken case.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("none.toml");
        std::fs::write(&path, "timeout_secs = 5\n").unwrap();

        let error = EvalCase::load(&path).unwrap_err().to_string();

        assert!(error.contains("needs `prompt` or `prompts`"), "{error}");
    }

    #[test]
    fn a_blank_turn_inside_prompts_is_refused_and_named() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("blank-turn.toml");
        std::fs::write(&path, "prompts = [\"ok\", \"  \"]\n").unwrap();

        let error = EvalCase::load(&path).unwrap_err().to_string();

        assert!(error.contains("prompts[1]"), "{error}");
    }

    #[test]
    fn a_timeout_that_would_overflow_the_deadline_is_refused_at_load_time() {
        // `timeout_secs = i64::MAX` parses, and both runners compute
        // `Instant::now() + Duration::from_secs(..)`. That addition panics on
        // overflow, so a case file used to take the whole suite down before its
        // first turn instead of failing the case.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("huge.toml");
        std::fs::write(
            &path,
            format!("prompt = \"p\"\ntimeout_secs = {}\n", i64::MAX),
        )
        .unwrap();

        let error = EvalCase::load(&path).unwrap_err().to_string();

        assert!(error.contains("exceeds"), "{error}");
        assert!(error.contains(&MAX_TIMEOUT_SECS.to_string()), "{error}");
    }

    #[test]
    fn the_largest_allowed_timeout_still_loads() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("max.toml");
        std::fs::write(
            &path,
            format!("prompt = \"p\"\ntimeout_secs = {MAX_TIMEOUT_SECS}\n"),
        )
        .unwrap();

        assert_eq!(
            EvalCase::load(&path).unwrap().timeout_secs,
            MAX_TIMEOUT_SECS
        );
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
