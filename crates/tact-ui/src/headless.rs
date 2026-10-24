use std::sync::Arc;

use tact::{config::CliArgs, consts::TactPath, extract_text, store::DynSessionStore};
use tact_protocol::AgentUpdate;

use crate::{
    headless_log,
    session_bootstrap::{Frontend, Notices, bootstrap_session, open_session},
    session_lock::SessionLockRegistry,
    user_message::build_user_message,
};

pub async fn run_headless(
    args: CliArgs,
    prompt: String,
    tact_path: TactPath,
    session_store: DynSessionStore,
    lock_registry: Arc<SessionLockRegistry>,
) -> anyhow::Result<()> {
    if prompt.trim().is_empty() {
        eprintln!("Usage: tact-ui headless <PROMPT>");
        eprintln!("Try 'tact-ui headless --help' for more information.");
        std::process::exit(1);
    }

    let (session_id, session_lock) =
        open_session(&args, &tact_path, &session_store, lock_registry.as_ref()).await?;

    eprintln!("[session: {session_id}]");

    let run_result = run_headless_locked(args, prompt, tact_path, session_store, session_id).await;

    session_lock.release().await?;
    run_result
}

/// Drain agent events onto stderr until `shutdown` fires, then drain what is
/// already queued and return.
///
/// A dedicated task rather than a callback because the events are produced
/// inside `agent_loop`, which the caller must also await: draining inline would
/// stall the consumer while the producer's turn is still running.
///
/// Each line is flushed as it is written. That is the whole point — a run that
/// hangs must still show what it last did, and a CI job that is killed must not
/// lose the log along with the process.
///
/// The channel is unbounded because a sender must never block on a log reader:
/// the pump only writes to stderr, which the kernel back-pressures rather than
/// dropping. A reader that stops draining entirely (a suspended terminal) makes
/// the queue grow; the alternative — a bounded channel — would silently drop
/// the lines that explain a hang, which is the one thing this log is for.
///
/// **The end of the log is signalled, not inferred from the channel closing.**
/// Waiting for `rx.recv()` to return `None` waits for the last *sender*, and
/// `detach_event_observers` only drops the agent's own. A clone handed to a
/// detached task keeps one alive — `background_run` does exactly that, and its
/// task has no time limit — so the pump would block until the background
/// command finished, and the stats and final answer below it would never print.
async fn pump_events_to_stderr(
    mut rx: tokio::sync::mpsc::UnboundedReceiver<AgentUpdate>,
    shutdown: tokio::sync::oneshot::Receiver<()>,
) {
    use std::io::Write;

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
        let Some(line) = headless_log::format_event(&update) else {
            continue;
        };
        // `stderr` is unbuffered, but the explicit flush makes the guarantee
        // local to this line rather than to the reader's knowledge of std.
        let mut stderr = std::io::stderr().lock();
        let _ = writeln!(stderr, "{line}");
        let _ = stderr.flush();
    }

    // The turn is over, but a detached task may have queued output while the
    // last event was being written. Drain what is already there so the log ends
    // where the turn did, without waiting for a sender that may never close.
    while let Ok(update) = rx.try_recv() {
        if let Some(line) = headless_log::format_event(&update) {
            let mut stderr = std::io::stderr().lock();
            let _ = writeln!(stderr, "{line}");
            let _ = stderr.flush();
        }
    }
}

async fn run_headless_locked(
    _args: CliArgs,
    prompt: String,
    tact_path: TactPath,
    session_store: DynSessionStore,
    session_id: String,
) -> anyhow::Result<()> {
    let work_dir = tact_path.workdir().to_path_buf();
    let skill_registry = tact::skill::shared_skill_registry(tact_path.workdir())?;
    // Everything from the LLM client to the hook pass is shared with the TUI;
    // see `session_bootstrap`. Headless has no UI channel and no frame to
    // protect, so its startup notices go to stderr.
    //
    // The observer is what makes the run legible: without it every event the
    // agent emits is dropped, and a long or wedged run is indistinguishable
    // from a working one until it exits. It carries no answering channel —
    // headless still cannot answer permission prompts, so a caller that needs
    // its tools allowed must choose a mode that does not ask
    // (`--permission-mode auto`; the scheduled review does this on the
    // throwaway CI runner itself).
    let (observer_tx, observer_rx) = tokio::sync::mpsc::unbounded_channel();
    // The pump stops on this signal, not on the channel closing: a detached
    // background task holds a clone of the observer sender and may outlive the
    // turn indefinitely.
    let (pump_shutdown_tx, pump_shutdown_rx) = tokio::sync::oneshot::channel();
    let pump = tokio::spawn(pump_events_to_stderr(observer_rx, pump_shutdown_rx));

    let mut agent = bootstrap_session(
        &tact_path,
        work_dir.clone(),
        skill_registry,
        session_id.clone(),
        session_store,
        Frontend::Observing(observer_tx),
        Notices::Stderr,
    )
    .await?;

    // Restore any prior messages for resumed sessions.
    agent.ensure_session().await?;

    let prompt_message = build_user_message(&prompt, &work_dir).await;
    let loop_result = agent.agent_loop(Some(prompt_message)).await;

    // Stop the pump explicitly. Detaching the observers only drops the agent's
    // own sender; a `background_run` task holds another one for as long as its
    // command runs, so waiting for the channel to close would wait for that
    // command — and the stats and answer below would never print.
    agent.detach_event_observers();
    let _ = pump_shutdown_tx.send(());
    let _ = pump.await;

    // Report a failed turn as a log line, then still print what the run did.
    // The stats below are how a caller tells "it never started" from "it spent
    // twenty minutes and then failed", so they must not be skipped by the `?`.
    if let Err(error) = &loop_result {
        eprintln!("[error] {error:#}");
    }

    eprintln!("[session id: {session_id}]");
    eprintln!(
        "{}",
        agent
            .runtime
            .stats
            .read()
            .expect("session stats lock poisoned")
            .summary()
    );

    loop_result?;

    match agent.runtime.context.last() {
        Some(final_content) => {
            let text = extract_text(&final_content.content);
            if text.trim().is_empty() {
                // The answer is what a caller (CI, a script) consumes. Saying
                // so here is the difference between an empty stdout that looks
                // like a bug and one that names its cause.
                eprintln!("[warn] the final message carried no text; stdout is empty");
            }
            println!("{text}");

            let summary = text.chars().take(200).collect::<String>();
            let _ = tact::notifications::notify_task_complete(&summary);
        }
        None => eprintln!("[warn] the run produced no final message; stdout is empty"),
    }

    // The headless run is done: cancel and persist any still-running
    // background subagents before the process exits.
    agent
        .tool_context
        .subagent_manager
        .cancel_all_and_persist()
        .await;

    // SessionEnd hooks fire once at teardown, symmetrical with SessionStart.
    let _ = agent.dispatch_session_end_hooks().await;

    agent.shutdown_mcp().await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The pump returns when told to, even while a sender is still alive.
    ///
    /// This is the headless hang in miniature: `background_run` hands a clone
    /// of the observer sender to a detached task that has no time limit, so
    /// waiting for the channel to close waits for that task. The shutdown
    /// signal is what makes the turn's end observable on its own.
    #[tokio::test]
    async fn the_pump_stops_on_the_shutdown_signal_with_a_live_sender() {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
        let pump = tokio::spawn(pump_events_to_stderr(rx, shutdown_rx));

        // Stand in for the detached background task's reporter.
        let _detached_sender = tx.clone();

        shutdown_tx.send(()).unwrap();

        tokio::time::timeout(std::time::Duration::from_secs(5), pump)
            .await
            .expect("the pump must return without the channel closing")
            .expect("the pump task must not panic");
    }

    /// Events queued before the signal are still drained, so the log does not
    /// lose the last thing the turn did.
    #[tokio::test]
    async fn the_pump_drains_queued_events_before_returning() {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();

        // Queue first, then signal: `select!` is biased toward the signal, so
        // these are exactly the events only the drain loop can recover.
        tx.send(AgentUpdate::Info("queued before the signal".into()))
            .unwrap();
        let pump = tokio::spawn(pump_events_to_stderr(rx, shutdown_rx));
        shutdown_tx.send(()).unwrap();

        tokio::time::timeout(std::time::Duration::from_secs(5), pump)
            .await
            .expect("the pump must return")
            .expect("the pump task must not panic");
    }
}
