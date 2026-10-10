use std::sync::Arc;

use tact_extensions::{config::CliArgs, consts::TactPath, extract_text, store::DynSessionStore};
use tact_protocol::{PluginId, RequestId};

use crate::{
    session_bootstrap::{Notices, bootstrap_session, open_session},
    session_lock::SessionLockRegistry,
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
    // cancellation and MCP shutdown all read it after `chat.submit` returns.
    let agent = Arc::new(tokio::sync::Mutex::new(agent));
    let runtime = headless_runtime(serving, Arc::clone(&agent), cancel_flag)?;

    let invocation = runtime.invocation(
        RequestId::from(uuid::Uuid::new_v4().to_string()),
        PluginId::from("tact.agent"),
        "headless",
    );
    // The raw prompt travels; the **turn** it says (the `@` file / `![]` image
    // message) is assembled by the Chat extension, the same turn the TUI
    // submits. Headless therefore gets Stop-hook continuations and
    // TaskCompleted hooks exactly as the TUI does.
    let input = serde_json::json!({ "prompt": prompt });
    // Map the Kernel error back to `anyhow` by its message alone, so a failed
    // run surfaces the same text (`agent_loop`'s error, carried in
    // `KernelError::message`) it did when the host called `agent_loop`
    // directly, and `main` exits non-zero exactly as before.
    runtime
        .router()
        .invoke("chat.submit", invocation, input)
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

/// The headless run's wiring: the run capabilities are registered on the
/// **session's** serving context, not on a router of its own.
///
/// The Chat turn (`chat.submit`) and the Agent's `runs.*` then share one real
/// router with one real set of services, so starting a turn reaches the same
/// context a plugin's `storage.set` would reach — and the turn's run is
/// answered by the same `runs.start`. The serving context's policy keeps the
/// host's own control plane (`chat.submit` / `runs.start` / `runs.cancel`)
/// allowed — see [`crate::permission::serving_permission`] — so starting the run
/// never depends on a prompt this process cannot answer.
fn headless_runtime(
    serving: tact::RuntimeContext,
    agent: Arc<tokio::sync::Mutex<tact_extensions::Agent>>,
    cancel_flag: Arc<std::sync::atomic::AtomicBool>,
) -> anyhow::Result<tact::RuntimeContext> {
    // One shared Agent for both: the turn's `chat.submit` runs the run on the
    // same conversation the host keeps for teardown.
    crate::session_bootstrap::register_host_extensions(&serving, agent, cancel_flag)?;
    Ok(serving)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tact_extensions::{permission::PermissionMode, store::open_sqlite_session_store};
    use tact_llm::{ContentBlock, MockClient, Role, StopReason};

    /// The headless host starts its turn by invoking `chat.submit` through the
    /// session's serving router; there is no direct `agent_loop` call and no
    /// host-side message assembly. This drives that exact wiring (the helper the
    /// entry point uses) with a real Agent behind the shared handle, so it also
    /// proves the turn runs on the host's own Agent — and that the turn
    /// capability shares one router with the Kernel's service capabilities.
    #[tokio::test]
    async fn headless_submits_its_turn_through_the_serving_router() {
        let mock = MockClient::new(vec![(
            vec![ContentBlock::Text {
                text: "routed answer".into(),
            }],
            Some(StopReason::EndTurn),
        )]);
        let (agent, _work_dir) = crate::test_support::build_test_agent(mock, None);
        let cancel_flag = agent.runtime.cancel_flag.clone();
        let agent = Arc::new(tokio::sync::Mutex::new(agent));
        let (_directory, serving) = serving_context().await;
        let runtime = headless_runtime(serving, Arc::clone(&agent), cancel_flag)
            .expect("the host extensions register on the session's serving Router");

        // Reachable by name, on one router: the entry point goes through it, and
        // the Kernel's own services are registered beside it — as is the run the
        // turn executes.
        assert!(runtime.router().describe("chat.submit").is_some());
        assert!(runtime.router().describe("runs.start").is_some());
        assert!(runtime.router().describe("storage.set").is_some());

        // Same input shape the entry point sends: the raw prompt.
        let output = runtime
            .router()
            .invoke(
                "chat.submit",
                runtime.invocation(
                    RequestId::from("headless-router-test"),
                    PluginId::from("tact.agent"),
                    "headless",
                ),
                serde_json::json!({ "prompt": "hello from headless" }),
            )
            .await
            .expect("chat.submit answers through the Router");
        assert_eq!(output, serde_json::json!({ "accepted": true }));

        // The turn really ran on the shared Agent: the host can still read it.
        let agent = agent.lock().await;
        let saw_user_turn = agent.runtime.context.iter().any(|message| {
            message.role == Role::User
                && tact_extensions::extract_text(&message.content) == "hello from headless"
        });
        assert!(saw_user_turn, "the user turn reached the shared Agent");
    }

    /// A `Stop` hook that blocks keeps the turn going on the **headless** path.
    ///
    /// This is the asymmetry the chat turn closes: headless used to call
    /// `runs.start` directly and stop there, so a continuation-requesting Stop
    /// hook was silently ignored. Now the turn — and its continuation loop —
    /// lives behind `chat.submit`, which both hosts call.
    #[tokio::test]
    async fn headless_stop_hook_continuation_runs_a_second_turn() {
        let mock = MockClient::new(vec![
            (
                vec![ContentBlock::Text {
                    text: "first answer".into(),
                }],
                Some(StopReason::EndTurn),
            ),
            (
                vec![ContentBlock::Text {
                    text: "continued answer".into(),
                }],
                Some(StopReason::EndTurn),
            ),
        ]);
        let hook_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let seen = Arc::clone(&hook_calls);
        let (agent, _work_dir) = crate::test_support::build_test_agent(mock, None);
        let agent = agent.with_stop(move |_agent| {
            let remaining = seen.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            Box::pin(async move {
                if remaining == 0 {
                    Ok(tact_extensions::hook::HookControl::Block(
                        "keep going".into(),
                    ))
                } else {
                    Ok(tact_extensions::hook::HookControl::Continue)
                }
            })
        });
        let cancel_flag = agent.runtime.cancel_flag.clone();
        let agent = Arc::new(tokio::sync::Mutex::new(agent));
        let (_directory, serving) = serving_context().await;
        let runtime = headless_runtime(serving, Arc::clone(&agent), cancel_flag)
            .expect("the host extensions register on the session's serving Router");

        runtime
            .router()
            .invoke(
                "chat.submit",
                runtime.invocation(
                    RequestId::from("headless-continuation-test"),
                    PluginId::from("tact.agent"),
                    "headless",
                ),
                serde_json::json!({ "prompt": "start" }),
            )
            .await
            .expect("chat.submit answers through the Router");

        assert_eq!(
            hook_calls.load(std::sync::atomic::Ordering::Relaxed),
            2,
            "the blocked Stop hook must ask again after the continuation"
        );
        let agent = agent.lock().await;
        let saw_continuation = agent.runtime.context.iter().any(|message| {
            message.role == Role::User
                && tact_extensions::extract_text(&message.content) == "keep going"
        });
        assert!(
            saw_continuation,
            "the Stop hook's reason must become the next turn"
        );
    }

    /// The serving context the bootstrap builds: the Kernel services and the
    /// Session extension over real backing, with the session's policy. The
    /// temp directory is returned to keep the session database alive for the
    /// duration of the test.
    async fn serving_context() -> (tempfile::TempDir, tact::RuntimeContext) {
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
        (directory, serving)
    }
}
