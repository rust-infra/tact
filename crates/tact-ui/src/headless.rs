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

/// Drain agent events onto stderr for as long as `rx` is open.
///
/// A dedicated task rather than a callback because the events are produced
/// inside `agent_loop`, which the caller must also await: draining inline would
/// deadlock the unbounded channel's consumer against its producer's turn.
///
/// Each line is flushed as it is written. That is the whole point — a run that
/// hangs must still show what it last did, and a CI job that is killed must not
/// lose the log along with the process.
async fn pump_events_to_stderr(mut rx: tokio::sync::mpsc::UnboundedReceiver<AgentUpdate>) {
    use std::io::Write;

    while let Some(update) = rx.recv().await {
        if headless_log::is_noise(&update) {
            continue;
        }
        let Some(line) = headless_log::format_event(&update) else {
            continue;
        };
        // `stderr` is unbuffered, but the explicit flush makes the guarantee
        // local to this line rather than to the reader's knowledge of std.
        let mut stderr = std::io::stderr().lock();
        let _ = writeln!(stderr, "{line}");
        let _ = stderr.flush();
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
    // The observer channel is what makes the run legible: without it every
    // event the agent emits is dropped, and a long or wedged run is
    // indistinguishable from a working one until it exits. It is *not* a UI
    // channel — headless still cannot answer permission prompts, so a caller
    // that needs its tools allowed must choose a mode that does not ask
    // (`--permission-mode auto`; the scheduled review does this on the
    // throwaway CI runner itself).
    let (observer_tx, observer_rx) = tokio::sync::mpsc::unbounded_channel();
    let pump = tokio::spawn(pump_events_to_stderr(observer_rx));

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

    // Close the observer so the pump drains and returns instead of waiting for
    // a sender the agent will never drop on its own.
    agent.runtime.observer_tx = None;
    let _ = pump.await;

    // A failed turn is still worth reporting against the stats below, so the
    // error is surfaced rather than swallowed — but the log lines it produced
    // are already on stderr by now.
    loop_result?;

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
