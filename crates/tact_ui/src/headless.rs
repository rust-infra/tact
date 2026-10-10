use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use tact_extensions::{
    config::CliArgs, consts::TactPath, extensions::agent::AgentExtension, extract_text,
    store::DynSessionStore,
};
use tact_protocol::{CapabilityDeclaration, PluginId, RequestId};

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
    // The shared event transport the bootstrap installed: the Router's
    // `RuntimeServices` must publish to the same stream the session's
    // trajectory recorder and any subscribed client already read.
    let events = agent
        .runtime
        .runtime_event_service
        .clone()
        .expect("bootstrap installs a protocol event transport");
    let cancel_flag = agent.runtime.cancel_flag.clone();

    // The run now goes through the Router, but the host keeps the Agent for
    // teardown: stats, the final message, session-end hooks, subagent
    // cancellation and MCP shutdown all read it after `runs.start` returns.
    let agent = Arc::new(tokio::sync::Mutex::new(agent));
    let runtime = headless_runtime(
        events,
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

/// Wires the Router a headless run is started through.
///
/// The entry point passes an [`AgentExtension::from_shared`]; the test calls it
/// the same way, so it exercises the host's *only* way to start a run — the
/// Router path — rather than a direct `agent_loop` call beside it.
fn headless_runtime(
    events: Arc<dyn tact::EventService>,
    extension: AgentExtension,
) -> anyhow::Result<tact::RuntimeContext> {
    let router = tact::CapabilityRouter::new();
    let services = tact::RuntimeServices::with_event_and_permission(
        events,
        Arc::new(HeadlessControlPlanePermission),
    );
    let runtime = tact::RuntimeContext::with_services(router, services);
    extension.register(&runtime)?;
    Ok(runtime)
}

/// The headless host's permission policy for its **own** control-plane calls.
///
/// `CapabilityRouter::invoke` always runs `PermissionService::check`, and the
/// headless host has no interactive channel to answer an approval prompt. The
/// general policy, `PermissionManagerService`
/// (crates/tact_extensions/src/permission/kernel_service.rs), is fail-closed in
/// `Ask` mode: with no responder it denies. That would break headless for any
/// user on `mode = ask`, because starting the run is the host's own control
/// plane — not a tool the model asked for — and must not be gated on a prompt
/// the process cannot answer.
///
/// So this policy allows exactly the host's own control capabilities
/// (`runs.start`, `runs.cancel`) and denies everything else. It is **not** a
/// per-tool bypass: tool invocations never pass through it — they are
/// authorized by the Agent's own permission manager (`PermissionManagerService`
/// and the preflight gate in `tool_dispatch`). The narrow, deny-by-default
/// shape keeps that honest: a capability of any other kind is refused.
///
/// The alternative is a mode-aware `PermissionManagerService` that still allows
/// the host's control plane; this is the minimal version that does not change
/// the agent's tool-permission behavior.
struct HeadlessControlPlanePermission;

#[async_trait]
impl tact::PermissionService for HeadlessControlPlanePermission {
    async fn check(
        &self,
        declaration: &CapabilityDeclaration,
        _context: &tact::InvocationContext,
        _input: &Value,
    ) -> Result<(), tact::KernelError> {
        match declaration.name.as_str() {
            "runs.start" | "runs.cancel" => Ok(()),
            other => Err(tact::KernelError::permission_denied(format!(
                "headless host only authorizes its own control capabilities; \
                 {other} is not one"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tact::PermissionService;
    use tact_llm::{ContentBlock, MockClient, Role, StopReason};

    /// The headless host starts its run by invoking `runs.start` through the
    /// Router; there is no direct `agent_loop` call. This drives that exact
    /// wiring (the helper the entry point uses) with a real Agent behind the
    /// shared handle, so it also proves `from_shared` keeps the host's Agent.
    #[tokio::test]
    async fn headless_starts_its_run_through_the_router() {
        let mock = MockClient::new(vec![(
            vec![ContentBlock::Text {
                text: "routed answer".into(),
            }],
            Some(StopReason::EndTurn),
        )]);
        let (agent, _work_dir) = crate::test_support::build_test_agent(mock, None);
        let cancel_flag = agent.runtime.cancel_flag.clone();
        let agent = Arc::new(tokio::sync::Mutex::new(agent));

        let events: Arc<dyn tact::EventService> = Arc::new(tact::EventTransport::new(8));
        let runtime = headless_runtime(
            events,
            AgentExtension::from_shared(Arc::clone(&agent), cancel_flag),
        )
        .expect("the Agent extension registers on the headless Router");

        // Reachable by name: the entry point goes through the Router.
        assert!(runtime.router().describe("runs.start").is_some());

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

    /// The host's control-plane policy allows its own capabilities and denies
    /// everything else — it is not a blanket allow.
    #[tokio::test]
    async fn headless_permission_allows_only_the_host_control_capabilities() {
        let check = |name: &str| {
            let declaration = CapabilityDeclaration {
                name: name.into(),
                kind: tact_protocol::CapabilityKind::App,
                version: "1".into(),
                description: None,
                input_schema: None,
                output_schema: None,
                risk: tact_protocol::CapabilityRisk::Medium,
            };
            let context = tact::InvocationContext::new(
                RequestId::from("permission-probe"),
                PluginId::from("tact.agent"),
                "headless",
            );
            async move {
                HeadlessControlPlanePermission
                    .check(&declaration, &context, &serde_json::json!({}))
                    .await
            }
        };

        assert!(check("runs.start").await.is_ok());
        assert!(check("runs.cancel").await.is_ok());
        let denied = check("bash").await.expect_err("a tool is not allowed");
        assert_eq!(
            denied.category(),
            tact_protocol::ErrorCategory::PermissionDenied
        );
    }
}
