//! Running a case through `tact-ui headless`, as a separate process.
//!
//! This is the black-box path. It works against a release binary, a container,
//! or a machine that only has `tact-ui` installed, and it is the same shape the
//! scheduled-review workflow already uses.
//!
//! What it can see: the final answer on stdout, one line per event on stderr,
//! and the exit status. What it cannot: the typed stop reason and turn count,
//! which never cross the process boundary. Those are reported as unknown rather
//! than guessed, so a suite mixing both runners cannot average a guess into its
//! numbers.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use tokio::process::Command;

use crate::case::EvalCase;
use crate::options::RunOptions;
use crate::runner::{
    RunArtifact, RunFailure, RunOutcome, Runner, StopView, TokenTotals, ToolRecord, ToolStatus,
};

/// Where the CLI binary comes from when the caller does not name one.
///
/// `TACT_EVAL_BINARY` exists so a release binary or a container image can be
/// pointed at without a flag, which is how CI runs this.
pub fn default_cli_binary() -> PathBuf {
    std::env::var_os("TACT_EVAL_BINARY")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("tact-ui"))
}

/// Runs cases by spawning `tact-ui headless`.
#[derive(Debug, Clone)]
pub struct CliRunner {
    binary: PathBuf,
    options: RunOptions,
}

impl CliRunner {
    /// The binary is resolved to an absolute path here, because every attempt
    /// spawns it from the case's own working directory: a relative
    /// `./target/debug/tact-ui` would resolve there instead of here, and fail
    /// with a bare "No such file or directory".
    pub fn new(binary: impl Into<PathBuf>) -> Self {
        let binary = binary.into();
        let binary = std::fs::canonicalize(&binary).unwrap_or(binary);
        Self {
            binary,
            options: RunOptions::default(),
        }
    }

    pub fn with_options(mut self, options: RunOptions) -> Self {
        self.options = options;
        self
    }
}

#[async_trait::async_trait]
impl Runner for CliRunner {
    fn kind(&self) -> crate::runner::RunnerKind {
        crate::runner::RunnerKind::Cli
    }

    fn identity(&self) -> crate::runner::RunIdentity {
        // The options are what this runner was told to pass. A `--config` file
        // can override them inside the child, which this process never reads,
        // so an unset option stays absent rather than being invented.
        crate::runner::RunIdentity {
            model: self.options.model.clone(),
            base_url: self.options.base_url.clone(),
            permission_mode: self.options.permission_mode,
        }
    }

    async fn run(&self, case: &EvalCase, attempt: u32) -> RunOutcome {
        let start = Instant::now();
        let mut artifact = RunArtifact {
            case_id: case.id.clone(),
            attempt,
            session_id: None,
            answer: String::new(),
            tools: Vec::new(),
            tokens: Default::default(),
            duration_ms: 0,
            stop: None,
            failure: None,
            turns: None,
        };

        // The work dir is kept even when the attempt fails, so the caller can
        // grade whatever half-finished state the run left behind.
        let work_dir = match crate::suite::prepare_work_dir(case) {
            Ok(dir) => dir,
            Err(error) => {
                artifact.failure = Some(RunFailure::harness(error.to_string()));
                return RunOutcome {
                    artifact,
                    work_dir: std::env::temp_dir().join("tact-eval").join("unprepared"),
                };
            }
        };

        let mut command = Command::new(&self.binary);
        command
            .args(self.options.to_flags())
            .arg("--notifications=false")
            .arg("headless")
            .arg(&case.prompt)
            .current_dir(&work_dir);

        let outcome = match crate::process::run_child(
            command,
            Duration::from_secs(case.timeout_secs),
        )
        .await
        {
            Ok(outcome) => outcome,
            Err(error) => {
                artifact.failure = Some(RunFailure::startup(error.to_string()));
                artifact.duration_ms = crate::runner::elapsed_ms(start);
                return RunOutcome { artifact, work_dir };
            }
        };

        let stderr_lines: Vec<String> = outcome.stderr.lines().map(str::to_string).collect();
        artifact.session_id = parse_session_id(&stderr_lines);
        artifact.tools = parse_tools(&stderr_lines);
        artifact.tokens = parse_tokens(&stderr_lines);
        artifact.turns = parse_turns(&stderr_lines);
        artifact.answer = outcome.stdout.trim_end().to_string();
        artifact.duration_ms = crate::runner::elapsed_ms(start);

        if outcome.timed_out() {
            artifact.failure = Some(RunFailure::Timeout {
                limit_secs: case.timeout_secs,
            });
            return RunOutcome { artifact, work_dir };
        }

        artifact.stop = parse_stop(&stderr_lines, outcome.end);

        if !outcome.succeeded() {
            artifact.failure = Some(RunFailure::harness(format!(
                "tact-ui ended {:?}: {}",
                outcome.end,
                tail(&outcome.stderr)
            )));
        }

        RunOutcome { artifact, work_dir }
    }
}

/// `[session id: <id>]`, printed by headless on stderr at teardown.
fn parse_session_id(lines: &[String]) -> Option<String> {
    lines.iter().find_map(|line| {
        line.strip_prefix("[session id: ")
            .and_then(|rest| rest.strip_suffix(']'))
            .map(str::to_string)
    })
}

/// Tool lines from the headless log.
///
/// Two different events share the `tool failed` prefix: a finished step
/// (`[tool failed] {tool} {message} (Nms)`) and a provider-activity failure
/// (`[tool failed] {error}`), which names no tool at all. Treating the second
/// as the first is how an error message becomes a tool name.
///
/// So a name is recorded only when the line proves it: an `ok` or `cancelled`
/// status can only come from a finished step, and a `failed` line with a
/// duration suffix is a finished step too. A bare `[tool failed] …` stays
/// unattributed, with the text kept as the message.
fn parse_tools(lines: &[String]) -> Vec<ToolRecord> {
    lines
        .iter()
        .filter_map(|line| {
            let rest = line.strip_prefix("[tool ")?;
            let (status, rest) = rest.split_once("] ")?;
            let status = match status {
                "ok" => ToolStatus::Succeeded,
                "failed" => ToolStatus::Failed,
                "cancelled" => ToolStatus::Cancelled,
                _ => return None,
            };

            let (text, duration_ms) = split_duration(rest);

            // An `ok` or `cancelled` line always came from a finished step, and
            // so did any line carrying a duration. A failed line with neither
            // could be a finished step without a duration or a bare failure, so
            // it stays unattributed rather than guessing at a name.
            let attributed = matches!(status, ToolStatus::Succeeded | ToolStatus::Cancelled)
                || duration_ms.is_some();
            if !attributed {
                return Some(ToolRecord {
                    name: None,
                    args: None,
                    status,
                    duration_ms: None,
                    message: Some(text.to_string()),
                });
            }

            let (name, message) = match text.split_once(' ') {
                Some((name, message)) => (name, message),
                None => (text, ""),
            };
            Some(ToolRecord {
                name: Some(name.to_string()),
                // The finished-step line carries the step message, not the
                // argument summary; the log does not print the latter.
                args: None,
                status,
                duration_ms,
                message: Some(message.to_string()),
            })
        })
        .collect()
}

/// Split a trailing ` (Nms)` off a line, if present.
fn split_duration(text: &str) -> (&str, Option<u64>) {
    match text.rsplit_once(" (") {
        Some((head, tail)) => match tail.strip_suffix("ms)") {
            Some(millis) => match millis.parse::<u64>() {
                Ok(millis) => (head, Some(millis)),
                Err(_) => (text, None),
            },
            None => (text, None),
        },
        None => (text, None),
    }
}

/// `[tokens] prompt=.. completion=.. total=..`, emitted once per model call.
///
/// The lines accumulate: a run with three calls prints three, and overwriting
/// would report only the last call's cost. Cache and reasoning counts are not on
/// this line, so they stay `None` rather than being guessed as zero.
fn parse_tokens(lines: &[String]) -> TokenTotals {
    let mut totals = TokenTotals::default();
    for line in lines {
        let Some(rest) = line.strip_prefix("[tokens] ") else {
            continue;
        };
        for field in rest.split_whitespace() {
            let Some((key, value)) = field.split_once('=') else {
                continue;
            };
            let Ok(value) = value.parse::<u64>() else {
                continue;
            };
            match key {
                "prompt" => totals.prompt += value,
                "completion" => totals.completion += value,
                _ => {}
            }
        }
    }
    totals
}

/// `[turn] n/max`: the last line carries the run's turn count.
fn parse_turns(lines: &[String]) -> Option<u32> {
    lines.iter().rev().find_map(|line| {
        let rest = line.strip_prefix("[turn] ")?;
        rest.split('/').next()?.trim().parse().ok()
    })
}

/// The closest the CLI path can get to a stop reason, in precedence order.
///
/// Headless prints two different empty-run warnings, one per branch of its
/// final-message match, and both mean the model produced nothing to read. A
/// turn counter that reached its own cap means the loop stopped on the cap
/// rather than on the model.
///
/// A clean exit with an answer is all the success case left: it cannot separate
/// `Completed` from other self-terminated runs, so it reports `Completed` only
/// then and unknown otherwise.
fn parse_stop(lines: &[String], end: crate::process::ChildEnd) -> Option<StopView> {
    let empty = lines.iter().any(|line| {
        line.contains("the final message carried no text")
            || line.contains("the run produced no final message")
    });
    if empty {
        return Some(StopView::EmptyTurn);
    }
    if turn_cap_reached(lines) {
        return Some(StopView::MaxTurns);
    }
    matches!(end, crate::process::ChildEnd::Exited(status) if status.success())
        .then_some(StopView::Completed)
}

/// `[turn] n/max` where `n == max`.
fn turn_cap_reached(lines: &[String]) -> bool {
    lines.iter().rev().find_map(|line| {
        let rest = line.strip_prefix("[turn] ")?;
        let (taken, max) = rest.split_once('/')?;
        Some(taken.trim().parse::<u32>().ok()? == max.trim().parse::<u32>().ok()?)
    }) == Some(true)
}

fn tail(text: &str) -> String {
    let lines: Vec<&str> = text.trim().lines().rev().take(6).collect();
    lines.into_iter().rev().collect::<Vec<_>>().join(" | ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::ChildEnd;

    fn lines(raw: &str) -> Vec<String> {
        raw.lines().map(str::to_string).collect()
    }

    #[test]
    fn a_session_id_line_is_read_whole() {
        let raw = lines("[session: s-1]\n[tool] bash ls\n[session id: abc-123]");
        assert_eq!(parse_session_id(&raw).as_deref(), Some("abc-123"));
    }

    #[test]
    fn tool_lines_carry_name_status_and_duration() {
        let raw = lines(
            "[tool ok] bash ls (12ms)\n\
             [tool failed] edit_file nope (3ms)\n\
             [tool cancelled] bash sleep (0ms)",
        );
        let tools = parse_tools(&raw);
        assert_eq!(tools.len(), 3);
        assert_eq!(tools[0].name.as_deref(), Some("bash"));
        assert_eq!(tools[0].message.as_deref(), Some("ls"));
        assert_eq!(tools[0].status, ToolStatus::Succeeded);
        assert_eq!(tools[0].duration_ms, Some(12));
        assert_eq!(tools[1].status, ToolStatus::Failed);
        assert_eq!(tools[1].name.as_deref(), Some("edit_file"));
        assert_eq!(tools[2].status, ToolStatus::Cancelled);
        assert_eq!(
            tools[0].args, None,
            "the log does not print an argument summary for a finished step"
        );
    }

    #[test]
    fn a_bare_failure_line_does_not_invent_a_tool_name() {
        // A provider-activity failure prints `[tool failed] {error}` with no
        // tool and no duration. Reading the error as a tool name would report a
        // tool called "too many requests".
        let tools = parse_tools(&lines("[tool failed] too many requests"));
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, None, "no tool was named");
        assert_eq!(tools[0].status, ToolStatus::Failed);
        assert_eq!(tools[0].message.as_deref(), Some("too many requests"));
    }

    #[test]
    fn a_failure_with_a_duration_is_a_finished_step() {
        // The duration suffix is what distinguishes a finished step from a bare
        // failure, so this one does carry a name.
        let tools = parse_tools(&lines("[tool failed] bash exit 1 (1500ms)"));
        assert_eq!(tools[0].name.as_deref(), Some("bash"));
        assert_eq!(tools[0].message.as_deref(), Some("exit 1"));
        assert_eq!(tools[0].duration_ms, Some(1500));
    }

    #[test]
    fn the_last_turn_line_is_the_turn_count() {
        let raw = lines("[turn] 1/500\n[turn] 2/500");
        assert_eq!(parse_turns(&raw), Some(2));
    }

    #[test]
    fn token_totals_are_read_from_the_single_usage_line() {
        let totals = parse_tokens(&lines("[tokens] prompt=10 completion=4 total=14"));
        assert_eq!(totals.prompt, 10);
        assert_eq!(totals.completion, 4);
        assert_eq!(
            totals.cache_hit, None,
            "cache hits are not on this line, so they are unknown rather than zero"
        );
    }

    #[test]
    fn one_usage_line_per_model_call_accumulates() {
        // A multi-turn run prints one line per call; the last one is not the
        // run's total.
        let totals = parse_tokens(&lines(
            "[tokens] prompt=10 completion=4 total=14\n\
             [tokens] prompt=20 completion=6 total=26",
        ));
        assert_eq!(totals.prompt, 30);
        assert_eq!(totals.completion, 10);
    }

    #[test]
    fn an_empty_answer_warning_is_an_empty_turn() {
        for warning in [
            "[warn] the final message carried no text; stdout is empty",
            "[warn] the run produced no final message; stdout is empty",
        ] {
            assert_eq!(
                parse_stop(&lines(warning), ChildEnd::TimedOut),
                Some(StopView::EmptyTurn),
                "{warning}"
            );
        }
    }

    #[test]
    fn a_turn_count_that_hit_its_cap_is_max_turns() {
        assert_eq!(
            parse_stop(&lines("[turn] 32/32"), ChildEnd::TimedOut),
            Some(StopView::MaxTurns)
        );
        assert_eq!(parse_stop(&lines("[turn] 5/32"), ChildEnd::TimedOut), None);
    }

    #[test]
    fn a_missing_exit_status_concludes_nothing() {
        assert_eq!(parse_stop(&[], ChildEnd::TimedOut), None);
    }
}
