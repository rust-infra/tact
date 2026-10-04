use std::sync::Arc;

use tact::{config::CliArgs, consts::TactPath, extract_text, store::DynSessionStore};

use crate::{
    session_bootstrap::{Notices, bootstrap_session, open_session},
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
    let mut agent = bootstrap_session(
        &tact_path,
        work_dir.clone(),
        skill_registry,
        session_id.clone(),
        session_store,
        None,
        Notices::Stderr,
    )
    .await?;

    // Restore any prior messages for resumed sessions.
    agent.ensure_session().await?;

    let prompt_message = build_user_message(&prompt, &work_dir).await;
    agent.agent_loop(Some(prompt_message)).await?;

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

    if let Some(final_content) = agent.runtime.context.last() {
        let text = extract_text(&final_content.content);
        println!("{text}");

        let summary = text.chars().take(200).collect::<String>();
        let _ = tact::notifications::notify_task_complete(&summary);
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
