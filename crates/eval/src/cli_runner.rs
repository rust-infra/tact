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
use std::process::Stdio;
use std::time::{Duration, Instant};

use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::process::Command;

use crate::case::EvalCase;
use crate::options::RunOptions;
use crate::runner::{
    RunArtifact, RunFailure, RunOutcome, Runner, StopView, TokenTotals, ToolRecord, ToolStatus,
};

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
    fn name(&self) -> &'static str {
        "cli"
    }

    fn identity(&self) -> crate::runner::RunIdentity {
        // The options are what this runner was told to pass. A `--config` file
        // can override them inside the child, which this process never reads,
        // so an unset option stays absent rather than being invented.
        crate::runner::RunIdentity {
            model: self.options.model.clone(),
            base_url: self.options.base_url.clone(),
            permission_mode: self.options.permission_mode.as_str().to_string(),
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
            .current_dir(&work_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                artifact.failure = Some(RunFailure::startup(error.to_string()));
                artifact.duration_ms = crate::runner::elapsed_ms(start);
                return RunOutcome { artifact, work_dir };
            }
        };

        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");

        // One timeout over both pipes and the exit status, because a run that
        // hangs on a tool must fail even when it keeps printing progress.
        let collected = tokio::time::timeout(Duration::from_secs(case.timeout_secs), async {
            let (stdout, stderr) = tokio::join!(read_all(stdout), read_lines(stderr));
            let status = child.wait().await;
            (stdout, stderr, status)
        })
        .await;

        let (stdout, stderr_lines, status) = match collected {
            Ok(collected) => collected,
            Err(_) => {
                let _ = child.start_kill();
                let _ = child.wait().await;
                artifact.failure = Some(RunFailure::Timeout {
                    limit_secs: case.timeout_secs,
                });
                artifact.duration_ms = crate::runner::elapsed_ms(start);
                return RunOutcome { artifact, work_dir };
            }
        };

        let status = status.ok();
        let stderr_text = stderr_lines.join("\n");
        artifact.session_id = parse_session_id(&stderr_lines);
        artifact.tools = parse_tools(&stderr_lines);
        artifact.tokens = parse_tokens(&stderr_lines);
        artifact.turns = parse_turns(&stderr_lines);
        artifact.stop = parse_stop(&stderr_lines, status);

        if !status.is_some_and(|status| status.success()) {
            artifact.failure = Some(RunFailure::harness(format!(
                "tact-ui exited {status:?}: {}",
                tail(&stderr_text)
            )));
        }

        // stdout is the final answer by contract; trailing newlines are not
        // part of it.
        artifact.answer = stdout.trim_end().to_string();
        artifact.duration_ms = crate::runner::elapsed_ms(start);
        RunOutcome { artifact, work_dir }
    }
}

async fn read_all(reader: impl AsyncRead + Unpin) -> String {
    let mut buf = String::new();
    let mut reader = BufReader::new(reader);
    let mut line = String::new();
    // Line by line rather than `read_to_string` so a multi-byte character split
    // across a chunk boundary cannot fail the whole read.
    while let Ok(count) = reader.read_line(&mut line).await {
        if count == 0 {
            break;
        }
        buf.push_str(&line);
        line.clear();
    }
    buf
}

async fn read_lines(reader: impl AsyncRead + Unpin) -> Vec<String> {
    let mut lines = Vec::new();
    let mut reader = BufReader::new(reader);
    let mut line = String::new();
    while let Ok(count) = reader.read_line(&mut line).await {
        if count == 0 {
            break;
        }
        lines.push(line.trim_end().to_string());
        line.clear();
    }
    lines
}

/// `[session id: <id>]`, printed by headless on stderr at teardown.
fn parse_session_id(lines: &[String]) -> Option<String> {
    lines.iter().find_map(|line| {
        line.strip_prefix("[session id: ")
            .and_then(|rest| rest.strip_suffix(']'))
            .map(str::to_string)
    })
}

/// `[tool ok] bash ls (12ms)` and friends.
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
            let (name, args) = match rest.split_once(' ') {
                Some((name, args)) => (name, args),
                None => (rest, ""),
            };
            let (args, duration_ms) = match args.rsplit_once(" (") {
                Some((args, ms)) if ms.ends_with("ms)") => {
                    (args, ms.trim_end_matches("ms)").parse::<u64>().ok())
                }
                _ => (args, None),
            };
            Some(ToolRecord {
                name: name.to_string(),
                args: args.to_string(),
                status,
                duration_ms,
            })
        })
        .collect()
}

/// `[tokens] prompt=.. completion=.. total=..`, emitted once per model call.
///
/// The lines accumulate: a run with three calls prints three, and overwriting
/// would report only the last call's cost. Cache and reasoning counts are not on
/// this line, so they stay zero rather than being guessed.
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

/// The closest the CLI path can get to a stop reason.
///
/// Headless prints its "no text" warning exactly when the model produced
/// nothing, which is `EmptyTurn`. A clean exit with a non-empty answer is all
/// the success case this path can distinguish: it cannot tell `Completed` from
/// a run that stopped for its own reasons, so it reports `Completed` only then
/// and unknown otherwise.
fn parse_stop(lines: &[String], status: Option<std::process::ExitStatus>) -> Option<StopView> {
    if lines
        .iter()
        .any(|line| line.contains("the final message carried no text"))
    {
        return Some(StopView::EmptyTurn);
    }
    status
        .is_some_and(|status| status.success())
        .then_some(StopView::Completed)
}

fn tail(text: &str) -> String {
    let lines: Vec<&str> = text.trim().lines().rev().take(6).collect();
    lines.into_iter().rev().collect::<Vec<_>>().join(" | ")
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(tools[0].name, "bash");
        assert_eq!(tools[0].args, "ls");
        assert_eq!(tools[0].status, ToolStatus::Succeeded);
        assert_eq!(tools[0].duration_ms, Some(12));
        assert_eq!(tools[1].status, ToolStatus::Failed);
        assert_eq!(tools[2].status, ToolStatus::Cancelled);
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
        assert_eq!(totals.cache_hit, 0, "cache hits are not on this line");
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
        let raw = lines("[warn] the final message carried no text; stdout is empty");
        assert_eq!(parse_stop(&raw, None), Some(StopView::EmptyTurn));
    }

    #[test]
    fn a_missing_exit_status_concludes_nothing() {
        assert_eq!(parse_stop(&[], None), None);
    }
}
