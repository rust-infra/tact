//! Running a case in process, through the same bootstrap the TUI and headless
//! use.
//!
//! This path exists for the evidence the CLI cannot carry: `agent_loop_detailed`
//! returns an [`AgentLoopOutcome`] with the real stop reason, turn count, and
//! tool-call count. `AgentUpdate` drops all three, so a runner layered on
//! `HostSession` would have to guess them from the log.
//!
//! It is not a replacement for the CLI path. It shares the process with the
//! caller, so `tact::config::install` can run only once and every case in a run
//! inherits that model and permission mode. A caller that needs a different
//! configuration per case spawns `tact-eval` again with different flags.

use std::time::{Duration, Instant};

use tact_host::{Frontend, Notices, bootstrap_session};
use tact_protocol::{AgentUpdate, StepResult, StepStatus, TokenUsageInfo, update_channel};

use crate::case::EvalCase;
use crate::runner::{
    RunArtifact, RunFailure, RunOutcome, Runner, StopView, TokenTotals, ToolRecord, ToolStatus,
};

/// Runs cases in process, through the same bootstrap as the product frontends.
#[derive(Debug)]
pub struct HostRunner {
    /// `--config`, `--provider`, `--model`, and `--permission-mode` are read
    /// exactly as `tact-ui` reads them, so a suite is configured the same way
    /// as a real session.
    args: tact::config::CliArgs,
}

impl HostRunner {
    /// The caller must have installed these settings first with
    /// `tact::config::init_from_args`; a host run reads them from the process.
    pub fn new(options: crate::options::RunOptions) -> Self {
        Self {
            args: options.to_cli_args(),
        }
    }
}

#[async_trait::async_trait]
impl Runner for HostRunner {
    fn kind(&self) -> crate::runner::RunnerKind {
        crate::runner::RunnerKind::Host
    }

    fn identity(&self) -> crate::runner::RunIdentity {
        // The host path reads the installed settings, which are what the run
        // actually uses, so this is the observed identity rather than the
        // requested one.
        match tact::config::try_settings() {
            Some(settings) => crate::runner::RunIdentity {
                model: Some(settings.agent.model),
                base_url: Some(settings.llm.base_url),
                permission_mode: settings.permission_mode,
            },
            // No installed settings means this run should have failed at
            // startup; report the requested mode so the header is not blank.
            None => crate::runner::RunIdentity {
                model: None,
                base_url: None,
                permission_mode: self.args.permission_mode.unwrap_or_default(),
            },
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

        // A host run reads process-global settings. Saying so here beats a panic
        // from `settings()` several frames down in bootstrap.
        if tact::config::try_settings().is_none() {
            artifact.failure = Some(RunFailure::startup(
                "tact config is not installed; call tact::config::init_from_args first",
            ));
            return RunOutcome { artifact, work_dir };
        }

        let tact_path = tact::consts::TactPath::new(work_dir.clone());
        let store = match tact::store::open_sqlite_session_store(&tact_path.session_db_path()).await
        {
            Ok(store) => store,
            Err(error) => {
                artifact.failure = Some(RunFailure::startup(error.to_string()));
                return RunOutcome { artifact, work_dir };
            }
        };
        let lock_registry = tact_host::SessionLockRegistry::new();
        let (session_id, lock) =
            match tact_host::open_session(&self.args, &tact_path, &store, &lock_registry).await {
                Ok(opened) => opened,
                Err(error) => {
                    artifact.failure = Some(RunFailure::startup(error.to_string()));
                    return RunOutcome { artifact, work_dir };
                }
            };
        artifact.session_id = Some(session_id.clone());

        let skill_registry = match tact::skill::shared_skill_registry(&work_dir) {
            Ok(registry) => registry,
            Err(error) => {
                let _ = lock.release().await;
                artifact.failure = Some(RunFailure::startup(error.to_string()));
                return RunOutcome { artifact, work_dir };
            }
        };

        let (tx, rx) = update_channel(1024);
        let mut agent = match bootstrap_session(
            &tact_path,
            work_dir.clone(),
            skill_registry,
            session_id.clone(),
            store,
            Frontend::Observing(tx),
            Notices::Stderr,
        )
        .await
        {
            Ok(agent) => agent,
            Err(error) => {
                let _ = lock.release().await;
                artifact.failure = Some(RunFailure::startup(error.to_string()));
                return RunOutcome { artifact, work_dir };
            }
        };

        if let Err(error) = agent.ensure_session().await {
            // The bootstrap already started MCP servers. Releasing the lock
            // without shutting them down would leave child processes behind
            // for the whole suite, each holding the work dir that grading is
            // about to read.
            agent.shutdown_mcp().await;
            let _ = lock.release().await;
            artifact.failure = Some(RunFailure::startup(error.to_string()));
            return RunOutcome { artifact, work_dir };
        }

        let prompt = tact_host::user_message::build_user_message(&case.prompt, &work_dir).await;
        // The pump stops on this signal, not on the channel closing.
        // `background_run` hands a clone of the observer sender to a detached
        // task with no time limit, so waiting for `recv` to return `None` would
        // wait for that command, and a run with a background job would never
        // report at all.
        let (pump_shutdown_tx, pump_shutdown_rx) = tokio::sync::oneshot::channel();
        let pump = tokio::spawn(drain_events(rx, pump_shutdown_rx));

        // Cancel cooperatively rather than dropping the loop mid-tool. A
        // dropped future leaves any tool task it spawned running against the
        // work dir that grading is about to read, and skips the loop's own
        // interruption hooks.
        let cancel = agent.cancel_handle();
        let mut budget_expired = false;
        let result = {
            let mut run = Box::pin(agent.agent_loop_detailed(Some(prompt)));
            tokio::select! {
                result = &mut run => Some(result),
                _ = tokio::time::sleep(Duration::from_secs(case.timeout_secs)) => {
                    budget_expired = true;
                    cancel.cancel();
                    // The loop checks the cancel flag at its own boundaries; a
                    // tool that ignores it is bounded by its own timeout, so
                    // this grace is a backstop rather than a second budget.
                    tokio::time::timeout(Duration::from_secs(30), &mut run).await.ok()
                }
            }
        };

        agent.detach_event_observers();
        let _ = pump_shutdown_tx.send(());
        let (tools, tokens) = pump.await.unwrap_or_default();

        match result {
            Some(Ok(outcome)) => {
                artifact.stop = Some(StopView::from(outcome.stop));
                artifact.turns = Some(outcome.turns);
            }
            Some(Err(error)) => {
                artifact.failure = Some(RunFailure::harness(error.to_string()));
            }
            // The grace window expired after cancellation: the run never
            // returned, so there is no outcome to report.
            None => {}
        }

        // A run this harness stopped is not a run the agent finished. The
        // cooperative cancel usually returns `RunStop::Cancelled`, which would
        // otherwise be graded on the files written before the bell; a case that
        // wrote its output and then hung would pass. The stop and turn count
        // stay in the artifact as evidence, and the attempt is inconclusive.
        if budget_expired {
            artifact.failure = Some(RunFailure::Timeout {
                limit_secs: case.timeout_secs,
            });
        }

        artifact.tools = tools;
        artifact.tokens = tokens;
        // The last *assistant* text, not the last message. A cancel mid-tools
        // makes the kernel persist synthetic tool results as `User` messages,
        // so the tail is a user block with no text and the run's actual answer
        // sits one or more messages back.
        artifact.answer = last_assistant_text(&agent);
        artifact.duration_ms = crate::runner::elapsed_ms(start);

        // Teardown is part of the run: a subagent left running would keep
        // writing after the verdict.
        agent
            .tool_context
            .subagent_manager
            .cancel_all_and_persist()
            .await;
        let _ = agent.dispatch_session_end_hooks().await;
        agent.shutdown_mcp().await;
        let _ = lock.release().await;

        RunOutcome { artifact, work_dir }
    }
}

/// Accumulate what the observer channel carries until the run says it is over.
///
/// The shutdown signal, not the channel closing, is what ends the loop: a
/// detached background task keeps a sender alive for as long as its command
/// runs. Queued events are drained after the signal so the log does not lose
/// the last thing the turn did.
async fn drain_events(
    mut rx: tact_protocol::UpdateReceiver,
    shutdown: tokio::sync::oneshot::Receiver<()>,
) -> (Vec<ToolRecord>, TokenTotals) {
    let mut tools = Vec::new();
    let mut tokens = TokenTotals::default();
    let mut shutdown = shutdown;

    loop {
        let update = tokio::select! {
            biased;
            _ = &mut shutdown => break,
            update = rx.recv() => update,
        };
        let Some(update) = update else {
            break;
        };
        record(&mut tools, &mut tokens, update);
    }

    while let Ok(update) = rx.try_recv() {
        record(&mut tools, &mut tokens, update);
    }

    (tools, tokens)
}

/// The last assistant message that carries text.
///
/// Walks back past tool-only assistant turns (their message has no text) and
/// past synthetic `User` tool results the kernel appends on a cancel.
fn last_assistant_text(agent: &tact::Agent) -> String {
    agent
        .context()
        .iter()
        .rev()
        .filter(|message| matches!(message.role, tact_llm::Role::Assistant))
        .map(|message| tact::extract_text(&message.content))
        .find(|text| !text.trim().is_empty())
        .unwrap_or_default()
        .trim_end()
        .to_string()
}

fn record(tools: &mut Vec<ToolRecord>, tokens: &mut TokenTotals, update: AgentUpdate) {
    match update {
        AgentUpdate::StepFinished { result, .. } => tools.push(tool_record(&result)),
        // The event adapter emits `StepFailed` for every failed tool call and
        // never a `StepFinished` with `Failed` status, so without this arm the
        // host report would show no failures at all.
        AgentUpdate::StepFailed { error, .. } => tools.push(ToolRecord {
            name: None,
            args: None,
            status: ToolStatus::Failed,
            duration_ms: None,
            message: Some(error),
        }),
        AgentUpdate::TokenUsage(usage) => add_usage(tokens, &usage),
        _ => {}
    }
}

fn tool_record(result: &StepResult) -> ToolRecord {
    ToolRecord {
        name: Some(result.tool.clone()),
        args: Some(result.arg_summary.clone()),
        status: match result.status {
            StepStatus::Success => ToolStatus::Succeeded,
            StepStatus::Failed => ToolStatus::Failed,
            StepStatus::Cancelled => ToolStatus::Cancelled,
        },
        duration_ms: result.duration_us.map(|micros| micros / 1_000),
        message: Some(result.message.clone()),
    }
}

/// Each usage event is one model call, so the totals accumulate rather than
/// overwrite; a later event is not a running total.
///
/// Cache and reasoning counts are real here because this runner sees the typed
/// `TokenUsage`, which the CLI path's log line does not carry.
fn add_usage(totals: &mut TokenTotals, usage: &TokenUsageInfo) {
    totals.prompt += u64::from(usage.prompt);
    totals.completion += u64::from(usage.completion);
    *totals.cache_hit.get_or_insert(0) += u64::from(usage.prompt_cache_hit_tokens);
    *totals.reasoning.get_or_insert(0) += u64::from(usage.reasoning_tokens);
}

#[cfg(test)]
mod tests {
    use super::*;
    use tact_protocol::ToolPresentationInfo;

    fn result(tool: &str, status: StepStatus, micros: Option<u64>) -> StepResult {
        StepResult {
            tool: tool.into(),
            arg_summary: "args".into(),
            arg_full: None,
            status,
            message: String::new(),
            detail: None,
            duration_us: micros,
            permission_label: None,
            presentation: ToolPresentationInfo::generic("t"),
        }
    }

    #[test]
    fn tool_records_carry_status_and_milliseconds() {
        let record = tool_record(&result("bash", StepStatus::Failed, Some(1_500_000)));
        assert_eq!(record.name.as_deref(), Some("bash"));
        assert_eq!(record.status, ToolStatus::Failed);
        assert_eq!(record.duration_ms, Some(1500));
    }

    /// The pump returns when told to, even while a sender is still alive.
    ///
    /// This is the hang that would make a run with a background command never
    /// report: the detached task holds a sender clone with no time limit, so
    /// the channel never closes.
    #[tokio::test]
    async fn the_pump_stops_on_the_shutdown_signal_with_a_live_sender() {
        let (tx, rx) = tact_protocol::update_channel(16);
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
        let pump = tokio::spawn(drain_events(rx, shutdown_rx));

        // Stands in for the detached background task's reporter.
        let _detached_sender = tx.clone();
        shutdown_tx.send(()).unwrap();

        tokio::time::timeout(std::time::Duration::from_secs(5), pump)
            .await
            .expect("the pump must return without the channel closing")
            .expect("the pump must not panic");
    }

    #[tokio::test]
    async fn usage_events_accumulate_rather_than_overwrite() {
        let mut totals = TokenTotals::default();
        add_usage(
            &mut totals,
            &TokenUsageInfo {
                prompt: 10,
                completion: 2,
                total: 12,
                prompt_cache_hit_tokens: 6,
                prompt_cache_miss_tokens: 4,
                reasoning_tokens: 1,
            },
        );
        add_usage(
            &mut totals,
            &TokenUsageInfo {
                prompt: 5,
                completion: 3,
                total: 8,
                ..Default::default()
            },
        );
        assert_eq!(totals.prompt, 15);
        assert_eq!(totals.completion, 5);
        assert_eq!(totals.cache_hit, Some(6));
        assert_eq!(totals.reasoning, Some(1));
    }
}
