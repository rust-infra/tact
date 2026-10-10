use std::sync::Arc;

use tact_extensions::{
    config::CliArgs, consts::TactPath, extensions::agent::AgentExtension, extract_text,
    store::DynSessionStore,
};
use tact_protocol::{PluginId, RequestId};

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
    let skill_registry = tact_extensions::skill::shared_skill_registry(tact_path.workdir())?;
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

    // The two steps that need `&mut Agent` (or a readable field) run before the
    // Agent moves behind the Router's shared handle.
    //
    // Restore any prior messages for resumed sessions.
    agent.ensure_session().await?;
    // The user turn is built here, from the raw prompt.
    let prompt_message = build_user_message(&prompt, &work_dir).await;
    // The session's serving context, cloned out before the Agent moves behind
    // its shared handle. It already carries the shared event transport the
    // bootstrap installed, so the Router's `RuntimeServices` publish to the same
    // stream the session's trajectory recorder and any subscribed client read.
    let serving = agent
        .serving_context
        .clone()
        .expect("bootstrap installs the session's serving context");
    let cancel_flag = agent.runtime.cancel_flag.clone();

    // The run now goes through the Router, but the host keeps the Agent for
    // teardown: stats, the final message, session-end hooks, subagent
    // cancellation and MCP shutdown all read it after `runs.start` returns.
    let agent = Arc::new(tokio::sync::Mutex::new(agent));
    let runtime = headless_runtime(
        serving,
        AgentExtension::from_shared(Arc::clone(&agent), cancel_flag),
    )?;

    let invocation = runtime.invocation(
        RequestId::from(uuid::Uuid::new_v4().to_string()),
        PluginId::from("tact.agent"),
        "headless",
    );
    // The built turn travels as full content, so blocks survive the boundary
    // exactly as `build_user_message` produced them.
    let input = serde_json::json!({ "content": prompt_message.content });
    // Map the Kernel error back to `anyhow` by its message alone, so a failed
    // run surfaces the same text (`agent_loop`'s error, carried in
    // `KernelError::message`) it did when the host called `agent_loop`
    // directly, and `main` exits non-zero exactly as before.
    runtime
        .router()
        .invoke("runs.start", invocation, input)
        .await
        .map_err(|error| anyhow::anyhow!("{}", error.message()))?;

    eprintln!("[session id: {session_id}]");

    let mut agent = agent.lock().await;
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
        let _ = tact_extensions::notifications::notify_task_complete(&summary);
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

/// The headless run's wiring: the Agent extension is registered on the
/// **session's** serving context, not on a router of its own.
///
/// The run capability and the Kernel's service capabilities then share one real
/// router with one real set of services, so `runs.start` is answered by the same
/// context a plugin's `storage.set` would reach. The serving context's policy
/// keeps the host's own control plane (`runs.start` / `runs.cancel`) allowed —
/// see [`crate::permission::serving_permission`] — so starting the run never
/// depends on a prompt this process cannot answer.
fn headless_runtime(
    serving: tact::RuntimeContext,
    extension: AgentExtension,
) -> anyhow::Result<tact::RuntimeContext> {
    extension.register(&serving)?;
    Ok(serving)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tact_extensions::{permission::PermissionMode, store::open_sqlite_session_store};
    use tact_llm::{ContentBlock, MockClient, Role, StopReason};

    /// The headless host starts its run by invoking `runs.start` through the
    /// session's serving router; there is no direct `agent_loop` call. This
    /// drives that exact wiring (the helper the entry point uses) with a real
    /// Agent behind the shared handle, so it also proves `from_shared` keeps the
    /// host's Agent — and that the run capability shares one router with the
    /// Kernel's service capabilities.
    #[tokio::test]
    async fn headless_starts_its_run_through_the_serving_router() {
        let mock = MockClient::new(vec![(
            vec![ContentBlock::Text {
                text: "routed answer".into(),
            }],
            Some(StopReason::EndTurn),
        )]);
        let (agent, _work_dir) = crate::test_support::build_test_agent(mock, None);
        let cancel_flag = agent.runtime.cancel_flag.clone();
        let agent = Arc::new(tokio::sync::Mutex::new(agent));

        // The serving context the bootstrap builds: the Kernel services and the
        // Session extension over real backing, with the session's policy.
        let directory = tempfile::tempdir().expect("temp directory");
        let db_path = directory.path().join("session.db");
        let store = open_sqlite_session_store(&db_path)
            .await
            .expect("session store");
        let transport = tact::EventTransport::new(8);
        let settings = tact_extensions::permission::settings::PermissionSettings::load_from(
            &directory.path().join("settings.json"),
            None,
        );
        let serving = crate::session_bootstrap::build_serving_context(
            &db_path,
            &store,
            &transport,
            Arc::new(tact_trajectory::KernelTrajectoryRecorder::default()),
            crate::permission::serving_permission(PermissionMode::Auto, settings)
                .expect("serving policy"),
            &Notices::Stderr,
        )
        .await;

        let runtime = headless_runtime(
            serving,
            AgentExtension::from_shared(Arc::clone(&agent), cancel_flag),
        )
        .expect("the Agent extension registers on the session's serving Router");

        // Reachable by name, on one router: the entry point goes through it, and
        // the Kernel's own services are registered beside it.
        assert!(runtime.router().describe("runs.start").is_some());
        assert!(runtime.router().describe("storage.set").is_some());

        // Same input shape the entry point sends: the built turn as content.
        let input = serde_json::json!({
            "content": tact_llm::MessageContent::Blocks {
                content: vec![ContentBlock::Text {
                    text: "hello from headless".into(),
                }],
            },
        });
        let output = runtime
            .router()
            .invoke(
                "runs.start",
                runtime.invocation(
                    RequestId::from("headless-router-test"),
                    PluginId::from("tact.agent"),
                    "headless",
                ),
                input,
            )
            .await
            .expect("runs.start answers through the Router");
        assert!(
            output.get("run_id").is_some(),
            "the run identity is returned"
        );

        // The turn really ran on the shared Agent: the host can still read it.
        let agent = agent.lock().await;
        let saw_user_turn = agent.runtime.context.iter().any(|message| {
            message.role == Role::User
                && tact_extensions::extract_text(&message.content) == "hello from headless"
        });
        assert!(saw_user_turn, "the user turn reached the shared Agent");
    }
}
