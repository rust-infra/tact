//! Interactive-mode command driver: bridges `UserCommand` from the TUI to `Agent`.

use std::{
    path::Path,
    sync::{Arc, atomic::Ordering},
};
use tact_extensions::extensions::chat::{TurnEntry, run_chat_turn};
use tact_extensions::runtime_event;
use tact_protocol::RuntimeEvent;

use tact_extensions::background::SharedBackgroundManager;
use tact_extensions::{Agent, utils::RwLockExt};
use tact_protocol::{AccountUpdate, PluginId, RequestId, RunId, RuntimeCommand};
use tact_view::{AgentErrorKind, UserCommand};
use tokio::{
    sync::{
        Mutex,
        mpsc::{UnboundedReceiver, UnboundedSender},
    },
    task::JoinHandle,
};

use crate::account;

/// The Agent shared between the driver, its in-flight run task and the
/// session's `runs.start` capability.
///
/// Before the run went through the capability protocol the driver *owned* the
/// `Agent` outright and the run task moved it in and out of the task. The
/// Router's [`AgentExtension`] needs shared ownership, so the driver now owns
/// an `Arc<Mutex<Agent>>` and every other command keeps answering from the
/// `Arc` handles cloned out of it at start (stats, ui_responder, managers,
/// cancel_flag, view_updates).
type SharedAgent = Arc<Mutex<Agent>>;

/// Process `UserCommand`s until the channel closes, then shut down MCP.
///
/// The driver owns the Agent behind a shared handle (`Arc<Mutex<Agent>>`) and
/// returns it, so the session's `runs.start` capability can reach the same
/// conversation. `SubmitTask` runs in a background task so `Cancel` can set
/// `cancel_flag` while the run is in progress. Integration tests drive this
/// with a fake TUI.
///
/// This convenience wrapper does **not** wire an account-update channel; balance
/// queries initiated through it are dropped. Use
/// [`run_command_loop_with_account`] when the caller wants to receive
/// [`AccountUpdate`] messages.
pub async fn run_command_loop(
    agent: Agent,
    user_cmd_rx: UnboundedReceiver<UserCommand>,
    image_work_dir: impl AsRef<Path>,
) -> SharedAgent {
    run_command_loop_with_account(agent, user_cmd_rx, image_work_dir, None).await
}

/// Like [`run_command_loop`], but forwards balance / usage quota results to the
/// provided account-update channel instead of mixing them into agent updates.
pub async fn run_command_loop_with_account(
    agent: Agent,
    mut user_cmd_rx: UnboundedReceiver<UserCommand>,
    image_work_dir: impl AsRef<Path>,
    account_tx: Option<UnboundedSender<AccountUpdate>>,
) -> SharedAgent {
    let image_work_dir = image_work_dir.as_ref().to_path_buf();
    let cancel_flag = agent.runtime.cancel_flag.clone();
    let view_updates = agent.tool_context.view_updates.clone();
    // Shared stats snapshot: QueryStats can read it without awaiting the
    // in-flight task (the Agent itself is locked by that task's run).
    let stats = agent.runtime.stats.clone();
    // Shared UI responder: routes TUI select responses to the waiter (parent
    // or subagent) even while the Agent is locked by the in-flight run.
    let ui_responder = agent.tool_context.ui_responder.clone();
    // Shared subagent manager: lets CancelSubagent flip a running child's
    // cooperative cancel flag without owning the parent Agent.
    let subagent_manager = agent.tool_context.subagent_manager.clone();
    // Shared background manager + session id: `/background` answers from the
    // manager directly instead of waiting for the in-flight turn to hand the
    // Agent back, so a listing opens the moment it is asked for.
    let background_manager = agent.tool_context.background_manager.clone();
    let background_session_id = agent.runtime.session_id.clone();

    // The session's serving context, cloned out before the Agent moves behind
    // the shared handle. The run is started through the capability protocol on
    // it (see `run_submit`), so `runs.start` becomes the interactive host's run
    // entry exactly as it already is for the headless host.
    let serving = agent.serving_context.clone();
    let mut agent: Option<SharedAgent> = Some(Arc::new(Mutex::new(agent)));
    // Register the host's control-plane extensions — the Agent's `runs.*` and
    // Chat's `chat.submit` — on the session's serving context, so the turn task
    // reaches the one conversation through `chat.submit` → `runs.start`. An
    // Agent built directly (the integration tests, a degraded setup) has no
    // serving context: keep today's direct `agent_loop` path for those, so the
    // driver is still usable without a Kernel router. A registration failure (a
    // router that already carries these capabilities) degrades the same way
    // rather than stranding the run.
    let serving = serving.and_then(|serving| {
        let extension_agent = agent.as_ref().expect("agent just wrapped").clone();
        match crate::session_bootstrap::register_host_extensions(
            &serving,
            extension_agent,
            cancel_flag.clone(),
        ) {
            Ok(()) => Some(serving),
            Err(error) => {
                let _ =
                    view_updates.emit_runtime_event(runtime_event::error(AgentErrorKind::Other(
                        format!("Run capabilities unavailable; using the direct run path: {error}"),
                    )));
                None
            }
        }
    });
    let mut active: Option<JoinHandle<SharedAgent>> = None;
    // A `SubagentFinishedNotification` that arrives while a turn is in flight
    // cannot be dropped: the result may already have been drained from the
    // queue at the start of the turn, and once the turn exits there is nothing
    // left to wake the parent. Retain the wake-up until the in-flight task
    // completes, then submit it.
    let mut pending_subagent_wakeup = false;

    loop {
        let cmd = if active.is_some() {
            tokio::select! {
                result = active.as_mut().expect("active task") => {
                    agent = Some(result.expect("active task join panicked"));
                    active = None;
                    if pending_subagent_wakeup {
                        pending_subagent_wakeup = false;
                        spawn_wakeup_task(&mut agent, &mut active, &serving)
                            .await;
                    }
                    continue;
                }
                cmd = user_cmd_rx.recv() => cmd,
            }
        } else {
            user_cmd_rx.recv().await
        };
        let Some(cmd) = cmd else { break };

        match cmd {
            UserCommand::Runtime(RuntimeCommand::StartRun { run_id, input }) => {
                let Some(task) = input.get("message").and_then(|value| value.as_str()) else {
                    let _ = view_updates.emit_runtime_event(runtime_event::error(
                        AgentErrorKind::Other(
                            "Runtime StartRun requires a string `message`".into(),
                        ),
                    ));
                    continue;
                };
                if let Some(handle) = active.take() {
                    agent = Some(handle.await.expect("runtime start task join panicked"));
                }
                pending_subagent_wakeup = false;
                let task_agent = agent.take().expect("agent available for runtime start");
                let task = task.to_string();
                let serving = serving.clone();
                active = Some(tokio::spawn(async move {
                    run_submit(task_agent, serving, task, Some(run_id)).await
                }));
            }
            UserCommand::Runtime(RuntimeCommand::RespondInteraction { response }) => {
                let _ = ui_responder.respond(response);
            }
            UserCommand::Runtime(RuntimeCommand::CancelRun { .. }) => {
                cancel_flag.store(true, Ordering::Relaxed);
                let _ = view_updates.emit_runtime_event(runtime_event::info("Cancelling..."));
            }
            UserCommand::Runtime(_) => {}
            UserCommand::Cancel => {
                cancel_flag.store(true, Ordering::Relaxed);
                let _ = view_updates.emit_runtime_event(runtime_event::info("Cancelling..."));
            }
            UserCommand::CancelSubagent { child_id } => {
                if subagent_manager.request_cancel(&child_id) {
                    let _ = subagent_manager.cancel(&child_id).await;
                    tact_extensions::subagent::emit_subagents_changed_view(
                        &view_updates,
                        &subagent_manager,
                    )
                    .await;
                    let _ = view_updates.emit_runtime_event(runtime_event::info(format!(
                        "Cancelling subagent {child_id}..."
                    )));
                } else {
                    let _ = view_updates.emit_runtime_event(runtime_event::info(format!(
                        "No running subagent {child_id} to cancel"
                    )));
                }
            }
            UserCommand::QueryStats => {
                // Immediate snapshot: does NOT wait for the running task —
                // stats live in an Arc<RwLock<SessionStats>> shared with the
                // agent, so /stats responds instantly even mid-run. Shown in the
                // read-out popup, like /background: a snapshot is not
                // conversation.
                let stats_text = stats.read_recover().summary();
                let _ = view_updates.emit_runtime_event(RuntimeEvent::PopupMarkdown {
                    run_id: None,
                    title: "Session Statistics".to_string(),
                    source: stats_text,
                });
            }
            UserCommand::QueryBackground(task_id) => {
                // `/background` and `/background <id>`. Answered here, from the
                // shared manager, so the popup opens immediately even while a
                // turn is running (the Agent is owned by that task).
                query_background(
                    &background_manager,
                    background_session_id.as_deref(),
                    task_id,
                    &view_updates,
                )
                .await;
            }
            UserCommand::SubagentFinishedNotification { .. } => {
                // If a turn is active, retain the wake-up until that turn's
                // JoinHandle completes. Otherwise the notification could be
                // lost in the gap between the final queue drain and turn exit.
                if active.is_none() {
                    spawn_wakeup_task(&mut agent, &mut active, &serving).await;
                } else {
                    pending_subagent_wakeup = true;
                }
            }
            UserCommand::SubmitTask(task) => {
                if let Some(handle) = active.take() {
                    agent = Some(handle.await.expect("submit task join panicked"));
                }
                // The new user turn drains pending subagent results itself;
                // do not add a redundant wake-up after it finishes.
                pending_subagent_wakeup = false;
                let task_agent = agent.take().expect("agent available for submit");
                let serving = serving.clone();
                active = Some(tokio::spawn(async move {
                    run_submit(task_agent, serving, task, None).await
                }));
            }
            UserCommand::RunMcpPrompt {
                server,
                name,
                arguments,
            } => {
                if let Some(handle) = active.take() {
                    agent = Some(handle.await.expect("mcp prompt task join panicked"));
                }
                // A prompt is a *starting message*, so it runs like any other
                // submitted turn: rendered here, then handed to the very same
                // `run_submit` helper as `SubmitTask` (no explicit run id), so
                // a plugin-prompt turn goes through `chat.submit` when a serving
                // context exists and keeps the direct path when it does not.
                pending_subagent_wakeup = false;
                // Render under a brief lock and *drop it before running*:
                // `run_submit` takes the same lock itself, and
                // `tokio::sync::Mutex` deadlocks if it is re-locked while held.
                let rendered = {
                    let agent_arc = agent.as_ref().expect("agent available for mcp prompt");
                    let guard = agent_arc.lock().await;
                    render_mcp_prompt(&guard, &server, &name, arguments).await
                };
                match rendered {
                    Ok(messages) => {
                        let task_agent = agent.take().expect("agent available for mcp prompt");
                        let serving = serving.clone();
                        active = Some(tokio::spawn(async move {
                            run_submit(task_agent, serving, messages, None).await
                        }));
                    }
                    Err(message) => {
                        // The same Error event the direct path emits, with the
                        // same run attribution: lock just long enough to emit.
                        agent
                            .as_ref()
                            .expect("agent available for mcp prompt")
                            .lock()
                            .await
                            .emit_update(runtime_event::error(AgentErrorKind::Other(message)));
                    }
                }
            }
            other => {
                if let Some(handle) = active.take() {
                    agent = Some(handle.await.expect("command join panicked"));
                }
                if let Some(agent_arc) = agent.take() {
                    let mut a = agent_arc.lock().await;
                    handle_user_command_with_account(
                        &mut a,
                        other,
                        &image_work_dir,
                        account_tx.as_ref(),
                    )
                    .await;
                    drop(a);
                    agent = Some(agent_arc);
                }
            }
        }
    }

    // The UI is gone: unblock any in-flight select waiter so the task can
    // finish instead of deadlocking on an answer that will never arrive.
    ui_responder.shutdown();

    // The parent is exiting: request cancellation and persist Cancelled for
    // every live child before detached tasks can be dropped by runtime shutdown.
    if subagent_manager.cancel_all_and_persist().await > 0 {
        let _ = view_updates.emit_runtime_event(runtime_event::info(
            "Cancelling background subagents (parent exiting)...",
        ));
    }

    if let Some(handle) = active.take() {
        agent = Some(handle.await.expect("final task join panicked"));
    }

    let agent = agent.expect("agent should be available after command loop");
    {
        // SessionEnd hooks fire once at teardown, symmetrical with SessionStart.
        let mut guard = agent.lock().await;
        let _ = guard.dispatch_session_end_hooks().await;
        guard.shutdown_mcp().await;
    }
    agent
}

/// Emit the `/background` read-out: the whole session's task listing
/// (`task_id = None`) or one task's pretty JSON.
///
/// Takes the shared manager instead of the `Agent` so the command loop can
/// answer while a turn owns the Agent — the same reason `QueryStats` is handled
/// at the loop level.
async fn query_background(
    manager: &SharedBackgroundManager,
    session_id: Option<&str>,
    task_id: Option<String>,
    view_updates: &tact_extensions::tool::ViewUpdateEmitter,
) {
    match manager.check(task_id.as_deref(), session_id).await {
        Ok(output) => {
            // Fenced code block keeps the one-line-per-task listing (and the
            // single-task pretty JSON) aligned and copyable. Shown in the popup
            // rather than the log: this is a read-out, not part of the
            // conversation.
            let _ = view_updates.emit_runtime_event(RuntimeEvent::PopupMarkdown {
                run_id: None,
                title: "⚙️ Background Tasks".to_string(),
                source: format!("```text\n{output}\n```"),
            });
        }
        Err(err) => {
            let _ = view_updates.emit_runtime_event(runtime_event::error(AgentErrorKind::Other(
                format!("Background check failed: {err}"),
            )));
        }
    }
}

async fn spawn_wakeup_task(
    agent: &mut Option<SharedAgent>,
    active: &mut Option<JoinHandle<SharedAgent>>,
    serving: &Option<tact::RuntimeContext>,
) {
    if active.is_some() {
        return;
    }
    // A wake-up turn only delivers queued results into the parent's context.
    // If a turn that just finished already drained the queue, the summary is
    // already in context and this turn would be empty — skip it entirely.
    let Some(agent_arc) = agent.as_ref() else {
        return;
    };
    if !agent_arc.lock().await.has_pending_subagent_results() {
        return;
    }
    let task_agent = Arc::clone(agent_arc);
    let serving = serving.clone();
    *active = Some(tokio::spawn(async move {
        // The queued result reaches the model during the drain below, so it
        // may legitimately not appear verbatim as a result "below"; point at
        // `check_subagent` as the fallback way to retrieve it.
        let prompt = "A background subagent finished. Review its result below \
                      (or call check_subagent if none is shown)."
            .to_string();
        run_submit(task_agent, serving, prompt, None).await
    }));
}

/// Runs one submitted turn on the shared Agent.
///
/// This is the driver's single submit entry: when the session has a serving
/// context the turn is submitted through the capability protocol
/// (`chat.submit`), and only a driver without one (the integration tests, a
/// degraded setup) calls the shared turn directly.
///
/// `requested_run_id` carries the run identity a `Runtime(StartRun)` command
/// asked for; it travels with the submission and is set on the Agent by the
/// chat turn itself, under the same per-turn lock that clears the cancel flag,
/// so the identity survives the boundary exactly as it did when the driver
/// called `agent_loop` directly.
///
/// Returns the shared Agent so the caller's `JoinHandle` hands it back the way
/// the old exclusive-ownership design did.
async fn run_submit(
    agent: SharedAgent,
    serving: Option<tact::RuntimeContext>,
    task: String,
    requested_run_id: Option<RunId>,
) -> SharedAgent {
    match serving {
        Some(serving) => {
            // The host starts the turn on its own control plane, exactly as the
            // headless host does. The turn's own failure is already reported
            // (the Error event the direct path emits, with the same text), so
            // the call's result is deliberately not re-reported here: emitting
            // both would put two Error rows in the transcript for one failure.
            let invocation = serving.invocation(
                RequestId::from(uuid::Uuid::new_v4().to_string()),
                PluginId::from("tact.agent"),
                "interactive",
            );
            let mut input = serde_json::json!({ "prompt": task });
            if let Some(run_id) = requested_run_id {
                input["run_id"] = serde_json::json!(run_id);
            }
            let _ = serving
                .router()
                .invoke("chat.submit", invocation, input)
                .await;
        }
        None => {
            // No serving context: the same turn, called directly.
            let _ = run_chat_turn(TurnEntry::Shared(&agent), &task, requested_run_id).await;
        }
    }
    agent
}

/// Handle a single user command (shared by the loop and tests).
///
/// This wrapper discards any account-related updates; tests that need to
/// observe them should use [`run_command_loop_with_account`].
/// Fetches one MCP prompt and renders it as the text of a user turn.
///
/// Split out from the command arm so the fetch-and-render path is reachable
/// without running a turn: this is the one place the router, the renderer and
/// the two error messages meet. The command loop renders here under a brief
/// lock and then hands the text to `run_submit`; a direct caller falls through
/// [`handle_user_command_with_account`]'s own arm and reuses the same render.
async fn render_mcp_prompt(
    agent: &Agent,
    server: &str,
    name: &str,
    arguments: std::collections::BTreeMap<String, String>,
) -> Result<String, String> {
    if name.is_empty() {
        return Err(
            "/mcp prompt needs a prompt name: /mcp prompt <server> <name> [key=value ...]"
                .to_string(),
        );
    }
    agent
        .mcp_router
        .get_prompt(
            server,
            name,
            tact_extensions::mcp::prompt_arguments_from_pairs(arguments),
        )
        .await
        .map_err(|error| format!("MCP prompt {server}/{name} failed: {error:#}"))
}

pub async fn handle_user_command(agent: &mut Agent, cmd: UserCommand, image_work_dir: &Path) {
    handle_user_command_with_account(agent, cmd, image_work_dir, None).await;
}

async fn handle_user_command_with_account(
    agent: &mut Agent,
    cmd: UserCommand,
    image_work_dir: &Path,
    account_tx: Option<&UnboundedSender<AccountUpdate>>,
) {
    match cmd {
        UserCommand::SubmitTask(task) => {
            // The same turn the loop submits, run by the one implementation:
            // the per-turn reset, the Stop-hook continuation loop and the
            // post-turn bookkeeping (task_complete / cancelled / TaskCompleted
            // hooks) all live in `run_chat_turn`, so this entry cannot drift
            // from the routed one. This caller owns the Agent exclusively, so
            // the turn runs on it directly.
            let _ = run_chat_turn(TurnEntry::Exclusive(&mut *agent), &task, None).await;
        }
        UserCommand::Compact => {
            agent.emit_update(runtime_event::info("[compacting]"));
            if let Err(error) = agent
                .compact_history_with_trigger(
                    tact_extensions::compact::CompactTrigger::Command,
                    None,
                )
                .await
            {
                agent.emit_update(runtime_event::error(AgentErrorKind::Other(format!(
                    "Compaction failed: {error}"
                ))));
            } else {
                agent.emit_update(runtime_event::info("Compaction complete."));
            }
        }
        UserCommand::QueryBalance => {
            let Some(account_tx) = account_tx else {
                return;
            };
            if !account::is_supported() {
                return;
            }
            match account::query_once().await {
                Ok(result) => {
                    let _ = account_tx.send(account::into_update(result));
                }
                Err(err) => {
                    let _ = account_tx.send(AccountUpdate::Error(err));
                }
            }
        }
        UserCommand::QueryStats => {
            // Handled at the loop level (immediate shared-stats snapshot);
            // reaching this arm means the caller bypassed the command loop.
        }
        UserCommand::QueryBackground(task_id) => {
            // Handled at the loop level (answering from the shared manager, so
            // it never awaits the in-flight turn); reaching this arm means the
            // caller bypassed the command loop. Answer anyway.
            query_background(
                &agent.tool_context.background_manager,
                agent.runtime.session_id.as_deref(),
                task_id,
                &agent.tool_context.view_updates,
            )
            .await;
        }
        UserCommand::SetPermissionMode(mode) => {
            let parsed = match mode.as_str() {
                "plan" => tact_extensions::permission::PermissionMode::Plan,
                "default" => tact_extensions::permission::PermissionMode::Default,
                _ => tact_extensions::permission::PermissionMode::Auto,
            };
            // TUI already shows the localized confirmation; do not emit Info here.
            agent.runtime.permission_manager.set_mode(parsed);
        }
        UserCommand::SetThinkingBudget(budget) => {
            agent.set_thinking_budget(budget);
        }
        UserCommand::SetReasoningEffort(effort) => {
            let parsed = effort.as_deref().and_then(|raw| raw.parse().ok());
            if effort.is_some() && parsed.is_none() {
                eprintln!("[driver] ignoring unparseable reasoning effort: {effort:?}");
            }
            agent.set_reasoning_effort(parsed);
        }
        UserCommand::SetModel(model) => {
            agent.set_model(model);
        }
        UserCommand::McpAuth { server } => {
            // Stream progress lines instead of buffering them: the URL is
            // reported *before* the flow blocks on the browser redirect, so
            // buffering would hide it for the whole round-trip — and forever,
            // if the user never authorizes, since the callback only times out.
            let (line_tx, line_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
            // `authorize_server` requires a `Send` notify closure, which rules
            // out capturing `&Agent` directly; the channel decouples the two.
            let mut notify = move |line: &str| {
                let _ = line_tx.send(line.to_owned());
            };
            let result = stream_auth_progress(
                tact_extensions::mcp::authorize_server(&server, &mut notify),
                line_rx,
                |line| agent.emit_update(runtime_event::info(line)),
            )
            .await;
            match result {
                Ok(()) => {
                    agent.emit_update(runtime_event::info(format!(
                        "Authorized MCP server {server}; reloading MCP servers..."
                    )));
                    let report = agent.reload_mcp_router().await;
                    for line in report.notice_lines() {
                        agent.emit_update(runtime_event::info(line));
                    }
                    agent.emit_update(runtime_event::info(format!(
                        "MCP reload complete ({} server(s) connected)",
                        report.connected.len()
                    )));
                }
                Err(error) => agent.emit_update(runtime_event::error(AgentErrorKind::Other(
                    format!("MCP authorization failed for {server}: {error:#}"),
                ))),
            }
        }
        UserCommand::McpList => {
            // Live view: describe what the agent's *current* router holds.
            // Never reload here — a reconnect would drop live stdio children
            // and duplicate remote dials just to print a table.
            match tact_extensions::mcp::describe_servers(&agent.mcp_router.server_summaries()) {
                Ok(views) => agent.emit_update(runtime_event::md_info(
                    crate::mcp_cli::render_live_listing(&views),
                )),
                Err(error) => agent.emit_update(runtime_event::error(AgentErrorKind::Other(
                    format!("MCP list failed: {error:#}"),
                ))),
            }
        }
        UserCommand::McpPrompts { server } => {
            // Live view, like `McpList`: the router the agent already holds.
            match agent.mcp_router.list_prompts(server.as_deref()).await {
                Ok(listing) => agent.emit_update(runtime_event::md_info(listing)),
                Err(error) => agent.emit_update(runtime_event::error(AgentErrorKind::Other(
                    format!("MCP prompts failed: {error:#}"),
                ))),
            }
        }
        UserCommand::RunMcpPrompt {
            server,
            name,
            arguments,
        } => match render_mcp_prompt(agent, &server, &name, arguments).await {
            // A prompt is a *starting message*, so it is submitted as one: the
            // ordinary task path, with the same rendering the `get_mcp_prompt`
            // tool returns. Nothing downstream learns a new turn shape.
            Ok(messages) => {
                Box::pin(handle_user_command_with_account(
                    agent,
                    UserCommand::SubmitTask(messages),
                    image_work_dir,
                    account_tx,
                ))
                .await;
            }
            Err(message) => {
                agent.emit_update(runtime_event::error(AgentErrorKind::Other(message)));
            }
        },
        UserCommand::HooksList => match tact_extensions::plugin::survey_hooks(image_work_dir) {
            // The same wording `tact-ui hooks list` uses: two surfaces naming
            // the same hooks must not describe them differently.
            Ok(report) => agent.emit_update(runtime_event::md_info(
                crate::hooks_cli::render_hooks_listing(&report),
            )),
            Err(error) => agent.emit_update(runtime_event::error(AgentErrorKind::Other(format!(
                "Hooks list failed: {error:#}"
            )))),
        },
        UserCommand::HooksTrust { all, source } => {
            match tact_extensions::plugin::trust_hooks(image_work_dir, all, source.as_deref()) {
                Ok(approved) if approved.is_empty() => agent.emit_update(runtime_event::info(
                    "Nothing to approve: every configured hook has already been reviewed."
                        .to_string(),
                )),
                Ok(approved) => {
                    let mut lines = vec![format!("Approved {} hook(s):", approved.len())];
                    lines.extend(approved.iter().map(|hook| format!("  {}", hook.describe())));
                    // Hooks are registered when the agent is built, so an
                    // approval that only takes effect next session must say so.
                    lines.push(
                        "They run from the next session onward. Revoke with /hooks forget --all."
                            .to_string(),
                    );
                    agent.emit_update(runtime_event::info(lines.join("\n")));
                }
                Err(error) => agent.emit_update(runtime_event::error(AgentErrorKind::Other(
                    format!("Hooks trust failed: {error:#}"),
                ))),
            }
        }
        UserCommand::HooksForget => match tact_extensions::plugin::forget_hook_trust() {
            Ok(()) => agent.emit_update(runtime_event::info(
                "Forgot every hook approval. No hook runs until it is reviewed again with \
                 /hooks trust --all."
                    .to_string(),
            )),
            Err(error) => agent.emit_update(runtime_event::error(AgentErrorKind::Other(format!(
                "Hooks forget failed: {error:#}"
            )))),
        },
        _ => {}
    }
}

/// Awaits `auth` while forwarding its progress lines to `emit` as they arrive.
///
/// The authorization URL is the one line that matters, and
/// `authorize_server` produces it *before* blocking on the loopback callback —
/// waiting for the future to resolve before showing it would hide the URL for
/// the whole browser round-trip, and for good if the user never authorizes.
///
/// Any lines still queued when the flow finishes are drained afterwards, so a
/// URL delivered in the same poll that completes the flow is never dropped.
async fn stream_auth_progress<F>(
    auth: F,
    mut line_rx: UnboundedReceiver<String>,
    mut emit: impl FnMut(String),
) -> F::Output
where
    F: std::future::Future,
{
    tokio::pin!(auth);
    let output = loop {
        tokio::select! {
            output = &mut auth => break output,
            Some(line) = line_rx.recv() => emit(line),
        }
    };
    while let Ok(line) = line_rx.try_recv() {
        emit(line);
    }
    output
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;

    use tact_llm::{ContentBlock, MockClient, StopReason};
    use tact_protocol::{RunId, RuntimeCommand, RuntimeEvent};
    use tact_view::UserCommand;

    use crate::test_support::{build_test_agent, install_test_config};

    fn text_block(content: &str) -> ContentBlock {
        ContentBlock::Text {
            text: content.to_string(),
        }
    }

    /// A permission policy that records every capability name the Router checks
    /// before delegating to the session's real policy.
    ///
    /// `CapabilityRouter::invoke` always runs the check, so a name appearing
    /// here is proof the capability was invoked *through the Router* — not that
    /// some direct path happened to do the same work.
    struct RecordingPermission {
        inner: std::sync::Arc<dyn tact::PermissionService>,
        seen: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    }

    #[async_trait::async_trait]
    impl tact::PermissionService for RecordingPermission {
        async fn check(
            &self,
            declaration: &tact_protocol::CapabilityDeclaration,
            context: &tact::InvocationContext,
            input: &serde_json::Value,
        ) -> Result<(), tact::KernelError> {
            self.seen
                .lock()
                .expect("recording lock")
                .push(declaration.name.clone());
            self.inner.check(declaration, context, input).await
        }
    }

    /// The serving context `bootstrap_session` builds, with a recording policy
    /// in front of the session's real one, plus the recorder of everything the
    /// Router checks. Mirrors the headless test's construction so the two hosts
    /// are exercised the same way.
    async fn serving_context_with_recorder(
        directory: &std::path::Path,
        seen: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    ) -> tact::RuntimeContext {
        let db_path = directory.join("session.db");
        let store = tact_extensions::store::open_sqlite_session_store(&db_path)
            .await
            .expect("session store");
        let transport = tact::EventTransport::new(8);
        let settings = tact_extensions::permission::settings::PermissionSettings::load_from(
            &directory.join("settings.json"),
            None,
        );
        let policy = crate::permission::serving_permission(
            tact_extensions::permission::PermissionMode::Auto,
            settings,
        )
        .expect("serving policy");
        let recording: std::sync::Arc<dyn tact::PermissionService> =
            std::sync::Arc::new(RecordingPermission {
                inner: policy,
                seen,
            });
        crate::session_bootstrap::build_serving_context(
            &db_path,
            &store,
            &transport,
            std::sync::Arc::new(tact_trajectory::KernelTrajectoryRecorder::default()),
            recording,
            &crate::session_bootstrap::Notices::Stderr,
        )
        .await
    }

    /// The interactive host submits its turn through the Chat extension's
    /// `chat.submit` — the extension owns the turn — and the turn's run still
    /// goes through the session's serving router (`runs.start`). The recording
    /// policy proves both invocations reached the Router; the completed turn
    /// proves the routed chain actually ran on the shared Agent. A driver test
    /// can see this; it is the entry point the `chat.submit` handler cannot
    /// observe from the inside.
    #[tokio::test]
    async fn interactive_submit_goes_through_chat_submit_when_a_serving_context_is_present() {
        install_test_config();
        let mock = MockClient::new(vec![(
            vec![text_block("chatted answer")],
            Some(StopReason::EndTurn),
        )]);
        let (agent_tx, mut agent_rx) = tokio::sync::mpsc::unbounded_channel();
        let (agent, work_dir) = build_test_agent(mock, Some(agent_tx));

        let directory = tempfile::tempdir().expect("temp directory");
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let serving = serving_context_with_recorder(directory.path(), seen.clone()).await;
        let agent = agent.with_serving_context(serving);

        let (command_tx, command_rx) = crate::test_support::user_command_channels();
        let driver = tokio::spawn(super::run_command_loop(agent, command_rx, work_dir));

        command_tx
            .send(UserCommand::SubmitTask("hello".into()))
            .unwrap();

        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                match agent_rx.recv().await {
                    Some(RuntimeEvent::TaskComplete { .. }) => break,
                    Some(_) => continue,
                    None => panic!("agent event channel closed before completion"),
                }
            }
        })
        .await
        .expect("the submitted turn must complete");

        drop(command_tx);
        tokio::time::timeout(std::time::Duration::from_secs(2), driver)
            .await
            .expect("driver did not shut down")
            .unwrap();

        let seen = seen.lock().expect("recording lock");
        assert!(
            seen.iter().any(|name| name == "chat.submit"),
            "the interactive turn must be submitted through the Chat extension: {seen:?}"
        );
    }

    /// The turn `chat.submit` starts still runs through `runs.start` — Chat owns
    /// the turn, the Agent extension owns the run. The recording policy proves
    /// the run was invoked *through the Router*, and the completed turn proves
    /// that routed call actually ran on the shared Agent.
    #[tokio::test]
    async fn interactive_submit_goes_through_runs_start_when_a_serving_context_is_present() {
        install_test_config();
        let mock = MockClient::new(vec![(
            vec![text_block("routed answer")],
            Some(StopReason::EndTurn),
        )]);
        let (agent_tx, mut agent_rx) = tokio::sync::mpsc::unbounded_channel();
        let (agent, work_dir) = build_test_agent(mock, Some(agent_tx));

        let directory = tempfile::tempdir().expect("temp directory");
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let serving = serving_context_with_recorder(directory.path(), seen.clone()).await;
        let agent = agent.with_serving_context(serving);

        let (command_tx, command_rx) = crate::test_support::user_command_channels();
        let driver = tokio::spawn(super::run_command_loop(agent, command_rx, work_dir));

        command_tx
            .send(UserCommand::SubmitTask("hello".into()))
            .unwrap();

        // Wait for the turn to complete, so the assertion below cannot race the
        // Router invocation.
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                match agent_rx.recv().await {
                    Some(RuntimeEvent::TaskComplete { .. }) => break,
                    Some(_) => continue,
                    None => panic!("agent event channel closed before completion"),
                }
            }
        })
        .await
        .expect("the routed turn must complete");

        drop(command_tx);
        tokio::time::timeout(std::time::Duration::from_secs(2), driver)
            .await
            .expect("driver did not shut down")
            .unwrap();

        let seen = seen.lock().expect("recording lock");
        assert!(
            seen.iter().any(|name| name == "runs.start"),
            "the interactive run must be invoked through the Router: {seen:?}"
        );
    }

    /// A `/mcp prompt` turn reaches `runs.start` too: the new arm renders the
    /// prompt (a *starting message*) and submits it through the same
    /// `run_submit` helper as `SubmitTask`, so a plugin-prompt run is routed
    /// exactly like an ordinary one when a serving context is present. The
    /// recording policy proves the invocation went through the Router, and the
    /// completed turn proves the routed call actually ran.
    #[tokio::test]
    async fn interactive_mcp_prompt_goes_through_runs_start_when_a_serving_context_is_present() {
        install_test_config();
        let mock = MockClient::new(vec![(
            vec![text_block("routed prompt answer")],
            Some(StopReason::EndTurn),
        )]);
        let (agent_tx, mut agent_rx) = tokio::sync::mpsc::unbounded_channel();
        let (mut agent, work_dir) = build_test_agent(mock, Some(agent_tx));

        // One mock MCP server publishing a prompt, so the render succeeds and
        // the arm proceeds to submit the composed message instead of erroring.
        let service = tact_extensions::mcp::MockMcpService::new(Vec::new(), |_| {
            Ok(rmcp::model::CallToolResult::success(Vec::new()))
        })
        .with_prompt("getting_started", "Introduce the project", &[])
        .with_prompt_messages(
            "getting_started",
            vec![rmcp::model::PromptMessage::new_text(
                rmcp::model::PromptMessageRole::User,
                "Show me around.",
            )],
        );
        agent
            .mcp_router
            .register_client(tact_extensions::mcp::McpClient::with_service(
                "basic-memory",
                Vec::new(),
                std::sync::Arc::new(service),
            ));

        let directory = tempfile::tempdir().expect("temp directory");
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let serving = serving_context_with_recorder(directory.path(), seen.clone()).await;
        let agent = agent.with_serving_context(serving);

        let (command_tx, command_rx) = crate::test_support::user_command_channels();
        let driver = tokio::spawn(super::run_command_loop(agent, command_rx, work_dir));

        command_tx
            .send(UserCommand::RunMcpPrompt {
                server: "basic-memory".into(),
                name: "getting_started".into(),
                arguments: std::collections::BTreeMap::new(),
            })
            .unwrap();

        // Wait for the turn to complete, so the assertion below cannot race the
        // Router invocation.
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                match agent_rx.recv().await {
                    Some(RuntimeEvent::TaskComplete { .. }) => break,
                    Some(_) => continue,
                    None => panic!("agent event channel closed before completion"),
                }
            }
        })
        .await
        .expect("the routed MCP-prompt turn must complete");

        drop(command_tx);
        tokio::time::timeout(std::time::Duration::from_secs(2), driver)
            .await
            .expect("driver did not shut down")
            .unwrap();

        let seen = seen.lock().expect("recording lock");
        assert!(
            seen.iter().any(|name| name == "runs.start"),
            "the MCP-prompt run must be invoked through the Router: {seen:?}"
        );
    }

    /// A run that fails through the Router shows the same message the direct
    /// path would: the router's `KernelError` carries `agent_loop`'s own error
    /// text, and the driver maps it back with `message()`.
    #[tokio::test]
    async fn routed_run_failure_shows_the_agent_error_text() {
        install_test_config();
        let mock = MockClient::with_error(vec![tact_llm::LlmError::Mock(
            "routed failure text".to_string(),
        )]);
        let (agent_tx, mut agent_rx) = tokio::sync::mpsc::unbounded_channel();
        let (agent, work_dir) = build_test_agent(mock, Some(agent_tx));

        let directory = tempfile::tempdir().expect("temp directory");
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let serving = serving_context_with_recorder(directory.path(), seen.clone()).await;
        let agent = agent.with_serving_context(serving);

        let (command_tx, command_rx) = crate::test_support::user_command_channels();
        let driver = tokio::spawn(super::run_command_loop(agent, command_rx, work_dir));

        command_tx
            .send(UserCommand::SubmitTask("hello".into()))
            .unwrap();

        // Collect every Error the failed turn emits: exactly one is the
        // invariant. The turn reports the failure, and the host must not add a
        // second row for the same failed submit.
        let mut errors: Vec<String> = Vec::new();
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while let Some(update) = agent_rx.recv().await {
                if let RuntimeEvent::Error { message: err, .. } = update {
                    errors.push(err.to_string());
                    break;
                }
            }
        })
        .await
        .expect("the routed failure must surface an Error event");
        // Give a wrongly-emitted second report time to arrive before teardown.
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        drop(command_tx);
        let _ = tokio::time::timeout(std::time::Duration::from_secs(2), driver).await;
        while let Ok(update) = agent_rx.try_recv() {
            if let RuntimeEvent::Error { message: err, .. } = update {
                errors.push(err.to_string());
            }
        }

        let message = errors
            .first()
            .expect("a failed routed run must emit an Error")
            .clone();
        assert!(
            message.contains("routed failure text"),
            "the TUI must show the Agent's own error text, got: {message}"
        );
        assert_eq!(
            errors.len(),
            1,
            "one failed turn must not be reported twice: {errors:?}"
        );
        assert!(
            seen.lock()
                .expect("recording lock")
                .iter()
                .any(|name| name == "runs.start"),
            "the failing run must still have gone through the Router"
        );
    }

    /// The same run-identity guarantee as the direct path, but through the
    /// Router: a `Runtime(StartRun)` id is set on the Agent before `runs.start`
    /// is invoked, so `RunStarted` carries exactly the requested id.
    #[tokio::test]
    async fn routed_start_command_preserves_the_requested_run_id() {
        install_test_config();
        let mock = MockClient::new(vec![(vec![text_block("done")], Some(StopReason::EndTurn))]);
        let (agent_tx, _agent_rx) = tokio::sync::mpsc::unbounded_channel();
        let (agent, work_dir) = build_test_agent(mock, Some(agent_tx));
        let transport = tact::EventTransport::new(8);
        let mut events = transport.subscribe();
        let agent = agent.with_runtime_event_transport(transport);

        let directory = tempfile::tempdir().expect("temp directory");
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let serving = serving_context_with_recorder(directory.path(), seen.clone()).await;
        let agent = agent.with_serving_context(serving);

        let (command_tx, command_rx) = crate::test_support::user_command_channels();
        let driver = tokio::spawn(super::run_command_loop(agent, command_rx, work_dir));
        let run_id = RunId::from("run-routed-start");

        command_tx
            .send(UserCommand::Runtime(RuntimeCommand::StartRun {
                run_id: run_id.clone(),
                input: serde_json::json!({"message": "go"}),
            }))
            .unwrap();

        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                match events.recv().await {
                    Ok(RuntimeEvent::RunStarted { run_id: actual }) => {
                        assert_eq!(
                            actual, run_id,
                            "the requested run id must survive the Router"
                        );
                        return;
                    }
                    Ok(_) => continue,
                    Err(error) => panic!("event stream closed before RunStarted: {error}"),
                }
            }
        })
        .await
        .expect("the routed run must start");

        drop(command_tx);
        let _ = tokio::time::timeout(std::time::Duration::from_secs(2), driver).await;

        assert!(
            seen.lock()
                .expect("recording lock")
                .iter()
                .any(|name| name == "runs.start"),
            "the run must have gone through the Router"
        );
    }

    #[tokio::test]
    async fn cancel_sets_flag_and_emits_info() {
        install_test_config();
        let (agent_tx, mut agent_rx) = tokio::sync::mpsc::unbounded_channel();
        let (agent, _) = build_test_agent(MockClient::new(vec![]), Some(agent_tx));

        agent.runtime.cancel_flag.store(true, Ordering::Relaxed);
        agent.emit_update(tact_extensions::runtime_event::info("Cancelling..."));

        assert!(agent.runtime.cancel_flag.load(Ordering::Relaxed));
        let update = agent_rx.try_recv().expect("expected Cancelling info");
        assert!(
            matches!(update, RuntimeEvent::Info { content: msg, .. } if msg.contains("Cancelling"))
        );
    }

    #[tokio::test]
    async fn runtime_cancel_command_cancels_its_active_run() {
        install_test_config();
        let (agent_tx, _agent_rx) = tokio::sync::mpsc::unbounded_channel();
        let (agent, work_dir) = build_test_agent(MockClient::new(vec![]), Some(agent_tx));
        let cancel_flag = agent.runtime.cancel_flag.clone();
        let (command_tx, command_rx) = crate::test_support::user_command_channels();
        let driver = tokio::spawn(super::run_command_loop(agent, command_rx, work_dir));

        command_tx
            .send(UserCommand::Runtime(RuntimeCommand::CancelRun {
                run_id: RunId::from("run-command-cancel"),
            }))
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_millis(200), async {
            while !cancel_flag.load(Ordering::Relaxed) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("Runtime cancel command was not applied");

        drop(command_tx);
        tokio::time::timeout(std::time::Duration::from_secs(1), driver)
            .await
            .expect("driver did not shut down")
            .unwrap();
    }

    #[tokio::test]
    async fn runtime_interaction_response_reaches_the_pending_waiter() {
        install_test_config();
        let (agent_tx, _agent_rx) = tokio::sync::mpsc::unbounded_channel();
        let (agent, work_dir) = build_test_agent(MockClient::new(vec![]), Some(agent_tx));
        let responder = agent.tool_context.ui_responder.clone();
        let (request_id, waiter) = responder.register_select(
            "Permission".into(),
            vec!["Allow".into(), "Deny".into()],
            false,
        );
        let (command_tx, command_rx) = crate::test_support::user_command_channels();
        let driver = tokio::spawn(super::run_command_loop(agent, command_rx, work_dir));

        command_tx
            .send(UserCommand::Runtime(RuntimeCommand::RespondInteraction {
                response: tact_protocol::InteractionResponse::Selected {
                    request_id: tact_protocol::RequestId::from(request_id.to_string()),
                    values: vec!["Deny".into()],
                },
            }))
            .unwrap();
        assert!(matches!(
            tokio::time::timeout(std::time::Duration::from_millis(200), waiter)
                .await
                .expect("protocol response timed out")
                .unwrap(),
            tact_protocol::InteractionResponse::Selected { request_id: id, values }
                if id.as_str() == request_id.to_string() && values == vec!["Deny".to_string()]
        ));

        drop(command_tx);
        tokio::time::timeout(std::time::Duration::from_secs(1), driver)
            .await
            .expect("driver did not shut down")
            .unwrap();
    }

    #[tokio::test]
    async fn runtime_start_command_preserves_the_requested_run_id() {
        install_test_config();
        let mock = MockClient::new(vec![(vec![text_block("done")], Some(StopReason::EndTurn))]);
        let (agent_tx, _agent_rx) = tokio::sync::mpsc::unbounded_channel();
        let (agent, work_dir) = build_test_agent(mock, Some(agent_tx));
        let transport = tact::EventTransport::new(8);
        let mut events = transport.subscribe();
        let agent = agent.with_runtime_event_transport(transport);
        let (command_tx, command_rx) = crate::test_support::user_command_channels();
        let driver = tokio::spawn(super::run_command_loop(agent, command_rx, work_dir));
        let run_id = RunId::from("run-start-command");

        command_tx
            .send(UserCommand::Runtime(RuntimeCommand::StartRun {
                run_id: run_id.clone(),
                input: serde_json::json!({"message": "go"}),
            }))
            .unwrap();
        let event = tokio::time::timeout(std::time::Duration::from_secs(1), events.recv())
            .await
            .expect("Runtime run event timed out")
            .unwrap();
        assert!(matches!(
            event,
            tact_protocol::RuntimeEvent::RunStarted { run_id: actual } if actual == run_id
        ));

        drop(command_tx);
        tokio::time::timeout(std::time::Duration::from_secs(2), driver)
            .await
            .expect("driver did not shut down")
            .unwrap();
    }

    #[tokio::test]
    async fn submit_clears_cancel_flag_on_new_task() {
        install_test_config();
        let mock = MockClient::new(vec![(vec![text_block("done")], Some(StopReason::EndTurn))]);
        let (agent_tx, mut agent_rx) = tokio::sync::mpsc::unbounded_channel();
        let (mut agent, work_dir) = build_test_agent(mock, Some(agent_tx));

        agent.runtime.cancel_flag.store(true, Ordering::Relaxed);
        super::handle_user_command(&mut agent, UserCommand::SubmitTask("go".into()), &work_dir)
            .await;

        assert!(!agent.runtime.cancel_flag.load(Ordering::Relaxed));
        let mut saw_complete = false;
        while let Ok(update) = agent_rx.try_recv() {
            if matches!(update, RuntimeEvent::TaskComplete { .. }) {
                saw_complete = true;
            }
        }
        assert!(saw_complete, "SubmitTask should clear cancel and complete");
    }

    #[tokio::test]
    async fn running_an_mcp_prompt_names_a_missing_prompt_name() {
        // The TUI forwards an incomplete `/mcp prompt` on purpose (see the
        // spec's note on arity); naming the missing piece is the driver's job,
        // because only the driver can also say "no such prompt".
        install_test_config();
        let (agent, _work_dir) = build_test_agent(MockClient::new(vec![]), None);

        let error = super::render_mcp_prompt(&agent, "bm", "", std::collections::BTreeMap::new())
            .await
            .expect_err("an empty name cannot be fetched");
        assert!(error.contains("needs a prompt name"), "{error}");
    }

    #[tokio::test]
    async fn running_an_mcp_prompt_on_an_unknown_server_names_it() {
        // An empty router is the honest case here: the fetch must not look like
        // a prompt that composed nothing.
        install_test_config();
        let (agent, _work_dir) = build_test_agent(MockClient::new(vec![]), None);

        let error = super::render_mcp_prompt(
            &agent,
            "nope",
            "getting_started",
            std::collections::BTreeMap::new(),
        )
        .await
        .expect_err("an unknown server is an error, not an empty prompt");
        assert!(error.contains("nope"), "{error}");
        assert!(error.contains("getting_started"), "{error}");
    }

    #[tokio::test]
    async fn set_thinking_budget_changes_the_next_request() {
        install_test_config();
        let mock = MockClient::with_responder(|request, _| {
            assert_eq!(
                request
                    .thinking
                    .as_ref()
                    .map(|thinking| thinking.budget_tokens),
                Some(64_000)
            );
            Ok((vec![text_block("done")], Some(StopReason::EndTurn), None))
        });
        let (mut agent, work_dir) = build_test_agent(mock, None);

        super::handle_user_command(
            &mut agent,
            UserCommand::SetThinkingBudget(64_000),
            &work_dir,
        )
        .await;
        super::handle_user_command(
            &mut agent,
            UserCommand::SubmitTask("use the new budget".into()),
            &work_dir,
        )
        .await;
    }

    #[tokio::test]
    async fn set_thinking_budget_emits_model_info_for_status_bar() {
        install_test_config();
        let (agent_tx, mut agent_rx) = tokio::sync::mpsc::unbounded_channel();
        let (mut agent, work_dir) = build_test_agent(MockClient::new(vec![]), Some(agent_tx));

        super::handle_user_command(
            &mut agent,
            UserCommand::SetThinkingBudget(32_000),
            &work_dir,
        )
        .await;

        let mut saw_model_info = false;
        while let Ok(update) = agent_rx.try_recv() {
            if let RuntimeEvent::ModelInfo { params, .. } = update {
                assert_eq!(params.thinking_budget, Some(32_000));
                assert!(params.max_tokens > 32_000);
                saw_model_info = true;
            }
        }
        assert!(
            saw_model_info,
            "SetThinkingBudget must emit ModelInfo so the TUI bar resyncs"
        );
    }

    #[tokio::test]
    async fn set_reasoning_effort_clears_stale_thinking_budget() {
        // Regression: switching an effort-semantic model (openai / deepseek /
        // kimi k3) must not leave a stale thinking budget behind — the bottom
        // bar would otherwise render a meaningless `think high(32K)`.
        install_test_config();
        let (agent_tx, mut agent_rx) = tokio::sync::mpsc::unbounded_channel();
        let (mut agent, work_dir) = build_test_agent(MockClient::new(vec![]), Some(agent_tx));

        super::handle_user_command(
            &mut agent,
            UserCommand::SetThinkingBudget(32_000),
            &work_dir,
        )
        .await;
        super::handle_user_command(
            &mut agent,
            UserCommand::SetReasoningEffort(Some("high".to_string())),
            &work_dir,
        )
        .await;

        let mut last: Option<tact_protocol::ModelCallParams> = None;
        while let Ok(update) = agent_rx.try_recv() {
            if let RuntimeEvent::ModelInfo { params, .. } = update {
                last = Some(params);
            }
        }
        let last = last.expect("SetReasoningEffort must emit ModelInfo so the TUI bar resyncs");
        assert_eq!(last.reasoning_effort, Some("high".to_string()));
        assert_eq!(
            last.thinking_budget, None,
            "effort pick must clear stale thinking budget"
        );
    }

    #[tokio::test]
    async fn query_background_emits_a_popup_listing_when_no_tasks() {
        install_test_config();
        let (agent_tx, mut agent_rx) = tokio::sync::mpsc::unbounded_channel();
        let (mut agent, work_dir) = build_test_agent(MockClient::new(vec![]), Some(agent_tx));

        super::handle_user_command(&mut agent, UserCommand::QueryBackground(None), &work_dir).await;

        let mut popup = None;
        while let Ok(update) = agent_rx.try_recv() {
            match update {
                RuntimeEvent::PopupMarkdown { title, source, .. } => {
                    popup = Some((title, source));
                }
                // The listing is a read-out: it must not land in the transcript.
                RuntimeEvent::MdInfo { content: md, .. } => {
                    panic!("background listing must not be MdInfo: {md}")
                }
                _ => {}
            }
        }
        let (title, source) = popup.expect("QueryBackground must emit PopupMarkdown");
        assert_eq!(title, "⚙️ Background Tasks");
        assert!(source.contains("No background tasks."), "source: {source}");
    }

    #[tokio::test]
    async fn query_background_unknown_id_emits_error() {
        install_test_config();
        let (agent_tx, mut agent_rx) = tokio::sync::mpsc::unbounded_channel();
        let (mut agent, work_dir) = build_test_agent(MockClient::new(vec![]), Some(agent_tx));

        super::handle_user_command(
            &mut agent,
            UserCommand::QueryBackground(Some("deadbeef".into())),
            &work_dir,
        )
        .await;

        let mut saw_error = false;
        while let Ok(update) = agent_rx.try_recv() {
            if let RuntimeEvent::Error { message: err, .. } = update {
                assert!(
                    err.to_string().contains("Unknown background task"),
                    "err: {err}"
                );
                saw_error = true;
            }
        }
        assert!(saw_error, "QueryBackground with unknown id must emit Error");
    }

    #[tokio::test]
    async fn mcp_list_emits_the_live_listing_without_reconnecting() {
        install_test_config();
        let (agent_tx, mut agent_rx) = tokio::sync::mpsc::unbounded_channel();
        let (mut agent, work_dir) = build_test_agent(MockClient::new(vec![]), Some(agent_tx));

        super::handle_user_command(&mut agent, UserCommand::McpList, &work_dir).await;

        let mut saw_md = false;
        while let Ok(update) = agent_rx.try_recv() {
            if let RuntimeEvent::MdInfo { content: md, .. } = update {
                assert!(md.contains("MCP Servers"), "md: {md}");
                saw_md = true;
            }
        }
        assert!(saw_md, "McpList must emit MdInfo with the server listing");
    }

    #[tokio::test]
    async fn hooks_list_emits_the_review_listing() {
        install_test_config();
        let (agent_tx, mut agent_rx) = tokio::sync::mpsc::unbounded_channel();
        let (mut agent, work_dir) = build_test_agent(MockClient::new(vec![]), Some(agent_tx));

        super::handle_user_command(&mut agent, UserCommand::HooksList, &work_dir).await;

        let mut rendered = None;
        while let Ok(update) = agent_rx.try_recv() {
            if let RuntimeEvent::MdInfo { content: md, .. } = update {
                rendered = Some(md);
            }
        }
        // Both wordings come from `hooks_cli::render_hooks_listing`, so the
        // assertion holds whether or not this machine has hooks configured —
        // what it pins is that `/hooks list` reaches the loader and is shown,
        // rather than being silently dropped by the driver.
        let text = rendered.expect("HooksList must emit MdInfo");
        assert!(
            text.contains("hook(s) configured") || text.contains("No command hooks configured"),
            "unexpected listing: {text}"
        );
    }

    #[tokio::test]
    async fn hooks_trust_without_a_selector_reports_the_refusal() {
        // `trust_hooks` bails *before* touching the review store, so this is the
        // one trust path a test can exercise without writing the developer's
        // real `~/.tact/hooks-state.json`.
        install_test_config();
        let (agent_tx, mut agent_rx) = tokio::sync::mpsc::unbounded_channel();
        let (mut agent, work_dir) = build_test_agent(MockClient::new(vec![]), Some(agent_tx));

        super::handle_user_command(
            &mut agent,
            UserCommand::HooksTrust {
                all: false,
                source: None,
            },
            &work_dir,
        )
        .await;

        let mut message = None;
        while let Ok(update) = agent_rx.try_recv() {
            if let RuntimeEvent::Error { message: kind, .. } = update {
                message = Some(kind.to_string());
            }
        }
        let message = message.expect("a refusal must be reported, not swallowed");
        assert!(message.contains("Hooks trust failed"), "{message}");
        assert!(message.contains("--source"), "{message}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn subagent_finished_notification_is_not_lost_when_parent_finishes() {
        use std::sync::atomic::AtomicUsize;
        use std::time::Duration;

        install_test_config();
        let release = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let release_rx = release.clone();
        let calls = std::sync::Arc::new(AtomicUsize::new(0));
        let calls_rx = calls.clone();
        let mock = MockClient::with_responder(move |_request, idx| {
            calls_rx.fetch_add(1, Ordering::Relaxed);
            if idx == 0 {
                while !release_rx.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(5));
                }
            }
            let text = if idx == 0 { "parent done" } else { "wake done" };
            Ok((vec![text_block(text)], Some(StopReason::EndTurn), None))
        });
        let (agent_tx, mut agent_rx) = tokio::sync::mpsc::unbounded_channel();
        let (agent, work_dir) = build_test_agent(mock, Some(agent_tx));
        // Share the parent's result queue so the test can enqueue the child's
        // summary exactly as the real async child does.
        let pending = agent.runtime.pending_subagent_results.clone();
        let (user_cmd_tx, user_cmd_rx) = tokio::sync::mpsc::unbounded_channel();
        let loop_handle = tokio::spawn(super::run_command_loop(agent, user_cmd_rx, work_dir));

        user_cmd_tx
            .send(UserCommand::SubmitTask("parent task".into()))
            .unwrap();
        tokio::time::timeout(Duration::from_secs(1), async {
            while calls.load(Ordering::Relaxed) == 0 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("parent task should start");

        // The child enqueues its summary before emitting the notification, so
        // the wake-up turn's drain is guaranteed to see it.
        pending
            .lock()
            .unwrap()
            .push_back(tact_extensions::subagent::SubagentResult {
                child_id: "child-1".into(),
                summary: "finished".into(),
                success: true,
            });
        // The notification arrives while the parent turn is still running.
        user_cmd_tx
            .send(UserCommand::SubagentFinishedNotification {
                child_id: "child-1".into(),
                summary: "finished".into(),
                success: true,
            })
            .unwrap();
        release.store(true, Ordering::Relaxed);

        let mut completions = 0;
        let wait_result = tokio::time::timeout(Duration::from_secs(1), async {
            while completions < 2 {
                if let Some(RuntimeEvent::TaskComplete { .. }) = agent_rx.recv().await {
                    completions += 1;
                }
            }
        })
        .await;
        drop(user_cmd_tx);
        let _ = loop_handle.await;

        wait_result.expect("queued wake-up should run after the parent finishes");
        assert_eq!(completions, 2);
    }

    /// A notification whose result an earlier turn already drained must not
    /// spawn an empty wake-up turn: the summary is already in the parent's
    /// context, so the extra turn would have nothing to review.
    #[tokio::test(flavor = "multi_thread")]
    async fn subagent_notification_with_empty_queue_does_not_wake_parent() {
        use std::sync::atomic::AtomicUsize;
        use std::time::Duration;

        install_test_config();
        let release = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let release_rx = release.clone();
        let calls = std::sync::Arc::new(AtomicUsize::new(0));
        let calls_rx = calls.clone();
        let mock = MockClient::with_responder(move |_request, _idx| {
            calls_rx.fetch_add(1, Ordering::Relaxed);
            while !release_rx.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(5));
            }
            Ok((
                vec![text_block("parent done")],
                Some(StopReason::EndTurn),
                None,
            ))
        });
        let (agent_tx, mut agent_rx) = tokio::sync::mpsc::unbounded_channel();
        let (agent, work_dir) = build_test_agent(mock, Some(agent_tx));
        let (user_cmd_tx, user_cmd_rx) = tokio::sync::mpsc::unbounded_channel();
        let loop_handle = tokio::spawn(super::run_command_loop(agent, user_cmd_rx, work_dir));

        user_cmd_tx
            .send(UserCommand::SubmitTask("parent task".into()))
            .unwrap();
        tokio::time::timeout(Duration::from_secs(1), async {
            while calls.load(Ordering::Relaxed) == 0 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("parent task should start");

        // The queue is deliberately left empty: this models a turn that already
        // drained and injected the summary.
        user_cmd_tx
            .send(UserCommand::SubagentFinishedNotification {
                child_id: "child-1".into(),
                summary: "already delivered".into(),
                success: true,
            })
            .unwrap();
        release.store(true, Ordering::Relaxed);

        // Give a wrongly-spawned wake-up turn time to reach the client.
        tokio::time::sleep(Duration::from_millis(300)).await;
        drop(user_cmd_tx);
        let _ = loop_handle.await;

        let mut completions = 0;
        while let Ok(update) = agent_rx.try_recv() {
            if let RuntimeEvent::TaskComplete { .. } = update {
                completions += 1;
            }
        }
        assert_eq!(
            calls.load(Ordering::Relaxed),
            1,
            "an empty queue must not trigger a wake-up request"
        );
        assert_eq!(completions, 1, "only the parent turn should complete");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn query_stats_responds_immediately_while_task_runs() {
        use std::time::Duration;

        install_test_config();
        let (agent_tx, mut agent_rx) = tokio::sync::mpsc::unbounded_channel();
        // The responder spins on an AtomicBool until the test releases it — a
        // deterministic "long running LLM call". A serialized QueryStats
        // (awaiting the task) would hang until the release fires.
        let release = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let release_rx = release.clone();
        let mock = MockClient::with_responder(move |_request, _| {
            while !release_rx.load(std::sync::atomic::Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(10));
            }
            Ok((vec![text_block("done")], Some(StopReason::EndTurn), None))
        });
        let (agent, work_dir) = build_test_agent(mock, Some(agent_tx));
        let (user_cmd_tx, user_cmd_rx) = tokio::sync::mpsc::unbounded_channel();

        let loop_handle = tokio::spawn(super::run_command_loop(agent, user_cmd_rx, work_dir));

        // Start a task that is now stuck in the responder, then ask for stats.
        user_cmd_tx
            .send(UserCommand::SubmitTask("long task".into()))
            .unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        let start = std::time::Instant::now();
        user_cmd_tx.send(UserCommand::QueryStats).unwrap();

        // The stats snapshot must arrive while the task is still blocked.
        let mut saw_stats = false;
        loop {
            match tokio::time::timeout(Duration::from_millis(300), agent_rx.recv()).await {
                Ok(Some(RuntimeEvent::PopupMarkdown { .. })) => {
                    saw_stats = true;
                    break;
                }
                Ok(Some(_)) => continue,
                Ok(None) | Err(_) => break,
            }
        }
        assert!(
            saw_stats,
            "expected the stats popup while the task is still running"
        );
        assert!(
            start.elapsed() < Duration::from_millis(450),
            "QueryStats must NOT await the in-flight task"
        );

        // Release the blocked task and let the loop drain.
        release.store(true, std::sync::atomic::Ordering::Relaxed);
        drop(user_cmd_tx);
        let _ = tokio::time::timeout(Duration::from_secs(5), loop_handle)
            .await
            .expect("command loop must finish");
    }

    /// `/background` is answered from the shared manager, so the popup opens
    /// while a turn still owns the Agent — the same guarantee `/stats` has.
    #[tokio::test(flavor = "multi_thread")]
    async fn query_background_responds_immediately_while_task_runs() {
        use std::time::Duration;

        install_test_config();
        let (agent_tx, mut agent_rx) = tokio::sync::mpsc::unbounded_channel();
        // Deterministic "long running LLM call": the responder spins until the
        // test releases it, but never longer than `spin_cap` — a failed
        // assertion below must not leave a blocked worker behind.
        let release = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let release_rx = release.clone();
        let mock = MockClient::with_responder(move |_request, _| {
            let spin_cap = Duration::from_secs(2);
            let deadline = std::time::Instant::now() + spin_cap;
            while !release_rx.load(std::sync::atomic::Ordering::Relaxed)
                && std::time::Instant::now() < deadline
            {
                std::thread::sleep(Duration::from_millis(10));
            }
            Ok((vec![text_block("done")], Some(StopReason::EndTurn), None))
        });
        let (agent, work_dir) = build_test_agent(mock, Some(agent_tx));
        let (user_cmd_tx, user_cmd_rx) = tokio::sync::mpsc::unbounded_channel();

        let loop_handle = tokio::spawn(super::run_command_loop(agent, user_cmd_rx, work_dir));

        user_cmd_tx
            .send(UserCommand::SubmitTask("long task".into()))
            .unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        let start = std::time::Instant::now();
        user_cmd_tx
            .send(UserCommand::QueryBackground(None))
            .unwrap();

        let mut saw_popup = false;
        loop {
            match tokio::time::timeout(Duration::from_millis(300), agent_rx.recv()).await {
                Ok(Some(RuntimeEvent::PopupMarkdown { title, source, .. })) => {
                    assert_eq!(title, "⚙️ Background Tasks");
                    assert!(source.contains("No background tasks."), "source: {source}");
                    saw_popup = true;
                    break;
                }
                Ok(Some(_)) => continue,
                Ok(None) | Err(_) => break,
            }
        }
        assert!(
            saw_popup,
            "expected the background popup while the task is still running"
        );
        assert!(
            start.elapsed() < Duration::from_millis(450),
            "QueryBackground must NOT await the in-flight task"
        );

        release.store(true, std::sync::atomic::Ordering::Relaxed);
        drop(user_cmd_tx);
        let _ = tokio::time::timeout(Duration::from_secs(5), loop_handle)
            .await
            .expect("command loop must finish");
    }

    /// The URL must reach the user while the flow is *still* waiting for the
    /// browser, which is exactly what the old buffer-then-flush version broke.
    #[tokio::test]
    async fn auth_progress_reaches_the_user_before_the_flow_finishes() {
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        };
        use std::time::Duration;

        use super::stream_auth_progress;
        use tokio::sync::mpsc::unbounded_channel;

        let (line_tx, line_rx) = unbounded_channel::<String>();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
        let finished = Arc::new(AtomicBool::new(false));
        let finished_in_task = Arc::clone(&finished);
        let auth = async move {
            line_tx
                .send("https://example.test/authorize?state=abc".to_string())
                .expect("receiver alive");
            // Block like the real flow does on the loopback callback.
            let _ = release_rx.await;
            finished_in_task.store(true, Ordering::SeqCst);
        };

        let (seen_tx, mut seen_rx) = unbounded_channel::<String>();
        let progress = stream_auth_progress(auth, line_rx, move |line| {
            let _ = seen_tx.send(line);
        });
        tokio::pin!(progress);

        // The line arrives while the flow is still pending.
        let line = tokio::select! {
            line = seen_rx.recv() => line.expect("progress channel open"),
            _ = &mut progress => panic!("flow completed before the URL was released"),
            _ = tokio::time::sleep(Duration::from_millis(500)) => {
                panic!("URL must be emitted without waiting for the callback")
            }
        };
        assert_eq!(line, "https://example.test/authorize?state=abc");
        assert!(
            !finished.load(Ordering::SeqCst),
            "the URL must be emitted before the authorization flow completes"
        );

        release_tx.send(()).expect("auth future alive");
        tokio::time::timeout(Duration::from_millis(500), progress)
            .await
            .expect("flow completes once released");
        assert!(finished.load(Ordering::SeqCst));
    }

    /// A line produced in the same poll that completes the flow must not be
    /// lost to the `select!` race.
    #[tokio::test]
    async fn auth_progress_drains_lines_sent_at_completion() {
        use std::time::Duration;

        use super::stream_auth_progress;
        use tokio::sync::mpsc::unbounded_channel;

        let (line_tx, line_rx) = unbounded_channel::<String>();
        let auth = async move {
            line_tx.send("last".to_string()).expect("receiver alive");
        };

        let (seen_tx, mut seen_rx) = unbounded_channel::<String>();
        tokio::time::timeout(
            Duration::from_millis(500),
            stream_auth_progress(auth, line_rx, move |line| {
                let _ = seen_tx.send(line);
            }),
        )
        .await
        .expect("flow completes");

        assert_eq!(seen_rx.recv().await.as_deref(), Some("last"));
    }
}
