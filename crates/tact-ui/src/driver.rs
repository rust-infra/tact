//! Interactive-mode command driver: bridges `UserCommand` from the TUI to `Agent`.

use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use anyhow::Result;
use chrono::Utc;
use tact::{
    Agent,
    goal::{
        Goal, GoalCreate, GoalCriterion, GoalEvent, GoalEventKind, GoalEventSource,
        GoalEvidenceUpdate, GoalStatus, GoalUpdate, GoalVerification, GoalVerificationKind,
        GoalVerifier, SharedGoalManager, new_event_id,
    },
    store::goal_store::GoalStoreError,
    task::SharedTaskManager,
};
use tact_protocol::{
    AccountUpdate, AgentErrorKind, AgentUpdate, GoalSnapshot, UpdateSink, UserCommand,
};
use tokio::{
    sync::mpsc::{UnboundedReceiver, UnboundedSender},
    task::JoinHandle,
};

use crate::{
    account,
    goal_runner::{GoalLimits, GoalRunner},
    user_message::build_user_message,
};

/// Resolved `[goal]` budgets for one configuration snapshot.
///
/// Takes the snapshot rather than reading the global: every caller already holds
/// an `Agent`, whose bundle records the configuration it was assembled with, so
/// the budgets the loop enforces are the ones that agent actually runs under.
/// `None` (no configuration installed) keeps the built-in defaults, which is the
/// shape a unit test and the non-LLM subcommand paths rely on.
pub(crate) fn goal_limits(config: Option<&tact::config::ResolvedConfig>) -> GoalLimits {
    config
        .map(|config| GoalLimits::from_settings(&config.goal))
        .unwrap_or_default()
}

/// The budgets the agent's own bundle recorded.
pub(crate) fn agent_goal_limits(agent: &Agent) -> GoalLimits {
    let config = agent.config();
    goal_limits(config.as_ref())
}

/// How often the command loop re-checks whether a Goal run lost its holder.
///
/// Startup recovery runs once; this timer covers the rest of the process's
/// life: another session's runner can crash while this loop is up, and a lease
/// whose holder is gone is reclaimable immediately rather than at its TTL. The
/// check costs one transaction that returns nothing when there is no lease, and
/// it stays silent unless it actually changed something.
const GOAL_RECONCILE_INTERVAL: Duration = Duration::from_secs(15);

/// A runner labelled with this session.
///
/// Lease contention must name an owner a human can recognise; `GoalRunner`
/// appends pid + a random suffix so two sessions can never impersonate one
/// another.
fn session_goal_runner(
    goal_manager: &SharedGoalManager,
    limits: GoalLimits,
    session_id: Option<&str>,
    events: &UpdateSink,
) -> GoalRunner {
    let runner = GoalRunner::new(goal_manager.clone(), limits).with_ui(events.ui().cloned());
    match session_id {
        Some(session_id) => runner.with_owner(format!("tact-ui:{session_id}")),
        None => runner,
    }
}

/// Reconcile durable Goal/run state after a restart, once, before the command
/// loop serves anything.
///
/// Recovery is best-effort and never fatal: whatever it could not repair is
/// reported as an informational update so the user can see it, and the loop
/// keeps running.
async fn recover_interrupted_goals(agent: &Agent) {
    let runner = session_goal_runner(
        &agent.tool_context.goal_manager,
        agent_goal_limits(agent),
        agent.runtime.session_id.as_deref(),
        &agent.runtime.events,
    );
    match runner.recover_interrupted().await {
        Ok(recovered) => {
            for run in recovered {
                agent.emit_update(AgentUpdate::Info(format!(
                    "🎯 recovered interrupted run {}; goal {} paused (this session ran the recovery)",
                    run.id, run.goal_id
                )));
            }
        }
        Err(error) => agent.emit_update(AgentUpdate::Info(format!(
            "⚠ Goal recovery failed; goal state may be stale: {error:#}"
        ))),
    }
}

/// The timer's half of Goal recovery.
///
/// Unlike startup recovery it parks **only** the Goals whose run it just
/// interrupted: a Goal that ended its own run as `no_progress` stays `running`
/// by design (Ch 28 §7), and a background timer must never pause that behind
/// the user's back. Failures are reported once per outage, not once per tick.
///
/// Callers on the command loop must go through [`spawn_goal_reconcile`]: a
/// store call can wait on the database's write lock, and the loop has to keep
/// answering `Esc` / `/goal pause` while it does.
async fn reconcile_goal_runs(
    goal_manager: &SharedGoalManager,
    task_manager: &SharedTaskManager,
    limits: GoalLimits,
    events: &UpdateSink,
    session_id: Option<&str>,
    failure_reported: &AtomicBool,
) {
    let runner = session_goal_runner(goal_manager, limits, session_id, events);
    match runner.reconcile_dead_runs().await {
        Ok(recovered) => {
            failure_reported.store(false, Ordering::Relaxed);
            if recovered.is_empty() {
                return;
            }
            // The reclaimed run may belong to *another* session — the lease is
            // workspace-wide, and this timer only knows its own `events`. Naming
            // every run here would hand this user work they never started while
            // staying invisible to the session that is actually waiting, so the
            // log gets one line that says what happened, and the snapshot —
            // which every session shares — carries the per-Goal truth.
            {
                events.send(AgentUpdate::Info(format!(
                    "🎯 Goal reconciliation reclaimed {} interrupted run(s); the Goal panel shows which",
                    recovered.len()
                )));
            }
            emit_goals_changed(goal_manager, task_manager, limits, events).await;
        }
        // `swap` both dedupes and re-arms: the first failure of an outage is
        // reported, the rest stay silent until a pass succeeds again.
        Err(error) => {
            if !failure_reported.swap(true, Ordering::Relaxed) {
                events.send(AgentUpdate::Info(format!(
                    "⚠ Goal reconciliation failed; goal state may be stale: {error:#}"
                )));
            }
        }
    }
}

/// Runs one reconciliation pass off the command loop.
fn spawn_goal_reconcile(
    goal_manager: &SharedGoalManager,
    task_manager: &SharedTaskManager,
    limits: GoalLimits,
    events: &UpdateSink,
    session_id: Option<String>,
    failure_reported: Arc<AtomicBool>,
) {
    let goal_manager = goal_manager.clone();
    let task_manager = task_manager.clone();
    let events = events.clone();
    tokio::spawn(async move {
        reconcile_goal_runs(
            &goal_manager,
            &task_manager,
            limits,
            &events,
            session_id.as_deref(),
            &failure_reported,
        )
        .await;
    });
}

/// Process `UserCommand`s until the channel closes, then shut down MCP.
///
/// `SubmitTask` runs in a background task so `Cancel` can set `cancel_flag`
/// while `agent_loop` is in progress. Integration tests drive this with a fake TUI.
///
/// This convenience wrapper does **not** wire an account-update channel; balance
/// queries initiated through it are dropped. Use
/// [`run_command_loop_with_account`] when the caller wants to receive
/// [`AccountUpdate`] messages.
pub async fn run_command_loop(
    agent: Agent,
    user_cmd_rx: UnboundedReceiver<UserCommand>,
    image_work_dir: impl AsRef<Path>,
) -> Agent {
    run_command_loop_with_account(agent, user_cmd_rx, image_work_dir, None).await
}

/// Send a command's answer to the frontend.
///
/// The one place that decides how an outcome becomes an update, so the loop-level
/// and agent-side paths cannot render the same answer differently.
fn forward_command_outcome(outcome: &tact::command::CommandOutcome, events: &UpdateSink) {
    match outcome {
        tact::command::CommandOutcome::Done => {}
        tact::command::CommandOutcome::Message(text) => {
            events.send(AgentUpdate::Info(text.clone()));
        }
        tact::command::CommandOutcome::Emit(emitted) => {
            let update = emitted.get().clone();
            // An empty Goal projection is also the log's cue: the panel clears
            // either way, but a bare `/goal` in a fresh workspace must not look
            // like it did nothing.
            if let AgentUpdate::GoalsChanged { goals } = &update
                && goals.is_empty()
            {
                events.send(AgentUpdate::Info("(no goals)".to_string()));
            }
            events.send(update);
        }
        tact::command::CommandOutcome::Failed(text)
        | tact::command::CommandOutcome::Usage(text) => {
            events.send(AgentUpdate::Error(AgentErrorKind::Other(text.clone())));
        }
    }
}

/// The catalog of host-side commands this loop serves.
///
/// Built where the shared state is, because each command closes over its own
/// handle (the stats snapshot, the background manager). A command that needs the
/// agent loop itself is not here: it cannot own a `&mut Agent` through a
/// catalog, so it keeps its arm on the driver's side.
fn builtin_command_catalog(
    slot: &Arc<tokio::sync::Mutex<Option<Agent>>>,
    chat: Option<std::sync::Arc<dyn tact::command::ChatService>>,
    work_dir: &std::path::Path,
) -> tact::command::CommandCatalog {
    // The loop holds the slot, so this lock is uncontended here.
    let guard = slot.try_lock().expect("the loop owns the slot here");
    build_command_catalog(guard.as_ref(), slot, chat, work_dir)
}

/// Build a catalog for a caller that already holds the agent.
///
/// Two entry points because the two callers differ in what they own: the loop
/// holds a slot, while `handle_user_command` holds `&mut Agent` outright. Passing
/// an empty slot from the second would silently drop `/goal` and `/background`,
/// which is exactly what the tests caught.
fn build_command_catalog(
    agent: Option<&Agent>,
    slot: &Arc<tokio::sync::Mutex<Option<Agent>>>,
    chat: Option<std::sync::Arc<dyn tact::command::ChatService>>,
    work_dir: &std::path::Path,
) -> tact::command::CommandCatalog {
    let session = agent.and_then(|agent| agent.runtime.session_id.clone());
    let background = agent.map(|agent| tact::command::BackgroundWiring {
        manager: std::sync::Arc::new(agent.tool_context.background_manager.clone()),
        session: std::sync::Arc::new(move || session.clone()),
    });
    let goals = agent.map(|agent| tact::command::GoalWiring {
        goal_manager: agent.tool_context.goal_manager.clone(),
        task_manager: agent.tool_context.task_manager.clone(),
        limits: agent_goal_limits(agent),
    });
    let stats = agent.map_or_else(
        || std::sync::Arc::new(std::sync::RwLock::new(tact::stats::SessionStats::default())),
        |agent| agent.runtime.stats.clone(),
    );
    tact::command::builtin_catalog(
        stats,
        background,
        goals,
        chat,
        Some(tact::command::HooksWiring {
            work_dir: work_dir.to_path_buf(),
            render: std::sync::Arc::new(crate::hooks_cli::render_hooks_listing),
        }),
        Some(tact::command::McpWiring {
            catalog: std::sync::Arc::new(RuntimeMcpCatalog { slot: slot.clone() }),
            render: std::sync::Arc::new(crate::mcp_cli::render_live_listing),
        }),
    )
}

/// An MCP view for a host that has no router in scope.
///
/// The palette needs the command to exist so it can be offered; the listing
/// itself runs in the command loop, which holds the live router. Reporting no
/// servers is the honest answer for a view that cannot see any.
pub struct EmptyMcpCatalog;

impl tact::command::McpCatalog for EmptyMcpCatalog {
    fn server_summaries(&self) -> Vec<(String, usize)> {
        Vec::new()
    }

    fn list_prompts<'a>(
        &'a self,
        _server: Option<&'a str>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<String>> + Send + 'a>>
    {
        Box::pin(async { anyhow::bail!("no MCP router is available in this view") })
    }
}

/// The read-only MCP view a `/mcp` listing uses.
///
/// It reads through the agent slot rather than capturing the router: `auth`
/// replaces the router on reload, and a captured copy would answer from the
/// router the session started with.
struct RuntimeMcpCatalog {
    slot: Arc<tokio::sync::Mutex<Option<Agent>>>,
}

impl tact::command::McpCatalog for RuntimeMcpCatalog {
    fn server_summaries(&self) -> Vec<(String, usize)> {
        match self.slot.try_lock() {
            Ok(guard) => guard
                .as_ref()
                .map(|agent| agent.mcp_router.server_summaries())
                .unwrap_or_default(),
            // A held lock means a turn is running; the listing then reports no
            // servers rather than stalling the caller behind that turn.
            Err(_) => Vec::new(),
        }
    }

    fn list_prompts<'a>(
        &'a self,
        server: Option<&'a str>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<String>> + Send + 'a>>
    {
        Box::pin(async move {
            let guard = self.slot.lock().await;
            let agent = guard
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("the agent is not installed yet"))?;
            agent.mcp_router.list_prompts(server).await
        })
    }
}

/// Like [`run_command_loop`], but forwards balance / usage quota results to the
/// provided account-update channel instead of mixing them into agent updates.
pub async fn run_command_loop_with_account(
    agent: Agent,
    mut user_cmd_rx: UnboundedReceiver<UserCommand>,
    image_work_dir: impl AsRef<Path>,
    account_tx: Option<UnboundedSender<AccountUpdate>>,
) -> Agent {
    let image_work_dir = image_work_dir.as_ref().to_path_buf();
    let cancel_flag = agent.runtime.cancel_flag.clone();
    // The turn lifecycle's service, when the host installed one. Cancelling
    // through it and through the flag are the same operation — the service holds
    // the same flag — so the fallback below is not a second behaviour.
    let chat = agent.chat_service();
    let events = agent.runtime.events.clone();
    // Shared stats snapshot: QueryStats can read it without awaiting the
    // in-flight task (the Agent itself is exclusively owned by that task).
    // Shared UI responder: routes TUI select responses to the waiter (parent
    // or subagent) even while the Agent is owned by the in-flight task.
    let ui_responder = agent.tool_context.ui_responder.clone();
    // Shared subagent manager: lets CancelSubagent flip a running child's
    // cooperative cancel flag without owning the parent Agent.
    let subagent_manager = agent.tool_context.subagent_manager.clone();
    // Shared background manager + session id: `/background` answers from the
    // manager directly instead of waiting for the in-flight turn to hand the
    // Agent back, so a listing opens the moment it is asked for.
    // The host-side commands, built once: a command answers from shared state,
    // so the loop does not rebuild one per invocation, and a name that is not
    // here is not a command at all.
    // Read again for the loop's own use: the catalog takes the agent now, but the
    // reconciliation and the goal panel still need these handles directly.
    let goal_manager = agent.tool_context.goal_manager.clone();
    let task_manager = agent.tool_context.task_manager.clone();
    let background_session_id = agent.runtime.session_id.clone();
    let limits = agent_goal_limits(&agent);
    // Reconcile durable Goal state once, before serving commands: a crashed
    // runner leaves an active run with an expired lease and a Goal still marked
    // `Running`. Non-fatal by design - a recovery failure is reported, never
    // allowed to take the command loop down with it.
    recover_interrupted_goals(&agent).await;

    // ... and again on a timer: the owner of a lease can die while *this*
    // process is up (another session crashes, or the run's own process is
    // killed), and nothing else would notice before the next restart.
    let mut goal_reconcile = tokio::time::interval(GOAL_RECONCILE_INTERVAL);
    goal_reconcile.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // `interval` fires immediately; the startup pass above already ran, so the
    // first tick is consumed here.
    goal_reconcile.tick().await;

    // Publish the durable Goal inventory once, before serving commands: the
    // panel (and the `/goal <verb>` completion that reads it) must not stay
    // empty until some later Goal command happens to emit a snapshot.
    emit_goals_changed(&goal_manager, &task_manager, limits, &events).await;

    // The slot is created before the catalog: `/mcp` reads the router through it,
    // and a command that captured the router directly would go stale the first
    // time `auth` reloaded it.
    let agent_slot: Arc<tokio::sync::Mutex<Option<Agent>>> =
        Arc::new(tokio::sync::Mutex::new(Some(agent)));
    let commands = builtin_command_catalog(
        &agent_slot,
        chat.as_ref()
            .map(|chat| chat.clone() as std::sync::Arc<dyn tact::command::ChatService>),
        &image_work_dir,
    );

    // One owner for the whole loop: the agent lives in this slot and is never
    // moved into a task. A task locks it for the duration of its work, which is
    // what the single-owner model gave for free — and which the chat service
    // already relies on, since it holds the same slot.
    let mut active: Option<JoinHandle<()>> = None;
    // Whether the local in-flight task is a Goal activation, and which Goal.
    // `None` while a normal task runs; the id is unknown during CreateGoal
    // (the Goal is minted inside that task). Only a matching local run may be
    // interrupted with the shared cancel flag.
    let mut active_goal_run = false;
    let mut active_goal_id: Option<String> = None;
    // A `SubagentFinishedNotification` that arrives while a turn is in flight
    // cannot be dropped: the result may already have been drained from the
    // queue at the start of the turn, and once the turn exits there is nothing
    // left to wake the parent. Retain the wake-up until the in-flight task
    // completes, then submit it.
    let mut pending_subagent_wakeup = false;
    // One "reconciliation failed" notice per outage: a broken store must not
    // print a line every tick. Shared with the spawned pass.
    let reconcile_failure_reported = Arc::new(AtomicBool::new(false));

    loop {
        let cmd = if active.is_some() {
            tokio::select! {
                result = active.as_mut().expect("active task") => {
                    result.expect("active task join panicked");
                    active = None;
                    active_goal_run = false;
                    active_goal_id = None;
                    if pending_subagent_wakeup {
                        pending_subagent_wakeup = false;
                        spawn_wakeup_task(&agent_slot, &mut active, &image_work_dir);
                        active_goal_run = false;
                        active_goal_id = None;
                    }
                    continue;
                }
                _ = goal_reconcile.tick() => {
                    spawn_goal_reconcile(
                        &goal_manager,
                        &task_manager,
                        limits,
                        &events,
                        background_session_id.clone(),
                        reconcile_failure_reported.clone(),
                    );
                    continue;
                }
                cmd = user_cmd_rx.recv() => cmd,
            }
        } else {
            tokio::select! {
                _ = goal_reconcile.tick() => {
                    spawn_goal_reconcile(
                        &goal_manager,
                        &task_manager,
                        limits,
                        &events,
                        background_session_id.clone(),
                        reconcile_failure_reported.clone(),
                    );
                    continue;
                }
                cmd = user_cmd_rx.recv() => cmd,
            }
        };
        let Some(cmd) = cmd else { break };

        match cmd {
            UserCommand::Cancel => {
                // Through the service when there is one: it is the component that
                // owns the turn, and its cancel is the same lock-free flag write.
                match &chat {
                    Some(chat) => chat.cancel(),
                    None => cancel_flag.store(true, Ordering::Relaxed),
                }
                events.send(AgentUpdate::Info("Cancelling...".into()));
            }
            UserCommand::CancelSubagent { child_id } => {
                if subagent_manager.request_cancel(&child_id) {
                    let _ = subagent_manager.cancel(&child_id).await;
                    tact::subagent::emit_subagents_changed(&events, &subagent_manager).await;
                    events.send(AgentUpdate::Info(format!(
                        "Cancelling subagent {child_id}..."
                    )));
                } else {
                    events.send(AgentUpdate::Info(format!(
                        "No running subagent {child_id} to cancel"
                    )));
                }
            }
            UserCommand::RunCommand { name, args } => {
                // One transport for every catalog command: the view forwards the
                // name and the raw arguments, and the catalog parses them. A
                // dedicated variant per command was a second definition of the
                // same grammar.
                let invocation = tact::command::CommandInvocation { name, args };
                forward_command_outcome(&commands.run_async(&invocation).await, &events);
            }
            UserCommand::SetGoalArchived { goal_id, archived } => {
                set_goal_archived(
                    &goal_manager,
                    &task_manager,
                    limits,
                    &events,
                    &goal_id,
                    archived,
                )
                .await;
            }
            // These three are safe to serve while the Agent is owned by an
            // in-flight Goal run: they only touch the shared managers (and the
            // cancel flag), so they must not await the run handle.
            UserCommand::SteerGoal { goal_id, text } => {
                steer_goal(
                    &goal_manager,
                    &task_manager,
                    &events,
                    limits,
                    goal_id.as_deref(),
                    &text,
                )
                .await;
            }
            UserCommand::PauseGoal { goal_id } => {
                let interrupt_local = active_goal_run
                    && (active_goal_id.is_none()
                        || active_goal_id.as_deref() == Some(goal_id.as_str()));
                request_goal_pause(
                    &goal_manager,
                    &task_manager,
                    limits,
                    &events,
                    &cancel_flag,
                    &goal_id,
                    interrupt_local,
                )
                .await;
            }
            UserCommand::CancelGoal { goal_id } => {
                let interrupt_local = active_goal_run
                    && (active_goal_id.is_none()
                        || active_goal_id.as_deref() == Some(goal_id.as_str()));
                request_goal_cancel(
                    &goal_manager,
                    &task_manager,
                    limits,
                    &events,
                    &cancel_flag,
                    &goal_id,
                    interrupt_local,
                )
                .await;
            }
            UserCommand::SubagentFinishedNotification { .. } => {
                // If a turn is active, retain the wake-up until that turn's
                // JoinHandle completes. Otherwise the notification could be
                // lost in the gap between the final queue drain and turn exit.
                if active.is_none() {
                    spawn_wakeup_task(&agent_slot, &mut active, &image_work_dir);
                    active_goal_run = false;
                    active_goal_id = None;
                } else {
                    pending_subagent_wakeup = true;
                }
            }
            UserCommand::SubmitTask(task) => {
                // A Goal that asked a question is waiting for exactly this
                // text. Only the *user* path routes here: agent-originated
                // submits (subagent wake-ups) go straight to
                // `handle_user_command` and must never be read as an answer.
                let answer_for = if task.contains('@') || task.contains("![") {
                    // `@file` / image references are expanded into the turn;
                    // recording them as a Goal event would drop the payload.
                    None
                } else {
                    goal_awaiting_answer(&goal_manager).await
                };
                if let Some(handle) = active.take() {
                    handle.await.expect("task join panicked");
                }
                // The new user turn drains pending subagent results itself;
                // do not add a redundant wake-up after it finishes.
                pending_subagent_wakeup = false;
                let work_dir = image_work_dir.clone();
                let slot = agent_slot.clone();
                // Cloned per spawn: the loop keeps its own handle for the
                // `Cancel` arm.
                let chat = chat.clone();
                if let Some(goal_id) = answer_for {
                    // The reply is the answer: record it and continue that
                    // Goal through the same entry `/goal resume` uses.
                    active_goal_run = true;
                    active_goal_id = Some(goal_id.clone());
                    active = Some(tokio::spawn(async move {
                        let mut guard = slot.lock().await;
                        let Some(task_agent) = guard.as_mut() else {
                            return;
                        };
                        answer_goal(task_agent, &goal_id, &task).await;
                        task_agent.emit_update(AgentUpdate::TaskIdle);
                    }));
                } else {
                    active_goal_run = false;
                    active_goal_id = None;
                    active = Some(tokio::spawn(async move {
                        // Through the chat service when the host installed one:
                        // it locks the same slot this loop owns, so a turn and a
                        // compaction cannot interleave. `run_turn` is the same
                        // lifecycle the fallback uses, so the two paths cannot
                        // diverge on Stop-hook continuation or completion.
                        match chat {
                            Some(chat) => {
                                let _ = chat
                                    .submit(tact::agent::chat::TurnRequest::text(task))
                                    .await;
                            }
                            None => {
                                let mut guard = slot.lock().await;
                                let Some(task_agent) = guard.as_mut() else {
                                    return;
                                };
                                handle_user_command(
                                    task_agent,
                                    UserCommand::SubmitTask(task),
                                    &work_dir,
                                )
                                .await;
                            }
                        }
                    }));
                }
            }
            UserCommand::CreateGoal {
                title,
                objective,
                criteria,
            } => {
                // Spawned, not awaited: creating a Goal now runs its first
                // activation, which must not block the loop that has to keep
                // serving `Cancel` (same shape as `ResumeGoal` below).
                if let Some(handle) = active.take() {
                    handle.await.expect("task join panicked");
                }
                let slot = agent_slot.clone();
                active_goal_run = true;
                active_goal_id = None;
                active = Some(tokio::spawn(async move {
                    let mut guard = slot.lock().await;
                    let Some(task_agent) = guard.as_mut() else {
                        return;
                    };
                    if let Some(goal_id) = create_goal(task_agent, title, objective, criteria).await
                    {
                        resume_goal(task_agent, &goal_id).await;
                    }
                    // A Goal activation does not go through `SubmitTask`, so no
                    // TaskComplete/TaskCancelled clears the TUI's busy state.
                    task_agent.emit_update(AgentUpdate::TaskIdle);
                }));
            }
            UserCommand::ResumeGoal { goal_id } => {
                if let Some(handle) = active.take() {
                    handle.await.expect("task join panicked");
                }
                let slot = agent_slot.clone();
                active_goal_run = true;
                active_goal_id = Some(goal_id.clone());
                active = Some(tokio::spawn(async move {
                    let mut guard = slot.lock().await;
                    let Some(task_agent) = guard.as_mut() else {
                        return;
                    };
                    resume_goal(task_agent, &goal_id).await;
                    task_agent.emit_update(AgentUpdate::TaskIdle);
                }));
            }
            UserCommand::AcceptGoal { goal_id, text } => {
                if let Some(handle) = active.take() {
                    handle.await.expect("task join panicked");
                }
                let slot = agent_slot.clone();
                active_goal_run = true;
                active_goal_id = Some(goal_id.clone());
                active = Some(tokio::spawn(async move {
                    let mut guard = slot.lock().await;
                    let Some(task_agent) = guard.as_mut() else {
                        return;
                    };
                    accept_goal(task_agent, &goal_id, &text).await;
                    task_agent.emit_update(AgentUpdate::TaskIdle);
                }));
            }
            other => {
                if let Some(handle) = active.take() {
                    handle.await.expect("task join panicked");
                }
                active_goal_run = false;
                active_goal_id = None;
                {
                    // Locked for the duration of one command: the loop no longer
                    // owns the agent, so this is the only way to reach it.
                    let mut guard = agent_slot.lock().await;
                    if let Some(a) = guard.as_mut() {
                        handle_user_command_with_account(
                            a,
                            other,
                            &image_work_dir,
                            account_tx.as_ref(),
                        )
                        .await;
                    }
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
        events.send(AgentUpdate::Info(
            "Cancelling background subagents (parent exiting)...".into(),
        ));
    }

    if let Some(handle) = active.take() {
        handle.await.expect("task join panicked");
    }

    // Take the agent back once, at exit: the slot was its home for the whole
    // loop, and the caller wants it for teardown.
    let mut agent = agent_slot
        .lock()
        .await
        .take()
        .expect("the agent is still in the slot after the loop");
    // SessionEnd hooks fire once at teardown, symmetrical with SessionStart.
    let _ = agent.dispatch_session_end_hooks().await;
    agent.shutdown_mcp().await;
    agent
}

fn spawn_wakeup_task(
    agent: &Arc<tokio::sync::Mutex<Option<Agent>>>,
    active: &mut Option<JoinHandle<()>>,
    image_work_dir: &Path,
) {
    if active.is_some() {
        return;
    }
    // A wake-up turn only delivers queued results into the parent's context.
    // If a turn that just finished already drained the queue, the summary is
    // already in context and this turn would be empty — skip it entirely.
    // `try_lock` because this runs on the loop's own turn: a held lock means a
    // task is already using the agent, and there is nothing to wake.
    let has_results = agent
        .try_lock()
        .map(|slot| {
            slot.as_ref()
                .is_some_and(Agent::has_pending_subagent_results)
        })
        .unwrap_or(false);
    if !has_results {
        return;
    }
    let slot = agent.clone();
    let work_dir = image_work_dir.to_path_buf();
    *active = Some(tokio::spawn(async move {
        // The queued result reaches the model during the drain below, so it
        // may legitimately not appear verbatim as a result "below"; point at
        // `check_subagent` as the fallback way to retrieve it.
        let prompt = "A background subagent finished. Review its result below \
                      (or call check_subagent if none is shown)."
            .to_string();
        let mut guard = slot.lock().await;
        let Some(task_agent) = guard.as_mut() else {
            return;
        };
        handle_user_command(task_agent, UserCommand::SubmitTask(prompt), &work_dir).await;
    }));
}

/// Handle a single user command (shared by the loop and tests).
///
/// This wrapper discards any account-related updates; tests that need to
/// observe them should use [`run_command_loop_with_account`].
/// Fetches one MCP prompt and renders it as the text of a user turn.
///
/// Split out from the command arm so the fetch-and-render path is reachable
/// without running a turn: the arm is then three lines, and this is the one
/// place the router, the renderer and the two error messages meet.
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
            tact::mcp::prompt_arguments_from_pairs(arguments),
        )
        .await
        .map_err(|error| format!("MCP prompt {server}/{name} failed: {error:#}"))
}

async fn goal_snapshots(
    goal_manager: &SharedGoalManager,
    task_manager: &SharedTaskManager,
    limits: GoalLimits,
) -> Result<Vec<GoalSnapshot>> {
    goal_snapshots_with(goal_manager, task_manager, limits, false).await
}

/// Snapshot builder with the archived switch.
///
/// Archived Goals are off by default: the snapshot is what the sticky panel
/// draws, and a finished Goal that has been filed away must stop taking rows
/// there. `/goal list --all` asks for them, which is how an archived Goal stays
/// discoverable without keeping it on screen.
async fn goal_snapshots_with(
    goal_manager: &SharedGoalManager,
    task_manager: &SharedTaskManager,
    limits: GoalLimits,
    include_archived: bool,
) -> Result<Vec<GoalSnapshot>> {
    // The projection lives in `tact` so a command can reach it without the
    // extension crate depending on the view.
    tact::goal::snapshots::snapshots(goal_manager, task_manager, limits, include_archived).await
}

pub(crate) async fn emit_goals_changed(
    goal_manager: &SharedGoalManager,
    task_manager: &SharedTaskManager,
    limits: GoalLimits,
    events: &UpdateSink,
) {
    match goal_snapshots(goal_manager, task_manager, limits).await {
        Ok(goals) => {
            events.send(AgentUpdate::GoalsChanged { goals });
        }
        Err(error) => emit_goal_error(events, format!("Goal snapshot failed: {error:#}")),
    }
}

/// Create a Goal from the command surface, announce it, and return its id.
///
/// The interactive surface has no separate confirm step, so creation *is* the
/// authorization to run: a `draft` Goal that nobody activates is a dead end
/// (the store keeps it, the Runner never picks it up). The caller starts the
/// first activation with [`resume_goal`], which drives `Draft → Queued →
/// Running`.
///
/// The snapshot goes out before the announcement: the log line names the id, and
/// the panel row does not show it.
async fn create_goal(
    agent: &mut Agent,
    title: String,
    objective: String,
    criteria: Vec<String>,
) -> Option<String> {
    // Criteria are optional on purpose. Injecting a required human criterion
    // here used to make every Goal end in `waiting_user` + `/goal accept`,
    // which turned the user's acceptance into a rubber stamp for work they had
    // not verified. A Goal with no criteria finishes on the model's proposal
    // and the runner records that as self-attested; declare criteria (API
    // callers can) when the result really needs a gate.
    let criteria = criteria
        .into_iter()
        .map(|description| GoalCriterion::new(description, true, GoalVerificationKind::Human))
        .collect::<Vec<_>>();
    match agent
        .tool_context
        .goal_manager
        .create(GoalCreate {
            title,
            objective,
            criteria,
        })
        .await
    {
        Ok(goal) => {
            let criteria_note = if goal.has_required_criteria() {
                ""
            } else {
                " - no acceptance criteria: completion will be self-attested"
            };
            emit_goals_changed(
                &agent.tool_context.goal_manager,
                &agent.tool_context.task_manager,
                agent_goal_limits(agent),
                &agent.runtime.events,
            )
            .await;
            agent.runtime.events.send(AgentUpdate::Info(format!(
                "🎯 Goal {} created ({}) - starting run{criteria_note}",
                goal.id, goal.title,
            )));
            Some(goal.id)
        }
        Err(error) => {
            emit_goal_error(
                &agent.runtime.events,
                format!("Goal creation failed: {error:#}"),
            );
            None
        }
    }
}

/// File a finished Goal out of the panel, or bring it back.
///
/// Not a lifecycle change: `archived_at` is a stamp on the Goal, so a failed
/// Goal stays `failed` while it is off the list. Only a terminal Goal can be
/// archived — an archive that hid running work would take it off the panel
/// while it kept going — and repeating the verb reports the state instead of
/// erroring, the same rule `/skill disable` follows.
async fn set_goal_archived(
    goal_manager: &SharedGoalManager,
    task_manager: &SharedTaskManager,
    limits: GoalLimits,
    events: &UpdateSink,
    goal_id: &str,
    archived: bool,
) {
    let goal = match goal_manager.get(goal_id).await {
        Ok(goal) => goal,
        Err(error) => {
            emit_goal_error(events, format!("Goal {goal_id} not found: {error:#}"));
            return;
        }
    };
    if archived && !goal.status.is_terminal() {
        emit_goal_error(
            events,
            format!(
                "Goal {goal_id} is {:?}; only a finished Goal can be archived",
                goal.status
            ),
        );
        return;
    }
    if goal.is_archived() == archived {
        events.send(AgentUpdate::Info(format!(
            "Goal {goal_id} is {}",
            if archived {
                "already archived"
            } else {
                "not archived"
            }
        )));
        return;
    }
    let kind = if archived {
        GoalEventKind::Archived
    } else {
        GoalEventKind::Unarchived
    };
    let event = goal_event(
        &goal,
        kind,
        None,
        format!("{kind} by user"),
        // `/goal archive` is a user command; the event says who filed it.
        GoalEventSource::User,
    );
    match goal_manager
        .update(
            goal_id,
            goal.version,
            GoalUpdate {
                archived: Some(archived),
                ..GoalUpdate::default()
            },
            Some(event),
            None,
        )
        .await
    {
        Ok(updated) => {
            // The panel refresh goes first: the announcement names a Goal the
            // panel may no longer show.
            emit_goals_changed(goal_manager, task_manager, limits, events).await;
            events.send(AgentUpdate::Info(format!(
                "🎯 Goal {} {kind} ({})",
                updated.id, updated.title
            )));
        }
        Err(error) => {
            emit_goal_error(events, format!("Goal {goal_id} {kind} failed: {error:#}"));
        }
    }
}

fn goal_event(
    goal: &Goal,
    kind: GoalEventKind,
    status: Option<GoalStatus>,
    payload: String,
    source: GoalEventSource,
) -> GoalEvent {
    GoalEvent {
        id: new_event_id(),
        goal_id: goal.id.clone(),
        run_id: None,
        kind,
        source,
        status,
        outcome: None,
        payload: Some(payload),
        occurred_at: Utc::now(),
    }
}

fn emit_goal_error(events: &UpdateSink, message: String) {
    events.send(AgentUpdate::Error(AgentErrorKind::Other(message)));
}

/// Resolves and records `/goal steer [<id>] <text>`.
///
/// The TUI forwards an id only when it recognized one in its own snapshot; the
/// store is authoritative here, so an empty or lagging snapshot can neither
/// mis-target a Goal nor turn the instruction into a fake id. Without an
/// explicit id, a leading id/prefix in the text is accepted (the panel may not
/// have caught up), then the single running Goal, then the single non-terminal
/// Goal. An ambiguous target is an error, never a guess.
async fn steer_goal(
    goal_manager: &SharedGoalManager,
    task_manager: &SharedTaskManager,
    events: &UpdateSink,
    limits: GoalLimits,
    requested: Option<&str>,
    text: &str,
) {
    let text = text.trim();
    if text.is_empty() {
        emit_goal_error(events, "Goal steering text must not be empty".to_string());
        return;
    }
    let goals = match goal_manager.list().await {
        Ok(goals) => goals,
        Err(error) => {
            emit_goal_error(events, format!("Goal steering failed: {error:#}"));
            return;
        }
    };

    let resolved = match requested {
        Some(prefix) => match unique_goal_matching(&goals, prefix) {
            Ok(Some(goal)) if !goal.status.is_terminal() => Ok((goal, text.to_string())),
            Ok(Some(goal)) => Err(format!(
                "Goal {} is terminal ({:?}); there is nothing left to steer",
                goal.id, goal.status
            )),
            Ok(None) => Err(format!("Goal {prefix} not found")),
            Err(count) => Err(format!(
                "Goal id prefix {prefix} matches {count} goals; type more characters"
            )),
        },
        None => {
            let mut parts = text.splitn(2, char::is_whitespace);
            let first = parts.next().unwrap_or_default();
            let rest = parts.next().unwrap_or_default().trim();
            match unique_goal_matching(&goals, first) {
                // An explicit id that the TUI could not resolve (empty or
                // stale snapshot) is still honored.
                Ok(Some(goal)) if !rest.is_empty() && !goal.status.is_terminal() => {
                    Ok((goal, rest.to_string()))
                }
                // A known id with nothing after it is a missing instruction,
                // not an instruction that happens to be the id itself.
                Ok(Some(_)) => Err("goal steering text must not be empty".to_string()),
                // An ambiguous first token is an id/prefix the user tried to
                // address, not ordinary instruction text.
                Err(count) => Err(format!(
                    "Goal id prefix {first} matches {count} goals; type more characters"
                )),
                Ok(None) => match implicit_steer_target(&goals) {
                    Ok(Some(goal)) => Ok((goal, text.to_string())),
                    Ok(None) => {
                        Err("no running Goal to steer; use /goal steer <goal-id> <text>"
                            .to_string())
                    }
                    Err(count) => Err(format!(
                        "{count} goals are open and none is uniquely running; use /goal steer <goal-id> <text>"
                    )),
                },
            }
        }
    };
    let (goal, instruction) = match resolved {
        Ok(pair) => pair,
        Err(message) => {
            emit_goal_error(events, message);
            return;
        }
    };

    match goal_manager
        .append_event(goal_event(
            goal,
            GoalEventKind::Steering,
            None,
            instruction,
            GoalEventSource::User,
        ))
        .await
    {
        Ok(_) => {
            emit_goals_changed(goal_manager, task_manager, limits, events).await;
        }
        Err(error) => emit_goal_error(
            events,
            format!("Goal {} steering failed: {error:#}", goal.id),
        ),
    }
}

/// Exact id or unique prefix; `Err(n)` means the prefix is ambiguous.
fn unique_goal_matching<'a>(goals: &'a [Goal], prefix: &str) -> Result<Option<&'a Goal>, usize> {
    let mut matches = goals
        .iter()
        .filter(|goal| goal.id == prefix || goal.id.starts_with(prefix));
    match (matches.next(), matches.next()) {
        (None, _) => Ok(None),
        (Some(goal), None) => Ok(Some(goal)),
        (Some(_), Some(_)) => {
            let count = goals
                .iter()
                .filter(|goal| goal.id == prefix || goal.id.starts_with(prefix))
                .count();
            Err(count)
        }
    }
}

/// The single running Goal, or the single non-terminal Goal when none is
/// running (a fresh activation can still be reported as `draft`/`queued`).
/// `Err(n)` means several candidates exist.
fn implicit_steer_target(goals: &[Goal]) -> Result<Option<&Goal>, usize> {
    let running: Vec<&Goal> = goals
        .iter()
        .filter(|goal| goal.status == GoalStatus::Running)
        .collect();
    match running.len() {
        1 => Ok(Some(running[0])),
        0 => {
            let open: Vec<&Goal> = goals
                .iter()
                .filter(|goal| !goal.status.is_terminal())
                .collect();
            match open.len() {
                0 => Ok(None),
                1 => Ok(Some(open[0])),
                count => Err(count),
            }
        }
        count => Err(count),
    }
}

async fn update_goal_status(
    agent: &Agent,
    goal_id: &str,
    status: GoalStatus,
    reason: &str,
) -> bool {
    let manager = &agent.tool_context.goal_manager;
    let result = async {
        let goal = manager.get(goal_id).await?;
        manager
            .update(
                goal_id,
                goal.version,
                GoalUpdate {
                    status: Some(status),
                    ..GoalUpdate::default()
                },
                Some(goal_event(
                    &goal,
                    GoalEventKind::StatusChanged,
                    Some(status),
                    reason.into(),
                    tact::goal::GoalEventSource::Runner,
                )),
                None,
            )
            .await
    }
    .await;
    match result {
        Ok(_) => {
            emit_goals_changed(
                manager,
                &agent.tool_context.task_manager,
                agent_goal_limits(agent),
                &agent.runtime.events,
            )
            .await;
            true
        }
        Err(error) => {
            emit_goal_error(
                &agent.runtime.events,
                format!("Goal {goal_id} status update failed: {error:#}"),
            );
            false
        }
    }
}

fn is_active_run_error(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        matches!(
            cause.downcast_ref::<GoalStoreError>(),
            Some(GoalStoreError::ActiveRunExists)
        )
    })
}

/// `/goal pause` — a run-local stop request, not a status write racing the run.
///
/// The request is a durable event first, so it works even while another
/// process owns the Agent. If this session owns the run, setting the shared
/// cancel flag makes the current model/tool wave stop early; the runner maps
/// that bare cancel to `paused` (not terminal `cancelled`).
async fn request_goal_pause(
    goal_manager: &SharedGoalManager,
    task_manager: &SharedTaskManager,
    limits: GoalLimits,
    events: &UpdateSink,
    cancel_flag: &Arc<AtomicBool>,
    goal_id: &str,
    interrupt_local: bool,
) {
    let goal = match goal_manager.get(goal_id).await {
        Ok(goal) => goal,
        Err(error) => {
            emit_goal_error(events, format!("Goal {goal_id} not found: {error:#}"));
            return;
        }
    };
    if goal.status.is_terminal() {
        emit_goal_error(
            events,
            format!(
                "Goal {goal_id} is already {:?}; nothing to pause",
                goal.status
            ),
        );
        return;
    }
    if goal.status == GoalStatus::Paused {
        events.send(AgentUpdate::Info(format!(
            "Goal {goal_id} is already paused"
        )));
        return;
    }

    let runs = match goal_manager.list_running_runs().await {
        Ok(runs) => runs,
        Err(error) => {
            emit_goal_error(events, format!("Goal {goal_id} pause failed: {error:#}"));
            return;
        }
    };
    let target_active = runs.iter().any(|run| run.goal_id == goal.id);

    if target_active {
        if let Err(error) = goal_manager
            .append_event(goal_event(
                &goal,
                GoalEventKind::PauseRequested,
                None,
                "paused by user".into(),
                GoalEventSource::User,
            ))
            .await
        {
            emit_goal_error(
                events,
                format!("Goal {goal_id} pause request failed: {error:#}"),
            );
            return;
        }
        if interrupt_local {
            cancel_flag.store(true, Ordering::Relaxed);
        }
        events.send(AgentUpdate::Info(format!(
            "🎯 Goal {goal_id} pause requested; stopping the current run at the next safe point"
        )));
    } else {
        match goal_manager
            .update(
                &goal.id,
                goal.version,
                GoalUpdate {
                    status: Some(GoalStatus::Paused),
                    ..GoalUpdate::default()
                },
                Some(goal_event(
                    &goal,
                    GoalEventKind::StatusChanged,
                    Some(GoalStatus::Paused),
                    "paused by user".into(),
                    GoalEventSource::User,
                )),
                None,
            )
            .await
        {
            Ok(_) => {
                events.send(AgentUpdate::Info(format!("🎯 Goal {goal_id} paused")));
            }
            // A run started between the probe and the write: the request event
            // must be timestamped after that run started, so append a fresh one.
            Err(error) if is_active_run_error(&error) => {
                if let Err(append) = goal_manager
                    .append_event(goal_event(
                        &goal,
                        GoalEventKind::PauseRequested,
                        None,
                        "paused by user".into(),
                        GoalEventSource::User,
                    ))
                    .await
                {
                    emit_goal_error(
                        events,
                        format!("Goal {goal_id} pause request failed: {append:#}"),
                    );
                    return;
                }
                if interrupt_local {
                    cancel_flag.store(true, Ordering::Relaxed);
                }
                events.send(AgentUpdate::Info(format!(
                    "🎯 Goal {goal_id} pause requested; stopping the current run at the next safe point"
                )));
            }
            Err(error) => {
                emit_goal_error(events, format!("Goal {goal_id} pause failed: {error:#}"))
            }
        }
    }
    emit_goals_changed(goal_manager, task_manager, limits, events).await;
}

/// `/goal cancel` — an explicit terminal decision.
///
/// The durable `cancel_requested` event is what distinguishes this from a
/// bare Esc interrupt: the runner maps the former to `cancelled` and the
/// latter to `paused`.
async fn request_goal_cancel(
    goal_manager: &SharedGoalManager,
    task_manager: &SharedTaskManager,
    limits: GoalLimits,
    events: &UpdateSink,
    cancel_flag: &Arc<AtomicBool>,
    goal_id: &str,
    interrupt_local: bool,
) {
    let goal = match goal_manager.get(goal_id).await {
        Ok(goal) => goal,
        Err(error) => {
            emit_goal_error(events, format!("Goal {goal_id} not found: {error:#}"));
            return;
        }
    };
    if goal.status.is_terminal() {
        events.send(AgentUpdate::Info(format!(
            "Goal {goal_id} is already {:?}",
            goal.status
        )));
        return;
    }

    let runs = match goal_manager.list_running_runs().await {
        Ok(runs) => runs,
        Err(error) => {
            emit_goal_error(events, format!("Goal {goal_id} cancel failed: {error:#}"));
            return;
        }
    };
    let target_active = runs.iter().any(|run| run.goal_id == goal.id);

    if target_active {
        if let Err(error) = goal_manager
            .append_event(goal_event(
                &goal,
                GoalEventKind::CancelRequested,
                None,
                "cancelled by user".into(),
                GoalEventSource::User,
            ))
            .await
        {
            emit_goal_error(
                events,
                format!("Goal {goal_id} cancel request failed: {error:#}"),
            );
            return;
        }
        if interrupt_local {
            cancel_flag.store(true, Ordering::Relaxed);
        }
        events.send(AgentUpdate::Info(format!(
            "🎯 Goal {goal_id} cancel requested; stopping the current run"
        )));
    } else {
        match goal_manager
            .update(
                &goal.id,
                goal.version,
                GoalUpdate {
                    status: Some(GoalStatus::Cancelled),
                    ..GoalUpdate::default()
                },
                Some(goal_event(
                    &goal,
                    GoalEventKind::StatusChanged,
                    Some(GoalStatus::Cancelled),
                    "cancelled by user".into(),
                    GoalEventSource::User,
                )),
                None,
            )
            .await
        {
            Ok(_) => {
                events.send(AgentUpdate::Info(format!("🎯 Goal {goal_id} cancelled")));
            }
            Err(error) if is_active_run_error(&error) => {
                if let Err(append) = goal_manager
                    .append_event(goal_event(
                        &goal,
                        GoalEventKind::CancelRequested,
                        None,
                        "cancelled by user".into(),
                        GoalEventSource::User,
                    ))
                    .await
                {
                    emit_goal_error(
                        events,
                        format!("Goal {goal_id} cancel request failed: {append:#}"),
                    );
                    return;
                }
                if interrupt_local {
                    cancel_flag.store(true, Ordering::Relaxed);
                }
                events.send(AgentUpdate::Info(format!(
                    "🎯 Goal {goal_id} cancel requested; stopping the current run"
                )));
            }
            Err(error) => {
                emit_goal_error(events, format!("Goal {goal_id} cancel failed: {error:#}"))
            }
        }
    }
    emit_goals_changed(goal_manager, task_manager, limits, events).await;
}

/// A Goal activation is a fresh, user-committed unit of work, so it starts with
/// a clean cancel flag — the same reason `SubmitTask` clears it.
///
/// Without this, a cancel the user pressed for something else (a normal turn)
/// would hit the activation's first loop check, map to `AgentLoopStop::Cancelled`,
/// and drive the Goal into the terminal `cancelled` state having done zero work.
fn clear_cancel_for_goal_activation(agent: &Agent) {
    agent.runtime.cancel_flag.store(false, Ordering::Relaxed);
}

/// The one Goal whose pending question this message answers, if there is one.
///
/// A `waiting_user` Goal splits into two cases that need opposite responses: a
/// Goal that *asked a question* (`needs_user` after its last `Running`
/// transition) is waiting for a reply, while a Goal parked on human acceptance
/// needs `/goal accept`. Only the first is a candidate for a chat message, and
/// only when the choice is unambiguous — with two waiting Goals a message would
/// have to guess.
async fn goal_awaiting_answer(manager: &SharedGoalManager) -> Option<String> {
    let goals = manager.list().await.ok()?;
    let mut waiting = Vec::new();
    for goal in goals
        .iter()
        .filter(|goal| goal.status == GoalStatus::WaitingUser)
    {
        let question = manager
            .latest_event_of_kind(&goal.id, GoalEventKind::NeedsUser)
            .await
            .ok()
            .flatten();
        let Some(question) = question else {
            continue;
        };
        let asked_since_running = match manager
            .latest_event_with_status(&goal.id, GoalStatus::Running)
            .await
            .ok()
            .flatten()
        {
            Some(running) => question.occurred_at >= running.occurred_at,
            None => true,
        };
        if asked_since_running {
            waiting.push(goal.id.clone());
        }
    }
    (waiting.len() == 1).then(|| waiting.remove(0))
}

/// Records `text` as the waiting Goal's answer and resumes it.
async fn answer_goal(agent: &mut Agent, goal_id: &str, text: &str) {
    let manager = agent.tool_context.goal_manager.clone();
    if let Ok(goal) = manager.get(goal_id).await {
        let _ = manager
            .append_event(goal_event(
                &goal,
                GoalEventKind::UserAnswer,
                None,
                text.to_string(),
                GoalEventSource::User,
            ))
            .await;
    }
    agent.runtime.events.send(AgentUpdate::Info(format!(
        "🎯 Goal {goal_id} was waiting for input — recorded your reply as the answer and resuming"
    )));
    resume_goal(agent, goal_id).await;
}

async fn resume_goal(agent: &mut Agent, goal_id: &str) {
    let goal = match agent.tool_context.goal_manager.get(goal_id).await {
        Ok(goal) => goal,
        Err(error) => {
            emit_goal_error(
                &agent.runtime.events,
                format!("Goal {goal_id} not found: {error:#}"),
            );
            return;
        }
    };
    let original_status = goal.status;
    if matches!(
        goal.status,
        GoalStatus::Paused | GoalStatus::WaitingUser | GoalStatus::WaitingPermission
    ) && !update_goal_status(agent, goal_id, GoalStatus::Running, "resumed by user").await
    {
        return;
    }
    clear_cancel_for_goal_activation(agent);
    let runner = GoalRunner::new(
        agent.tool_context.goal_manager.clone(),
        agent_goal_limits(agent),
    );
    if let Err(error) = runner.run_once(agent, goal_id).await {
        emit_goal_error(
            &agent.runtime.events,
            format!("Goal {goal_id} activation failed: {error:#}"),
        );
        // The resume transition above may have committed a `Running` status
        // that no activation ever took over (lease held, start refused). Put
        // it back to `paused` when nothing is running, so the panel does not
        // advertise work that is not happening and the next `/goal resume` is
        // the same explicit action.
        if matches!(
            original_status,
            GoalStatus::Paused | GoalStatus::WaitingUser | GoalStatus::WaitingPermission
        ) {
            let manager = &agent.tool_context.goal_manager;
            if let Ok(current) = manager.get(goal_id).await
                && current.status == GoalStatus::Running
                && manager
                    .list_running_runs()
                    .await
                    .map(|runs| !runs.iter().any(|run| run.goal_id == current.id))
                    .unwrap_or(false)
            {
                let _ = manager
                    .update(
                        goal_id,
                        current.version,
                        GoalUpdate {
                            status: Some(GoalStatus::Paused),
                            ..GoalUpdate::default()
                        },
                        Some(goal_event(
                            &current,
                            GoalEventKind::StatusChanged,
                            Some(GoalStatus::Paused),
                            "activation could not start; restored paused".into(),
                            tact::goal::GoalEventSource::Runner,
                        )),
                        None,
                    )
                    .await;
            }
        }
    }
    emit_goals_changed(
        &agent.tool_context.goal_manager,
        &agent.tool_context.task_manager,
        agent_goal_limits(agent),
        &agent.runtime.events,
    )
    .await;
}

async fn accept_goal(agent: &mut Agent, goal_id: &str, text: &str) {
    let text = text.trim();
    if text.is_empty() {
        emit_goal_error(
            &agent.runtime.events,
            format!("Goal {goal_id} acceptance text must not be empty"),
        );
        return;
    }
    let manager = agent.tool_context.goal_manager.clone();
    let goal = match manager.get(goal_id).await {
        Ok(goal) => goal,
        Err(error) => {
            emit_goal_error(
                &agent.runtime.events,
                format!("Goal {goal_id} not found: {error:#}"),
            );
            return;
        }
    };
    // Acceptance is a human decision, not a race with the runner. A message
    // sent while the Goal was `waiting_user` can wake the runner and leave it
    // `running` after a no-op activation, which used to make the hinted
    // `/goal accept` fail. Accept the stranded `running` state too, but only
    // when no activation is actually in flight.
    let active_run = match manager.list_running_runs().await {
        Ok(runs) => runs.iter().any(|run| run.goal_id == goal.id),
        Err(error) => {
            emit_goal_error(
                &agent.runtime.events,
                format!("Goal {goal_id} acceptance failed: {error:#}"),
            );
            return;
        }
    };
    // `paused` is equally acceptable: recovery (`Running` with no active run)
    // parks a Goal there, and a paused Goal is still a human decision away from
    // done. Only an actual activation in flight blocks acceptance.
    let acceptable = matches!(goal.status, GoalStatus::WaitingUser | GoalStatus::Paused)
        || (goal.status == GoalStatus::Running && !active_run);
    if !acceptable {
        let suffix = if active_run {
            " (an activation is in flight)"
        } else {
            ""
        };
        emit_goal_error(
            &agent.runtime.events,
            format!(
                "Goal {goal_id} is {:?}{suffix}; acceptance requires waiting_user, paused, or an idle running Goal",
                goal.status
            ),
        );
        return;
    }
    let evidence: Vec<_> = goal
        .criteria
        .iter()
        .filter(|criterion| {
            // Automatic criteria are reserved (creation is refused by
            // `GoalManager::create`), but legacy rows can still exist. An
            // explicit human acceptance is the only supported way to close
            // them, so it covers every pending required criterion.
            criterion.required && !criterion.has_valid_evidence(goal.criteria_version)
        })
        .map(|criterion| GoalEvidenceUpdate {
            criterion_id: criterion.id.clone(),
            value: text.to_string(),
        })
        .collect();
    if evidence.is_empty() {
        let hint = if goal.has_required_criteria() {
            "already accepted, or none pending"
        } else {
            "a Goal without acceptance criteria completes on its own"
        };
        emit_goal_error(
            &agent.runtime.events,
            format!("Goal {goal_id} has no pending required criterion ({hint})"),
        );
        return;
    }
    let first = GoalUpdate {
        evidence,
        // `waiting_user` / `paused` re-enter `running`; a stranded `running`
        // Goal stays there (Running -> Running is not a transition).
        status: matches!(goal.status, GoalStatus::WaitingUser | GoalStatus::Paused)
            .then_some(GoalStatus::Running),
        ..GoalUpdate::default()
    };
    // Verify a candidate before the transaction so the update chain can be
    // computed up front: evidence + Running + Verifying + Succeeded must be
    // one durable commit, otherwise a failure between the last two leaves the
    // Goal stranded in `verifying`.
    let mut candidate = goal.clone();
    if let Err(error) = candidate.update(first.clone()) {
        emit_goal_error(
            &agent.runtime.events,
            format!("Goal {goal_id} acceptance failed: {error:#}"),
        );
        return;
    }
    let verification = GoalVerifier::verify(&candidate);
    let mut updates = vec![(
        first,
        goal_event(
            &goal,
            GoalEventKind::UserAccepted,
            Some(GoalStatus::Running),
            text.to_string(),
            GoalEventSource::User,
        ),
    )];
    if verification == GoalVerification::Verified {
        updates.push((
            GoalUpdate {
                status: Some(GoalStatus::Verifying),
                ..GoalUpdate::default()
            },
            goal_event(
                &candidate,
                GoalEventKind::StatusChanged,
                Some(GoalStatus::Verifying),
                "human acceptance verified".into(),
                tact::goal::GoalEventSource::Runner,
            ),
        ));
        updates.push((
            GoalUpdate {
                status: Some(GoalStatus::Succeeded),
                ..GoalUpdate::default()
            },
            goal_event(
                &candidate,
                GoalEventKind::StatusChanged,
                Some(GoalStatus::Succeeded),
                "accepted by user".into(),
                tact::goal::GoalEventSource::Runner,
            ),
        ));
    }
    if let Err(error) = manager
        .update_chain(goal_id, goal.version, updates, None)
        .await
    {
        emit_goal_error(
            &agent.runtime.events,
            format!("Goal {goal_id} acceptance failed: {error:#}"),
        );
        return;
    }

    match verification {
        GoalVerification::Verified => {
            // The acceptance is durable now; say so in the log. Without this
            // line the last visible message is still the "run /goal accept"
            // instruction, which reads as if acceptance were still pending.
            agent.emit_update(AgentUpdate::Info(format!(
                "🎯 Goal {goal_id} accepted — succeeded"
            )));
        }
        GoalVerification::Rejected { .. } => {
            clear_cancel_for_goal_activation(agent);
            let runner = GoalRunner::new(manager.clone(), agent_goal_limits(agent));
            if let Err(error) = runner.run_once(agent, goal_id).await {
                emit_goal_error(
                    &agent.runtime.events,
                    format!("Goal {goal_id} continuation failed: {error:#}"),
                );
            }
        }
        GoalVerification::NeedsUser { .. } => {
            emit_goal_error(
                &agent.runtime.events,
                format!("Goal {goal_id} still needs additional human acceptance"),
            );
        }
    }
    emit_goals_changed(
        &manager,
        &agent.tool_context.task_manager,
        agent_goal_limits(agent),
        &agent.runtime.events,
    )
    .await;
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
        UserCommand::CreateGoal {
            title,
            objective,
            criteria,
        } => {
            // Creation is also the start: `/goal create` must not leave a Goal
            // that never runs (see `create_goal`).
            if let Some(goal_id) = create_goal(agent, title, objective, criteria).await {
                resume_goal(agent, &goal_id).await;
            }
        }
        UserCommand::RunCommand { name, args } => {
            // Same catalog the loop-level arm uses, so the two cannot answer
            // differently.
            // The agent-side path has no slot here (it holds `&mut Agent`
            // directly), so `/mcp` answers from no servers rather than stalling;
            // the loop-level arm is where a listing really runs.
            // This path holds `&mut Agent` outright, so it builds the catalog
            // from it directly. An empty slot here would silently drop `/goal`
            // and `/background`.
            let local_slot: Arc<tokio::sync::Mutex<Option<Agent>>> =
                Arc::new(tokio::sync::Mutex::new(None));
            let catalog = build_command_catalog(
                Some(agent),
                &local_slot,
                // The agent service path has no chat handle in scope here; the
                // loop-level arm is where `/compact` runs, and it passes one.
                None,
                image_work_dir,
            );
            // `goal show` answers with the detail popup only; the panel refresh
            // is a separate `list`, because one command outcome carries one
            // update. Doing it here keeps the order the loop-level path had:
            // panel first, then the detail.
            if name == "goal" && args.starts_with("show ") {
                let refresh = tact::command::CommandInvocation {
                    name: name.clone(),
                    args: "list".to_string(),
                };
                forward_command_outcome(&catalog.run_async(&refresh).await, &agent.runtime.events);
            }
            let invocation = tact::command::CommandInvocation { name, args };
            forward_command_outcome(&catalog.run_async(&invocation).await, &agent.runtime.events);
        }
        UserCommand::SteerGoal { goal_id, text } => {
            steer_goal(
                &agent.tool_context.goal_manager,
                &agent.tool_context.task_manager,
                &agent.runtime.events,
                agent_goal_limits(agent),
                goal_id.as_deref(),
                &text,
            )
            .await;
        }
        UserCommand::PauseGoal { goal_id } => {
            request_goal_pause(
                &agent.tool_context.goal_manager,
                &agent.tool_context.task_manager,
                agent_goal_limits(agent),
                &agent.runtime.events,
                &agent.runtime.cancel_flag,
                &goal_id,
                false,
            )
            .await;
        }
        UserCommand::ResumeGoal { goal_id } => resume_goal(agent, &goal_id).await,
        UserCommand::CancelGoal { goal_id } => {
            request_goal_cancel(
                &agent.tool_context.goal_manager,
                &agent.tool_context.task_manager,
                agent_goal_limits(agent),
                &agent.runtime.events,
                &agent.runtime.cancel_flag,
                &goal_id,
                false,
            )
            .await;
        }
        UserCommand::AcceptGoal { goal_id, text } => accept_goal(agent, &goal_id, &text).await,
        UserCommand::SetGoalArchived { goal_id, archived } => {
            set_goal_archived(
                &agent.tool_context.goal_manager,
                &agent.tool_context.task_manager,
                agent_goal_limits(agent),
                &agent.runtime.events,
                &goal_id,
                archived,
            )
            .await;
        }
        UserCommand::SubmitTask(task) => {
            agent.tool_use_counter = 0;
            agent.runtime.cancel_flag.store(false, Ordering::Relaxed);

            let task_message = build_user_message(&task, image_work_dir).await;

            // DeepSeek V4 and other text-only models reject `image_url` parts.
            // Reject images early rather than sending a broken request to the API.
            // `tact::config::supports_vision` honours a per-model override on top
            // of the endpoint heuristic.
            if task_message.has_images() && !tact::config::supports_vision() {
                // From the agent's own snapshot, not the process global: a
                // consumer that already has an agent should not have to re-ask.
                let model = agent.provider().map(|info| info.model).unwrap_or_default();
                agent.emit_update(AgentUpdate::Error(AgentErrorKind::Other(format!(
                    "Image attachments are not supported by {model}. \
                     The current model does not accept image input."
                ))));
                return;
            }

            // One turn lifecycle, shared with `ChatService`: the Stop-hook
            // continuation and the completion notice live in one place, so the
            // command path and the service path cannot drift.
            tact::agent::chat::run_turn(agent, task_message).await;
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
        UserCommand::SetPermissionMode(mode) => {
            let parsed = match mode.as_str() {
                "plan" => tact::permission::PermissionMode::Plan,
                "default" => tact::permission::PermissionMode::Default,
                _ => tact::permission::PermissionMode::Auto,
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
                tact::mcp::authorize_server(&server, &mut notify),
                line_rx,
                |line| agent.emit_update(AgentUpdate::Info(line)),
            )
            .await;
            match result {
                Ok(()) => {
                    agent.emit_update(AgentUpdate::Info(format!(
                        "Authorized MCP server {server}; reloading MCP servers..."
                    )));
                    let report = agent.reload_mcp_router().await;
                    for line in report.notice_lines() {
                        agent.emit_update(AgentUpdate::Info(line));
                    }
                    agent.emit_update(AgentUpdate::Info(format!(
                        "MCP reload complete ({} server(s) connected)",
                        report.connected.len()
                    )));
                }
                Err(error) => agent.emit_update(AgentUpdate::Error(AgentErrorKind::Other(
                    format!("MCP authorization failed for {server}: {error:#}"),
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
                agent.emit_update(AgentUpdate::Error(AgentErrorKind::Other(message)));
            }
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
    use std::sync::atomic::{AtomicBool, Ordering};

    use tact::goal::GoalEventKind;
    use tact_llm::{ContentBlock, MockClient, StopReason};
    use tact_protocol::{AgentUpdate, UpdateSink, UserCommand};

    use crate::test_support::{build_test_agent, install_test_config};

    fn text_block(content: &str) -> ContentBlock {
        ContentBlock::Text {
            text: content.to_string(),
        }
    }

    /// `[goal]` settings map onto the runner budget.
    ///
    /// The mapping is asserted against a snapshot handed in directly: the value
    /// is the one the loop enforces, so a wrong field (or the wrong unit for
    /// `max_elapsed_secs`) would silently shorten or extend every Goal run.
    #[test]
    fn goal_budgets_come_from_the_given_snapshot() {
        let mut config = crate::test_support::default_test_config();
        config.goal.max_turns = 7;
        config.goal.max_elapsed_secs = 11;

        let limits = super::goal_limits(Some(&config));
        assert_eq!(limits.max_turns, 7);
        assert_eq!(
            limits.max_elapsed.as_secs(),
            11,
            "max_elapsed_secs is seconds, not millis"
        );
    }

    /// The agent's budgets are the ones its own bundle recorded.
    ///
    /// Deliberately not asserting a literal: `Agent::new` snapshots whatever
    /// configuration was installed when it ran, and tests run in parallel, so a
    /// literal would pin the test fixture rather than the behaviour. What must
    /// hold is the relationship — the number the loop uses is the number in the
    /// agent's own snapshot.
    #[test]
    fn goal_budgets_follow_the_agents_own_snapshot() {
        let (agent, _dir) = build_test_agent(MockClient::new(Vec::new()), None);
        let from_bundle = super::agent_goal_limits(&agent);
        let expected = super::goal_limits(agent.config().as_ref());
        assert_eq!(from_bundle.max_turns, expected.max_turns);
        assert_eq!(from_bundle.max_elapsed, expected.max_elapsed);
    }

    /// Nothing in the driver reads the configuration global.
    ///
    /// This is the property that lets the frontend be decoupled: every budget
    /// the loop enforces has to arrive from the agent's snapshot, so a new
    /// `config::try_settings()` here would be a second source of truth. Reading
    /// the source is crude, but it is the only check that sees the whole
    /// production region without standing up a session.
    #[test]
    fn the_driver_reads_configuration_from_the_agent_not_the_global() {
        let source = include_str!("driver.rs");
        let body = source
            .split_once("#[cfg(test)]")
            .expect("the test module is declared here")
            .0;
        for forbidden in ["config::try_settings()", "config::settings()"] {
            assert!(
                !body.contains(forbidden),
                "the driver must take its configuration from the agent, found {forbidden}"
            );
        }
    }

    /// A snapshot that never existed keeps the built-in defaults.
    ///
    /// The non-LLM subcommand paths and unit tests have no installed
    /// configuration; they must keep working rather than panicking on the
    /// missing snapshot.
    #[test]
    fn a_missing_snapshot_falls_back_to_the_built_in_budgets() {
        let limits = super::goal_limits(None);
        let defaults = tact::goal::GoalLimits::default();
        assert_eq!(limits.max_turns, defaults.max_turns);
        assert_eq!(limits.max_elapsed, defaults.max_elapsed);
    }

    fn goal_in_status(id: &str, status: tact::goal::GoalStatus) -> tact::goal::Goal {
        let mut goal = tact::goal::Goal::new(tact::goal::GoalCreate {
            title: format!("goal {id}"),
            objective: format!("objective {id}"),
            criteria: vec![tact::goal::GoalCriterion::required("done")],
        })
        .unwrap();
        goal.id = id.to_string();
        goal.status = status;
        goal
    }

    /// The timer's reconciliation helper stays silent when nothing was
    /// reclaimed: no Info line, no redundant snapshot republish.
    #[tokio::test]
    async fn reconcile_goal_runs_is_silent_when_nothing_was_reclaimed() {
        install_test_config();
        let (events, mut ui_rx) = tokio::sync::mpsc::unbounded_channel();
        let mock = MockClient::new(vec![]);
        let (agent, _work_dir) = build_test_agent(mock, Some(events.clone()));
        let manager = agent.tool_context.goal_manager.clone();
        let task_manager = agent.tool_context.task_manager.clone();
        let goal = manager
            .create(tact::goal::GoalCreate {
                title: "idle goal".into(),
                objective: "stay quiet".into(),
                criteria: vec![tact::goal::GoalCriterion::required("done")],
            })
            .await
            .unwrap();
        manager
            .update(
                &goal.id,
                goal.version,
                tact::goal::GoalUpdate {
                    status: Some(tact::goal::GoalStatus::Queued),
                    ..Default::default()
                },
                None,
                None,
            )
            .await
            .unwrap();

        let reported = AtomicBool::new(false);
        super::reconcile_goal_runs(
            &manager,
            &task_manager,
            super::goal_limits(None),
            &UpdateSink::for_ui(events),
            Some("test-session"),
            &reported,
        )
        .await;

        assert!(!reported.load(Ordering::Relaxed));
        assert!(
            ui_rx.try_recv().is_err(),
            "an unchanged workspace must not produce an update"
        );
    }

    /// The timer path is the recovery the startup pass cannot cover: a lease
    /// that dies while this loop is up is reclaimed (and announced) without a
    /// restart.
    #[tokio::test]
    async fn reconcile_goal_runs_reports_a_reclaimed_run() {
        install_test_config();
        let (events, mut ui_rx) = tokio::sync::mpsc::unbounded_channel();
        let mock = MockClient::new(vec![]);
        let (agent, _work_dir) = build_test_agent(mock, Some(events.clone()));
        let manager = agent.tool_context.goal_manager.clone();
        let task_manager = agent.tool_context.task_manager.clone();
        let goal = manager
            .create(tact::goal::GoalCreate {
                title: "crashed goal".into(),
                objective: "be reclaimed".into(),
                criteria: vec![tact::goal::GoalCriterion::required("done")],
            })
            .await
            .unwrap();
        let goal = manager
            .update(
                &goal.id,
                goal.version,
                tact::goal::GoalUpdate {
                    status: Some(tact::goal::GoalStatus::Queued),
                    ..Default::default()
                },
                None,
                None,
            )
            .await
            .unwrap();
        let goal = manager
            .update(
                &goal.id,
                goal.version,
                tact::goal::GoalUpdate {
                    status: Some(tact::goal::GoalStatus::Running),
                    ..Default::default()
                },
                None,
                None,
            )
            .await
            .unwrap();

        // A holder that died without releasing: its lease lapses unreleased.
        let ttl = std::time::Duration::from_millis(20);
        let (_token, run) = manager
            .start_run(
                "crashed-runner",
                ttl,
                tact::goal::GoalRun::new(goal.id.clone()),
            )
            .await
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(60)).await;

        let reported = AtomicBool::new(false);
        super::reconcile_goal_runs(
            &manager,
            &task_manager,
            super::goal_limits(None),
            &UpdateSink::for_ui(events),
            Some("test-session"),
            &reported,
        )
        .await;

        let mut infos = Vec::new();
        let mut republished = false;
        while let Ok(update) = ui_rx.try_recv() {
            match update {
                AgentUpdate::Info(message) => infos.push(message),
                AgentUpdate::GoalsChanged { .. } => republished = true,
                _ => {}
            }
        }
        // The timer is workspace-wide: the run it reclaimed may belong to
        // another session, and `events` only reaches this one. So the line
        // summarises the pass instead of naming a run this user may never see,
        // and the shared snapshot carries the per-Goal truth.
        assert!(
            infos
                .iter()
                .any(|message| message.contains("reclaimed 1 interrupted run")),
            "the reclaimed pass is announced: {infos:?}"
        );
        assert!(
            !infos.iter().any(|message| message.contains(&run.id)),
            "the timer does not name a run another session may own: {infos:?}"
        );
        assert!(republished, "the panel is republished after a reclaim");
        assert_eq!(
            manager.get(&goal.id).await.unwrap().status,
            tact::goal::GoalStatus::Paused
        );
    }

    /// `/goal steer` resolution is store-authoritative: exact/unique-prefix id
    /// first, then the single running Goal, then the single non-terminal Goal.
    /// Ambiguity must be an error, never a guess.
    #[test]
    fn steer_target_resolution_is_unambiguous() {
        use tact::goal::GoalStatus;

        let goals = vec![
            goal_in_status("aaaa1111", GoalStatus::Paused),
            goal_in_status("aaaa2222", GoalStatus::Draft),
            goal_in_status("bbbb3333", GoalStatus::Running),
        ];

        assert_eq!(
            super::unique_goal_matching(&goals, "bbbb")
                .unwrap()
                .map(|goal| goal.id.as_str()),
            Some("bbbb3333")
        );
        assert!(
            super::unique_goal_matching(&goals, "cccc")
                .unwrap()
                .is_none()
        );
        assert_eq!(super::unique_goal_matching(&goals, "aaaa"), Err(2));

        assert_eq!(
            super::implicit_steer_target(&goals)
                .unwrap()
                .map(|goal| goal.id.as_str()),
            Some("bbbb3333"),
            "the running Goal wins"
        );

        // No running Goal: one open Goal is still unambiguous, two are not.
        let open = vec![
            goal_in_status("aaaa1111", GoalStatus::Paused),
            goal_in_status("aaaa2222", GoalStatus::Draft),
        ];
        assert_eq!(super::implicit_steer_target(&open), Err(2));
        let single = vec![goal_in_status("aaaa1111", GoalStatus::Draft)];
        assert_eq!(
            super::implicit_steer_target(&single)
                .unwrap()
                .map(|goal| goal.id.as_str()),
            Some("aaaa1111")
        );
        let none: Vec<tact::goal::Goal> = Vec::new();
        assert!(super::implicit_steer_target(&none).unwrap().is_none());
    }

    #[tokio::test]
    async fn steer_refuses_a_terminal_goal() {
        install_test_config();
        let (agent_tx, mut agent_rx) = tokio::sync::mpsc::unbounded_channel();
        let (mut agent, work_dir) = build_test_agent(MockClient::new(vec![]), Some(agent_tx));
        let manager = agent.tool_context.goal_manager.clone();
        let goal = manager
            .create(tact::goal::GoalCreate {
                title: "finished".into(),
                objective: "nothing left to steer".into(),
                criteria: vec![tact::goal::GoalCriterion::required("done")],
            })
            .await
            .unwrap();
        manager
            .update(
                &goal.id,
                goal.version,
                tact::goal::GoalUpdate {
                    status: Some(tact::goal::GoalStatus::Cancelled),
                    ..Default::default()
                },
                None,
                None,
            )
            .await
            .unwrap();

        super::handle_user_command(
            &mut agent,
            UserCommand::SteerGoal {
                goal_id: Some(goal.id.clone()),
                text: "this must not land".into(),
            },
            &work_dir,
        )
        .await;

        assert!(
            !manager
                .list_events(&goal.id)
                .await
                .unwrap()
                .iter()
                .any(|event| event.kind == GoalEventKind::Steering),
            "a terminal Goal cannot be steered"
        );
        assert!(
            std::iter::from_fn(|| agent_rx.try_recv().ok())
                .any(|update| matches!(update, AgentUpdate::Error(_))),
            "the refusal is reported"
        );
    }

    #[tokio::test]
    async fn create_goal_persists_starts_and_emits_full_snapshot() {
        install_test_config();
        let (agent_tx, mut agent_rx) = tokio::sync::mpsc::unbounded_channel();
        // Creation runs the Goal's first activation, so the mock has to be able
        // to answer a turn.
        let mock = MockClient::new(vec![(
            vec![text_block("working")],
            Some(StopReason::EndTurn),
        )]);
        let (mut agent, work_dir) = build_test_agent(mock, Some(agent_tx));
        let manager = agent.tool_context.goal_manager.clone();

        super::handle_user_command(
            &mut agent,
            UserCommand::CreateGoal {
                title: "Ship Goal UI".into(),
                objective: "Expose supervised goal execution".into(),
                criteria: vec!["User accepts the result".into()],
            },
            &work_dir,
        )
        .await;

        let goals = manager.list().await.unwrap();
        assert_eq!(goals.len(), 1);
        assert_eq!(goals[0].title, "Ship Goal UI");
        assert_eq!(goals[0].criteria.len(), 1);
        assert!(matches!(
            goals[0].criteria[0].verification_kind,
            tact::goal::GoalVerificationKind::Human
        ));
        // Creating is also starting: a Goal left in `draft` is one nothing ever
        // activates, which is exactly the dead end this pins down.
        assert_eq!(goals[0].status, tact::goal::GoalStatus::Running);

        let update = tokio::time::timeout(std::time::Duration::from_secs(1), agent_rx.recv())
            .await
            .expect("goal create update timed out")
            .expect("goal create update channel closed");
        assert!(matches!(
            update,
            AgentUpdate::GoalsChanged { goals }
                if goals.len() == 1 && goals[0].title == "Ship Goal UI"
        ));

        // The announcement carries the id: the panel row does not show it, and
        // every later command is addressed by it.
        let goal_id = goals[0].id.clone();
        let mut announced = false;
        while let Ok(Some(update)) =
            tokio::time::timeout(std::time::Duration::from_secs(1), agent_rx.recv()).await
        {
            if let AgentUpdate::Info(message) = update {
                announced = message.contains(&goal_id);
                break;
            }
        }
        assert!(announced, "goal creation must announce the new goal id");
    }

    /// An empty workspace must answer *something*: the sticky panel hides itself
    /// when it has nothing to show, so `/goal list` used to leave the screen
    /// completely unchanged.
    #[tokio::test]
    async fn list_goals_announces_an_empty_workspace() {
        install_test_config();
        let (agent_tx, mut agent_rx) = tokio::sync::mpsc::unbounded_channel();
        let (agent, _work_dir) = build_test_agent(MockClient::new(vec![]), Some(agent_tx));
        let events = agent.runtime.events.clone();

        // The command answers; the host is what turns an empty projection into a
        // log line, so this test covers both halves.
        let slot: std::sync::Arc<tokio::sync::Mutex<Option<tact::Agent>>> =
            std::sync::Arc::new(tokio::sync::Mutex::new(Some(agent)));
        let catalog = super::builtin_command_catalog(&slot, None, std::path::Path::new("/tmp"));
        let outcome = catalog
            .run_async(&tact::command::CommandInvocation {
                name: "goal".into(),
                args: "list".into(),
            })
            .await;
        super::forward_command_outcome(&outcome, &events);

        let first = tokio::time::timeout(std::time::Duration::from_secs(1), agent_rx.recv())
            .await
            .expect("empty goal list timed out")
            .expect("goal update channel closed");
        let empty_notice = match &first {
            AgentUpdate::Info(message) => message.contains("no goals"),
            _ => false,
        };
        assert!(
            empty_notice,
            "an empty list must say so in the log, got {first:?}"
        );

        let second = tokio::time::timeout(std::time::Duration::from_secs(1), agent_rx.recv())
            .await
            .expect("empty goal snapshot timed out")
            .expect("goal update channel closed");
        assert!(matches!(second, AgentUpdate::GoalsChanged { goals } if goals.is_empty()));
    }

    /// Creation now runs the Goal's first activation, which must happen off the
    /// command loop: `/goal list` (and `Cancel`) have to stay answerable while a
    /// model call is in flight.
    #[tokio::test(flavor = "multi_thread")]
    async fn create_goal_starts_a_run_without_blocking_the_command_loop() {
        use std::time::Duration;

        install_test_config();
        let (agent_tx, mut agent_rx) = tokio::sync::mpsc::unbounded_channel();
        let release = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let release_rx = release.clone();
        let mock = MockClient::with_responder(move |_request, _| {
            let deadline = std::time::Instant::now() + Duration::from_secs(2);
            while !release_rx.load(std::sync::atomic::Ordering::Relaxed)
                && std::time::Instant::now() < deadline
            {
                std::thread::sleep(Duration::from_millis(10));
            }
            Ok((
                vec![text_block("progress")],
                Some(StopReason::EndTurn),
                None,
            ))
        });
        let (agent, work_dir) = build_test_agent(mock, Some(agent_tx));
        let manager = agent.tool_context.goal_manager.clone();
        let (user_cmd_tx, user_cmd_rx) = tokio::sync::mpsc::unbounded_channel();
        let loop_handle = tokio::spawn(super::run_command_loop(agent, user_cmd_rx, work_dir));

        user_cmd_tx
            .send(UserCommand::CreateGoal {
                title: "Create starts".into(),
                objective: "Start the first activation at creation time".into(),
                criteria: Vec::new(),
            })
            .unwrap();

        // The announcement arrives before the activation blocks, so drain to it
        // and then ask for the list: that answer is what proves the loop is free.
        let mut announced = false;
        let drain_deadline = std::time::Instant::now() + Duration::from_millis(900);
        while std::time::Instant::now() < drain_deadline {
            match tokio::time::timeout(Duration::from_millis(200), agent_rx.recv()).await {
                Ok(Some(AgentUpdate::Info(message))) if message.contains("starting run") => {
                    announced = true;
                    break;
                }
                Ok(Some(_)) => continue,
                Ok(None) | Err(_) => break,
            }
        }
        assert!(announced, "creation must announce the started run");

        tokio::time::sleep(Duration::from_millis(100)).await;
        let start = std::time::Instant::now();
        user_cmd_tx
            .send(UserCommand::RunCommand {
                name: "goal".to_string(),
                args: "list".to_string(),
            })
            .unwrap();
        let mut answered = false;
        loop {
            match tokio::time::timeout(Duration::from_millis(300), agent_rx.recv()).await {
                Ok(Some(AgentUpdate::GoalsChanged { goals })) if goals.len() == 1 => {
                    answered = true;
                    break;
                }
                Ok(Some(_)) => continue,
                Ok(None) | Err(_) => break,
            }
        }
        assert!(answered, "expected a Goal snapshot during the activation");
        assert!(
            start.elapsed() < Duration::from_millis(450),
            "ListGoals must not await the creation activation"
        );

        // Persisted only after the activation claimed the lease, so this is
        // asserted once the run is provably in flight.
        let goals = manager.list().await.unwrap();
        assert_eq!(goals.len(), 1);
        assert_eq!(goals[0].status, tact::goal::GoalStatus::Running);
        assert_eq!(
            manager.list_running_runs().await.unwrap().len(),
            1,
            "the created Goal must have an active run"
        );

        release.store(true, std::sync::atomic::Ordering::Relaxed);

        // The activation now finishes. It does not go through `SubmitTask`, so
        // it must explicitly announce that it left the TUI's busy state —
        // otherwise a message typed during the run stays queued forever.
        let mut idle = false;
        let idle_deadline = std::time::Instant::now() + Duration::from_secs(2);
        while std::time::Instant::now() < idle_deadline {
            match tokio::time::timeout(Duration::from_millis(200), agent_rx.recv()).await {
                Ok(Some(AgentUpdate::TaskIdle)) => {
                    idle = true;
                    break;
                }
                Ok(Some(_)) => continue,
                Ok(None) | Err(_) => break,
            }
        }
        assert!(idle, "a goal activation must emit TaskIdle when it ends");

        drop(user_cmd_tx);
        tokio::time::timeout(Duration::from_secs(5), loop_handle)
            .await
            .expect("command loop must finish")
            .expect("command loop join failed");
    }

    #[tokio::test]
    async fn steer_pause_and_cancel_goal_are_durable_before_snapshot() {
        install_test_config();
        let (agent_tx, mut agent_rx) = tokio::sync::mpsc::unbounded_channel();
        let (mut agent, work_dir) = build_test_agent(MockClient::new(vec![]), Some(agent_tx));
        let manager = agent.tool_context.goal_manager.clone();
        let created = manager
            .create(tact::goal::GoalCreate {
                title: "Durable lifecycle".into(),
                objective: "Persist control commands".into(),
                criteria: vec![tact::goal::GoalCriterion::required("done")],
            })
            .await
            .unwrap();
        manager
            .update(
                &created.id,
                created.version,
                tact::goal::GoalUpdate {
                    status: Some(tact::goal::GoalStatus::Queued),
                    ..Default::default()
                },
                None,
                None,
            )
            .await
            .unwrap();

        super::handle_user_command(
            &mut agent,
            UserCommand::SteerGoal {
                goal_id: Some(created.id.clone()),
                text: "Prefer a focused diff".into(),
            },
            &work_dir,
        )
        .await;
        assert!(
            manager
                .list_events(&created.id)
                .await
                .unwrap()
                .iter()
                .any(|event| event.kind == GoalEventKind::Steering
                    && event.payload.as_deref() == Some("Prefer a focused diff"))
        );

        // No id: the store resolves the single open Goal (the panel snapshot
        // may not have caught up).
        super::handle_user_command(
            &mut agent,
            UserCommand::SteerGoal {
                goal_id: None,
                text: "Keep the diff focused".into(),
            },
            &work_dir,
        )
        .await;
        assert!(
            manager
                .list_events(&created.id)
                .await
                .unwrap()
                .iter()
                .any(|event| event.kind == GoalEventKind::Steering
                    && event.payload.as_deref() == Some("Keep the diff focused")),
            "an id-less steer must land on the only open Goal"
        );

        // An id the TUI could not resolve (empty/stale snapshot) is still
        // honored when it leads the raw text.
        let text = format!("{} Prefer fewer files", created.id);
        super::handle_user_command(
            &mut agent,
            UserCommand::SteerGoal {
                goal_id: None,
                text,
            },
            &work_dir,
        )
        .await;
        assert!(
            manager
                .list_events(&created.id)
                .await
                .unwrap()
                .iter()
                .any(|event| event.kind == GoalEventKind::Steering
                    && event.payload.as_deref() == Some("Prefer fewer files")),
            "a leading id in the raw text must be resolved by the store"
        );

        // An id with no instruction must not be replayed as the instruction.
        let steered_before = manager
            .list_events(&created.id)
            .await
            .unwrap()
            .iter()
            .filter(|event| event.kind == GoalEventKind::Steering)
            .count();
        super::handle_user_command(
            &mut agent,
            UserCommand::SteerGoal {
                goal_id: None,
                text: created.id.clone(),
            },
            &work_dir,
        )
        .await;
        let steered_after = manager
            .list_events(&created.id)
            .await
            .unwrap()
            .iter()
            .filter(|event| event.kind == GoalEventKind::Steering)
            .count();
        assert_eq!(
            steered_after, steered_before,
            "an id without an instruction must not append a steering event"
        );
        assert!(
            std::iter::from_fn(|| agent_rx.try_recv().ok())
                .any(|update| matches!(update, AgentUpdate::Error(_))),
            "the refused steer must be reported"
        );

        super::handle_user_command(
            &mut agent,
            UserCommand::PauseGoal {
                goal_id: created.id.clone(),
            },
            &work_dir,
        )
        .await;
        assert_eq!(
            manager.get(&created.id).await.unwrap().status,
            tact::goal::GoalStatus::Paused
        );

        super::handle_user_command(
            &mut agent,
            UserCommand::CancelGoal {
                goal_id: created.id.clone(),
            },
            &work_dir,
        )
        .await;
        assert_eq!(
            manager.get(&created.id).await.unwrap().status,
            tact::goal::GoalStatus::Cancelled
        );
        let mut saw_cancelled = false;
        while let Ok(update) = agent_rx.try_recv() {
            if matches!(
                update,
                AgentUpdate::GoalsChanged { goals }
                    if goals.iter().any(|goal| goal.id == created.id
                        && goal.status == tact_protocol::GoalStatusSnapshot::Cancelled)
            ) {
                saw_cancelled = true;
            }
        }
        assert!(saw_cancelled);
    }

    /// The panel is the reason archiving exists: a finished Goal that has been
    /// filed away must stop taking rows there, while `/goal list --all` still
    /// finds it and `/goal show` still reports how it finished.
    #[tokio::test]
    async fn archiving_hides_a_finished_goal_until_list_all_asks_for_it() {
        install_test_config();
        let (agent_tx, mut agent_rx) = tokio::sync::mpsc::unbounded_channel();
        let (mut agent, work_dir) = build_test_agent(MockClient::new(vec![]), Some(agent_tx));
        let manager = agent.tool_context.goal_manager.clone();
        let created = manager
            .create(tact::goal::GoalCreate {
                title: "Filed away".into(),
                objective: "Stop taking panel space".into(),
                criteria: vec![tact::goal::GoalCriterion::required("done")],
            })
            .await
            .unwrap();
        manager
            .update(
                &created.id,
                created.version,
                tact::goal::GoalUpdate {
                    status: Some(tact::goal::GoalStatus::Cancelled),
                    ..Default::default()
                },
                None,
                None,
            )
            .await
            .unwrap();

        let cancelled = manager.get(&created.id).await.unwrap();
        assert_eq!(cancelled.status, tact::goal::GoalStatus::Cancelled);

        super::handle_user_command(
            &mut agent,
            UserCommand::SetGoalArchived {
                goal_id: created.id.clone(),
                archived: true,
            },
            &work_dir,
        )
        .await;

        let stored = manager.get(&created.id).await.unwrap();
        assert!(stored.is_archived(), "the archive is persisted");
        assert_eq!(
            stored.status,
            tact::goal::GoalStatus::Cancelled,
            "archiving does not rewrite the outcome"
        );
        assert!(
            manager
                .list_events(&created.id)
                .await
                .unwrap()
                .iter()
                .any(|event| event.kind == GoalEventKind::Archived),
            "the user action is in the Goal's history"
        );

        let mut saw_empty = false;
        let mut announced = false;
        while let Ok(update) = agent_rx.try_recv() {
            match update {
                AgentUpdate::GoalsChanged { goals } if goals.is_empty() => saw_empty = true,
                AgentUpdate::Info(message) if message.contains("archived") => announced = true,
                _ => {}
            }
        }
        assert!(saw_empty, "the panel snapshot drops the archived Goal");
        assert!(announced, "the archive is announced in the log");

        // `--all` is how it stays reachable: the goal is back in the snapshot
        // and marked as archived.
        super::handle_user_command(
            &mut agent,
            UserCommand::RunCommand {
                name: "goal".to_string(),
                args: "list --all".to_string(),
            },
            &work_dir,
        )
        .await;
        let mut listed = None;
        while let Ok(update) = agent_rx.try_recv() {
            if let AgentUpdate::GoalsChanged { goals } = update {
                listed = Some(goals);
            }
        }
        let listed = listed.expect("a snapshot for --all");
        assert_eq!(listed.len(), 1);
        assert!(listed[0].archived);
        assert_eq!(listed[0].id, created.id);

        // ... and `show` says so, instead of pretending the Goal is live.
        super::handle_user_command(
            &mut agent,
            UserCommand::RunCommand {
                name: "goal".to_string(),
                args: format!("show {}", created.id),
            },
            &work_dir,
        )
        .await;
        let mut shown = None;
        while let Ok(update) = agent_rx.try_recv() {
            if let AgentUpdate::PopupMarkdown { source, .. } = update {
                shown = Some(source);
            }
        }
        assert!(
            shown
                .as_deref()
                .is_some_and(|source| source.contains("**Archived:** yes")),
            "show must say the Goal is archived: {shown:?}"
        );
    }

    /// Archiving running work would take it off the panel while it kept going.
    #[tokio::test]
    async fn archiving_an_unfinished_goal_is_refused_with_a_reason() {
        install_test_config();
        let (agent_tx, mut agent_rx) = tokio::sync::mpsc::unbounded_channel();
        let (mut agent, work_dir) = build_test_agent(MockClient::new(vec![]), Some(agent_tx));
        let manager = agent.tool_context.goal_manager.clone();
        let created = manager
            .create(tact::goal::GoalCreate {
                title: "Still running".into(),
                objective: "Stay on the panel".into(),
                criteria: vec![tact::goal::GoalCriterion::required("done")],
            })
            .await
            .unwrap();

        super::handle_user_command(
            &mut agent,
            UserCommand::SetGoalArchived {
                goal_id: created.id.clone(),
                archived: true,
            },
            &work_dir,
        )
        .await;

        assert!(!manager.get(&created.id).await.unwrap().is_archived());
        let mut error = None;
        while let Ok(update) = agent_rx.try_recv() {
            if let AgentUpdate::Error(tact_protocol::AgentErrorKind::Other(message)) = update {
                error = Some(message);
            }
        }
        assert!(
            error
                .as_deref()
                .is_some_and(|message| message.contains("only a finished Goal can be archived")),
            "the refusal has to name the rule: {error:?}"
        );
    }

    #[tokio::test]
    async fn accept_goal_records_human_evidence_and_succeeds_only_after_verification() {
        install_test_config();
        let (agent_tx, mut agent_rx) = tokio::sync::mpsc::unbounded_channel();
        let (mut agent, work_dir) = build_test_agent(MockClient::new(vec![]), Some(agent_tx));
        let manager = agent.tool_context.goal_manager.clone();
        let mut goal = manager
            .create(tact::goal::GoalCreate {
                title: "Human acceptance".into(),
                objective: "Require explicit review".into(),
                criteria: vec![tact::goal::GoalCriterion::new(
                    "User accepts the result",
                    true,
                    tact::goal::GoalVerificationKind::Human,
                )],
            })
            .await
            .unwrap();
        for status in [
            tact::goal::GoalStatus::Queued,
            tact::goal::GoalStatus::Running,
            tact::goal::GoalStatus::WaitingUser,
        ] {
            goal = manager
                .update(
                    &goal.id,
                    goal.version,
                    tact::goal::GoalUpdate {
                        status: Some(status),
                        ..Default::default()
                    },
                    None,
                    None,
                )
                .await
                .unwrap();
        }

        super::handle_user_command(
            &mut agent,
            UserCommand::AcceptGoal {
                goal_id: goal.id.clone(),
                text: "Accepted after manual review".into(),
            },
            &work_dir,
        )
        .await;

        let accepted = manager.get(&goal.id).await.unwrap();
        assert_eq!(accepted.status, tact::goal::GoalStatus::Succeeded);
        assert!(accepted.criteria[0].has_valid_evidence(accepted.criteria_version));
        assert!(
            manager
                .list_events(&goal.id)
                .await
                .unwrap()
                .iter()
                .any(|event| event.kind == GoalEventKind::UserAccepted)
        );
        assert!(
            std::iter::from_fn(|| agent_rx.try_recv().ok()).any(|update| matches!(
                update,
                AgentUpdate::Info(message) if message.contains("accepted — succeeded")
            )),
            "a successful acceptance must be announced in the log"
        );
    }

    /// A Goal that asked a question is answered by the very next chat message:
    /// the text becomes the answer and the Goal resumes without `/goal resume`.
    #[tokio::test]
    async fn a_reply_answers_a_waiting_goal_and_resumes_it() {
        install_test_config();
        let mock = MockClient::new(vec![(
            vec![text_block("noted; continuing")],
            Some(StopReason::EndTurn),
        )]);
        let (agent_tx, mut agent_rx) = tokio::sync::mpsc::unbounded_channel();
        let (mut agent, _work_dir) = build_test_agent(mock, Some(agent_tx));
        let manager = agent.tool_context.goal_manager.clone();
        let mut goal = manager
            .create(tact::goal::GoalCreate {
                title: "asker".into(),
                objective: "ask, then continue".into(),
                criteria: Vec::new(),
            })
            .await
            .unwrap();
        for status in [
            tact::goal::GoalStatus::Queued,
            tact::goal::GoalStatus::Running,
            tact::goal::GoalStatus::WaitingUser,
        ] {
            goal = manager
                .update(
                    &goal.id,
                    goal.version,
                    tact::goal::GoalUpdate {
                        status: Some(status),
                        ..Default::default()
                    },
                    None,
                    None,
                )
                .await
                .unwrap();
        }
        manager
            .append_event(super::goal_event(
                &goal,
                GoalEventKind::NeedsUser,
                None,
                "which database should I target?".into(),
                tact::goal::GoalEventSource::Model,
            ))
            .await
            .unwrap();

        let answer_for = super::goal_awaiting_answer(&manager)
            .await
            .expect("the goal asked a question, so the reply answers it");
        assert_eq!(answer_for, goal.id);
        super::answer_goal(&mut agent, &answer_for, "use postgres").await;

        let events = manager.list_events(&goal.id).await.unwrap();
        assert!(
            events
                .iter()
                .any(|event| event.kind == GoalEventKind::UserAnswer
                    && event.payload.as_deref() == Some("use postgres")),
            "the reply is recorded as the goal's answer: {events:?}"
        );
        assert_ne!(
            manager.get(&goal.id).await.unwrap().status,
            tact::goal::GoalStatus::WaitingUser,
            "the reply resumed the Goal instead of leaving it stranded"
        );
        assert!(
            std::iter::from_fn(|| agent_rx.try_recv().ok()).any(|update| matches!(
                update,
                AgentUpdate::Info(message) if message.contains("was waiting for input")
            )),
            "the log explains why the message went to the Goal"
        );
    }

    /// A Goal waiting on human acceptance is not waiting for a chat reply:
    /// Two Goals waiting on questions are ambiguous, so the driver must not
    /// guess which one a message answers.
    #[tokio::test]
    async fn two_waiting_goals_make_a_reply_ambiguous() {
        install_test_config();
        let (agent_tx, _agent_rx) = tokio::sync::mpsc::unbounded_channel();
        let (agent, _work_dir) = build_test_agent(MockClient::new(vec![]), Some(agent_tx));
        let manager = agent.tool_context.goal_manager.clone();

        for title in ["first", "second"] {
            let mut goal = manager
                .create(tact::goal::GoalCreate {
                    title: title.into(),
                    objective: format!("{title} asks"),
                    criteria: Vec::new(),
                })
                .await
                .unwrap();
            for status in [
                tact::goal::GoalStatus::Queued,
                tact::goal::GoalStatus::Running,
                tact::goal::GoalStatus::WaitingUser,
            ] {
                goal = manager
                    .update(
                        &goal.id,
                        goal.version,
                        tact::goal::GoalUpdate {
                            status: Some(status),
                            ..Default::default()
                        },
                        None,
                        None,
                    )
                    .await
                    .unwrap();
            }
            manager
                .append_event(super::goal_event(
                    &goal,
                    GoalEventKind::NeedsUser,
                    None,
                    "which one?".into(),
                    tact::goal::GoalEventSource::Model,
                ))
                .await
                .unwrap();
        }

        assert!(
            super::goal_awaiting_answer(&manager).await.is_none(),
            "the driver must not guess which Goal a message answers"
        );
    }

    /// A Goal waiting on human acceptance is not waiting for a chat reply:
    /// only `/goal accept` writes that evidence, so a message must not be
    /// swallowed as an "answer".
    #[tokio::test]
    async fn a_reply_is_not_swallowed_by_a_goal_waiting_for_acceptance() {
        install_test_config();
        let mock = MockClient::new(vec![(vec![text_block("sure")], Some(StopReason::EndTurn))]);
        let (agent_tx, _agent_rx) = tokio::sync::mpsc::unbounded_channel();
        let (mut agent, work_dir) = build_test_agent(mock, Some(agent_tx));
        let manager = agent.tool_context.goal_manager.clone();
        let mut goal = manager
            .create(tact::goal::GoalCreate {
                title: "gated".into(),
                objective: "wait for acceptance".into(),
                criteria: vec![tact::goal::GoalCriterion::new(
                    "User accepts the result",
                    true,
                    tact::goal::GoalVerificationKind::Human,
                )],
            })
            .await
            .unwrap();
        for status in [
            tact::goal::GoalStatus::Queued,
            tact::goal::GoalStatus::Running,
            tact::goal::GoalStatus::WaitingUser,
        ] {
            goal = manager
                .update(
                    &goal.id,
                    goal.version,
                    tact::goal::GoalUpdate {
                        status: Some(status),
                        ..Default::default()
                    },
                    None,
                    None,
                )
                .await
                .unwrap();
        }

        assert!(
            super::goal_awaiting_answer(&manager).await.is_none(),
            "an acceptance wait is not a question"
        );
        // What the command loop does for a non-answer message: a normal turn.
        super::handle_user_command(
            &mut agent,
            UserCommand::SubmitTask("hello".into()),
            &work_dir,
        )
        .await;

        let events = manager.list_events(&goal.id).await.unwrap();
        assert!(
            !events
                .iter()
                .any(|event| event.kind == GoalEventKind::UserAnswer),
            "an acceptance wait is not a question: {events:?}"
        );
        assert_eq!(
            manager.get(&goal.id).await.unwrap().status,
            tact::goal::GoalStatus::WaitingUser,
            "the Goal keeps waiting for /goal accept"
        );
    }

    /// A message sent while the Goal was `waiting_user` can wake the runner
    /// and strand the Goal in `running` with no active run. Acceptance is a
    /// human decision, so the explicit command must still work there.
    #[tokio::test]
    async fn accept_goal_accepts_a_stranded_running_goal_with_no_active_run() {
        install_test_config();
        let (agent_tx, _agent_rx) = tokio::sync::mpsc::unbounded_channel();
        let (mut agent, work_dir) = build_test_agent(MockClient::new(vec![]), Some(agent_tx));
        let manager = agent.tool_context.goal_manager.clone();
        let mut goal = manager
            .create(tact::goal::GoalCreate {
                title: "Stranded acceptance".into(),
                objective: "Accept after a no-op wake".into(),
                criteria: vec![tact::goal::GoalCriterion::new(
                    "User accepts the result",
                    true,
                    tact::goal::GoalVerificationKind::Human,
                )],
            })
            .await
            .unwrap();
        for status in [
            tact::goal::GoalStatus::Queued,
            tact::goal::GoalStatus::Running,
        ] {
            goal = manager
                .update(
                    &goal.id,
                    goal.version,
                    tact::goal::GoalUpdate {
                        status: Some(status),
                        ..Default::default()
                    },
                    None,
                    None,
                )
                .await
                .unwrap();
        }

        super::handle_user_command(
            &mut agent,
            UserCommand::AcceptGoal {
                goal_id: goal.id.clone(),
                text: "Accepted after the commit".into(),
            },
            &work_dir,
        )
        .await;

        let accepted = manager.get(&goal.id).await.unwrap();
        assert_eq!(accepted.status, tact::goal::GoalStatus::Succeeded);
        assert!(accepted.criteria[0].has_valid_evidence(accepted.criteria_version));
    }

    /// Crash recovery parks a Goal in `paused`; accepting it is still a human
    /// decision, and the evidence path re-enters `running` before verifying.
    #[tokio::test]
    async fn accept_goal_accepts_a_recovered_paused_goal() {
        install_test_config();
        let (agent_tx, _agent_rx) = tokio::sync::mpsc::unbounded_channel();
        let (mut agent, work_dir) = build_test_agent(MockClient::new(vec![]), Some(agent_tx));
        let manager = agent.tool_context.goal_manager.clone();
        let mut goal = manager
            .create(tact::goal::GoalCreate {
                title: "Recovered pause".into(),
                objective: "Accept after recovery".into(),
                criteria: vec![tact::goal::GoalCriterion::new(
                    "User accepts the result",
                    true,
                    tact::goal::GoalVerificationKind::Human,
                )],
            })
            .await
            .unwrap();
        for status in [
            tact::goal::GoalStatus::Queued,
            tact::goal::GoalStatus::Running,
            tact::goal::GoalStatus::Paused,
        ] {
            goal = manager
                .update(
                    &goal.id,
                    goal.version,
                    tact::goal::GoalUpdate {
                        status: Some(status),
                        ..Default::default()
                    },
                    None,
                    None,
                )
                .await
                .unwrap();
        }

        super::handle_user_command(
            &mut agent,
            UserCommand::AcceptGoal {
                goal_id: goal.id.clone(),
                text: "Accepted after recovery".into(),
            },
            &work_dir,
        )
        .await;

        let accepted = manager.get(&goal.id).await.unwrap();
        assert_eq!(accepted.status, tact::goal::GoalStatus::Succeeded);
        assert!(accepted.criteria[0].has_valid_evidence(accepted.criteria_version));
    }

    /// An activation in flight owns the Goal's status; acceptance must wait.
    #[tokio::test]
    async fn accept_goal_refuses_while_an_activation_is_in_flight() {
        install_test_config();
        let (agent_tx, mut agent_rx) = tokio::sync::mpsc::unbounded_channel();
        let (mut agent, work_dir) = build_test_agent(MockClient::new(vec![]), Some(agent_tx));
        let manager = agent.tool_context.goal_manager.clone();
        let mut goal = manager
            .create(tact::goal::GoalCreate {
                title: "Busy acceptance".into(),
                objective: "Do not race the runner".into(),
                criteria: vec![tact::goal::GoalCriterion::new(
                    "User accepts the result",
                    true,
                    tact::goal::GoalVerificationKind::Human,
                )],
            })
            .await
            .unwrap();
        for status in [
            tact::goal::GoalStatus::Queued,
            tact::goal::GoalStatus::Running,
        ] {
            goal = manager
                .update(
                    &goal.id,
                    goal.version,
                    tact::goal::GoalUpdate {
                        status: Some(status),
                        ..Default::default()
                    },
                    None,
                    None,
                )
                .await
                .unwrap();
        }
        manager
            .start_run(
                "test-runner",
                std::time::Duration::from_secs(60),
                tact::goal::GoalRun::new(goal.id.clone()),
            )
            .await
            .unwrap();

        super::handle_user_command(
            &mut agent,
            UserCommand::AcceptGoal {
                goal_id: goal.id.clone(),
                text: "Too early".into(),
            },
            &work_dir,
        )
        .await;

        let stored = manager.get(&goal.id).await.unwrap();
        assert_eq!(stored.status, tact::goal::GoalStatus::Running);
        assert!(!stored.criteria[0].has_valid_evidence(stored.criteria_version));
        assert!(
            std::iter::from_fn(|| agent_rx.try_recv().ok())
                .any(|update| matches!(update, AgentUpdate::Error(_))),
            "the refusal must be reported"
        );
    }

    #[tokio::test]
    async fn wait_reason_prefers_the_goal_question() {
        install_test_config();
        let (agent_tx, _agent_rx) = tokio::sync::mpsc::unbounded_channel();
        let (agent, _work_dir) = build_test_agent(MockClient::new(vec![]), Some(agent_tx));
        let manager = agent.tool_context.goal_manager.clone();
        let task_manager = agent.tool_context.task_manager.clone();
        let mut goal = manager
            .create(tact::goal::GoalCreate {
                title: "ask a question".into(),
                objective: "surface the question in the panel".into(),
                criteria: vec![tact::goal::GoalCriterion::required("done")],
            })
            .await
            .unwrap();
        for status in [
            tact::goal::GoalStatus::Queued,
            tact::goal::GoalStatus::Running,
        ] {
            goal = manager
                .update(
                    &goal.id,
                    goal.version,
                    tact::goal::GoalUpdate {
                        status: Some(status),
                        ..Default::default()
                    },
                    None,
                    None,
                )
                .await
                .unwrap();
        }
        manager
            .append_event(super::goal_event(
                &goal,
                GoalEventKind::NeedsUser,
                None,
                "Which path should I take?".into(),
                tact::goal::GoalEventSource::Model,
            ))
            .await
            .unwrap();
        manager
            .update(
                &goal.id,
                goal.version,
                tact::goal::GoalUpdate {
                    status: Some(tact::goal::GoalStatus::WaitingUser),
                    ..Default::default()
                },
                Some(super::goal_event(
                    &goal,
                    GoalEventKind::StatusChanged,
                    Some(tact::goal::GoalStatus::WaitingUser),
                    "run ended as NeedsUser".into(),
                    tact::goal::GoalEventSource::Runner,
                )),
                None,
            )
            .await
            .unwrap();

        let goals = super::goal_snapshots(&manager, &task_manager, super::goal_limits(None))
            .await
            .unwrap();
        let snapshot = goals
            .iter()
            .find(|snapshot| snapshot.id == goal.id)
            .unwrap();
        assert_eq!(
            snapshot.wait_reason.as_deref(),
            Some("Which path should I take?"),
            "the panel must show the question, not the generic NeedsUser line"
        );
    }

    #[tokio::test]
    async fn unknown_goal_command_emits_an_error() {
        install_test_config();
        let (agent_tx, mut agent_rx) = tokio::sync::mpsc::unbounded_channel();
        let (mut agent, work_dir) = build_test_agent(MockClient::new(vec![]), Some(agent_tx));

        super::handle_user_command(
            &mut agent,
            UserCommand::PauseGoal {
                goal_id: "missing-goal".into(),
            },
            &work_dir,
        )
        .await;

        let update = tokio::time::timeout(std::time::Duration::from_secs(1), agent_rx.recv())
            .await
            .expect("goal error timed out")
            .expect("goal error channel closed");
        assert!(matches!(
            update,
            AgentUpdate::Error(error) if error.to_string().contains("missing-goal")
        ));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn goal_list_and_pause_respond_while_resume_runs() {
        use std::time::Duration;

        install_test_config();
        let (agent_tx, mut agent_rx) = tokio::sync::mpsc::unbounded_channel();
        let release = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let release_rx = release.clone();
        let mock = MockClient::with_responder(move |_request, _| {
            let deadline = std::time::Instant::now() + Duration::from_secs(2);
            while !release_rx.load(std::sync::atomic::Ordering::Relaxed)
                && std::time::Instant::now() < deadline
            {
                std::thread::sleep(Duration::from_millis(10));
            }
            Ok((
                vec![text_block("progress")],
                Some(StopReason::EndTurn),
                None,
            ))
        });
        let (agent, work_dir) = build_test_agent(mock, Some(agent_tx));
        let manager = agent.tool_context.goal_manager.clone();
        let mut goal = manager
            .create(tact::goal::GoalCreate {
                title: "Concurrent control".into(),
                objective: "Keep reads responsive".into(),
                criteria: vec![tact::goal::GoalCriterion::required("done")],
            })
            .await
            .unwrap();
        for status in [
            tact::goal::GoalStatus::Queued,
            tact::goal::GoalStatus::Paused,
        ] {
            goal = manager
                .update(
                    &goal.id,
                    goal.version,
                    tact::goal::GoalUpdate {
                        status: Some(status),
                        ..Default::default()
                    },
                    None,
                    None,
                )
                .await
                .unwrap();
        }
        let goal_id = goal.id.clone();
        let (user_cmd_tx, user_cmd_rx) = tokio::sync::mpsc::unbounded_channel();
        let loop_handle = tokio::spawn(super::run_command_loop(agent, user_cmd_rx, work_dir));

        user_cmd_tx
            .send(UserCommand::ResumeGoal {
                goal_id: goal_id.clone(),
            })
            .unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        let start = std::time::Instant::now();
        user_cmd_tx
            .send(UserCommand::RunCommand {
                name: "goal".to_string(),
                args: "list".to_string(),
            })
            .unwrap();

        let mut saw_running_snapshot = false;
        loop {
            match tokio::time::timeout(Duration::from_millis(300), agent_rx.recv()).await {
                Ok(Some(AgentUpdate::GoalsChanged { goals }))
                    if goals.iter().any(|goal| goal.id == goal_id) =>
                {
                    saw_running_snapshot = true;
                    break;
                }
                Ok(Some(_)) => continue,
                Ok(None) | Err(_) => break,
            }
        }
        assert!(
            saw_running_snapshot,
            "expected Goal snapshot during active run"
        );
        assert!(
            start.elapsed() < Duration::from_millis(450),
            "ListGoals must not await the in-flight Goal run"
        );

        user_cmd_tx
            .send(UserCommand::PauseGoal {
                goal_id: goal_id.clone(),
            })
            .unwrap();
        // The pause request must be processed *while* the run is active. The
        // mock is still blocked here, so observing the durable request proves
        // the command loop did not await the run handle.
        let pause_deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            if manager
                .list_events(&goal_id)
                .await
                .unwrap()
                .iter()
                .any(|event| event.kind == GoalEventKind::PauseRequested)
            {
                break;
            }
            assert!(
                std::time::Instant::now() < pause_deadline,
                "pause request was not processed during the active run"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        release.store(true, std::sync::atomic::Ordering::Relaxed);
        drop(user_cmd_tx);
        tokio::time::timeout(Duration::from_secs(5), loop_handle)
            .await
            .expect("command loop must finish")
            .expect("command loop join failed");
        assert_eq!(
            manager.get(&goal_id).await.unwrap().status,
            tact::goal::GoalStatus::Paused
        );
    }

    #[tokio::test]
    async fn cancel_sets_flag_and_emits_info() {
        install_test_config();
        let (agent_tx, mut agent_rx) = tokio::sync::mpsc::unbounded_channel();
        let (agent, _) = build_test_agent(MockClient::new(vec![]), Some(agent_tx));

        agent.runtime.cancel_flag.store(true, Ordering::Relaxed);
        agent.emit_update(AgentUpdate::Info("Cancelling...".into()));

        assert!(agent.runtime.cancel_flag.load(Ordering::Relaxed));
        let update = agent_rx.try_recv().expect("expected Cancelling info");
        assert!(matches!(update, AgentUpdate::Info(msg) if msg.contains("Cancelling")));
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
            if matches!(update, AgentUpdate::TaskComplete(_)) {
                saw_complete = true;
            }
        }
        assert!(saw_complete, "SubmitTask should clear cancel and complete");
    }

    async fn paused_goal_with_one_required_criterion(agent: &mut tact::Agent) -> String {
        let manager = agent.tool_context.goal_manager.clone();
        let created = manager
            .create(tact::goal::GoalCreate {
                title: "Resume after a stale cancel".into(),
                objective: "A pressed Esc must not kill the goal".into(),
                criteria: vec![tact::goal::GoalCriterion::required("done")],
            })
            .await
            .unwrap();
        manager
            .update(
                &created.id,
                created.version,
                tact::goal::GoalUpdate {
                    status: Some(tact::goal::GoalStatus::Queued),
                    ..Default::default()
                },
                None,
                None,
            )
            .await
            .unwrap();
        let queued = manager.get(&created.id).await.unwrap();
        manager
            .update(
                &created.id,
                queued.version,
                tact::goal::GoalUpdate {
                    status: Some(tact::goal::GoalStatus::Paused),
                    ..Default::default()
                },
                None,
                None,
            )
            .await
            .unwrap();
        created.id
    }

    #[tokio::test]
    async fn resume_goal_clears_a_stale_cancel_flag_before_activating() {
        install_test_config();
        let (agent_tx, _agent_rx) = tokio::sync::mpsc::unbounded_channel();
        // Resume always activates, so the mock needs a scripted turn: an empty
        // script panics on the first model call (`idx % 0`).
        let mock = MockClient::new(vec![(vec![text_block("done")], Some(StopReason::EndTurn))]);
        let (mut agent, work_dir) = build_test_agent(mock, Some(agent_tx));
        let manager = agent.tool_context.goal_manager.clone();
        let goal_id = paused_goal_with_one_required_criterion(&mut agent).await;

        agent.runtime.cancel_flag.store(true, Ordering::Relaxed);
        super::handle_user_command(
            &mut agent,
            UserCommand::ResumeGoal {
                goal_id: goal_id.clone(),
            },
            &work_dir,
        )
        .await;

        assert!(
            !agent.runtime.cancel_flag.load(Ordering::Relaxed),
            "resuming a Goal must clear a stale cancel flag"
        );
        assert_ne!(
            manager.get(&goal_id).await.unwrap().status,
            tact::goal::GoalStatus::Cancelled,
            "a stale cancel flag must not destroy the Goal on resume"
        );
    }

    #[tokio::test]
    async fn resume_rolls_back_to_paused_when_the_lease_is_held() {
        install_test_config();
        let (agent_tx, _agent_rx) = tokio::sync::mpsc::unbounded_channel();
        let (mut agent, work_dir) = build_test_agent(MockClient::new(vec![]), Some(agent_tx));
        let manager = agent.tool_context.goal_manager.clone();
        let goal_id = paused_goal_with_one_required_criterion(&mut agent).await;
        let held = manager
            .claim_lease("other-live-runner", std::time::Duration::from_secs(60))
            .await
            .unwrap();

        super::handle_user_command(
            &mut agent,
            UserCommand::ResumeGoal {
                goal_id: goal_id.clone(),
            },
            &work_dir,
        )
        .await;

        assert_eq!(
            manager.get(&goal_id).await.unwrap().status,
            tact::goal::GoalStatus::Paused,
            "a resume that cannot claim a lease must not strand the Goal in Running"
        );
        manager.release_lease(&held).await.unwrap();
    }

    #[tokio::test]
    async fn accept_goal_ignores_a_stale_cancel_flag_and_succeeds() {
        install_test_config();
        let (agent_tx, _agent_rx) = tokio::sync::mpsc::unbounded_channel();
        let mock = MockClient::new(vec![]);
        let (mut agent, work_dir) = build_test_agent(mock, Some(agent_tx));
        let manager = agent.tool_context.goal_manager.clone();
        // A pending required human criterion. The acceptance is a human
        // decision; a stale cancel flag from an unrelated turn must not turn it
        // into a cancelled Goal.
        let created = manager
            .create(tact::goal::GoalCreate {
                title: "Acceptance continues".into(),
                objective: "Human text must not leave the goal cancelled".into(),
                criteria: vec![tact::goal::GoalCriterion::new(
                    "reviewed by a person",
                    true,
                    tact::goal::GoalVerificationKind::Human,
                )],
            })
            .await
            .unwrap();
        manager
            .update(
                &created.id,
                created.version,
                tact::goal::GoalUpdate {
                    status: Some(tact::goal::GoalStatus::Queued),
                    ..Default::default()
                },
                None,
                None,
            )
            .await
            .unwrap();
        let queued = manager.get(&created.id).await.unwrap();
        manager
            .update(
                &created.id,
                queued.version,
                tact::goal::GoalUpdate {
                    status: Some(tact::goal::GoalStatus::Running),
                    ..Default::default()
                },
                None,
                None,
            )
            .await
            .unwrap();
        let running = manager.get(&created.id).await.unwrap();
        manager
            .update(
                &created.id,
                running.version,
                tact::goal::GoalUpdate {
                    status: Some(tact::goal::GoalStatus::WaitingUser),
                    ..Default::default()
                },
                None,
                None,
            )
            .await
            .unwrap();

        agent.runtime.cancel_flag.store(true, Ordering::Relaxed);
        super::handle_user_command(
            &mut agent,
            UserCommand::AcceptGoal {
                goal_id: created.id.clone(),
                text: "looks good".into(),
            },
            &work_dir,
        )
        .await;

        assert_eq!(
            manager.get(&created.id).await.unwrap().status,
            tact::goal::GoalStatus::Succeeded,
            "a stale cancel flag must not stop a human acceptance"
        );
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
            if let AgentUpdate::ModelInfo(params) = update {
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
            if let AgentUpdate::ModelInfo(params) = update {
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

        super::handle_user_command(
            &mut agent,
            UserCommand::RunCommand {
                name: "background".to_string(),
                args: String::new(),
            },
            &work_dir,
        )
        .await;

        let mut popup = None;
        while let Ok(update) = agent_rx.try_recv() {
            match update {
                AgentUpdate::PopupMarkdown { title, source } => popup = Some((title, source)),
                // The listing is a read-out: it must not land in the transcript.
                AgentUpdate::MdInfo(md) => panic!("background listing must not be MdInfo: {md}"),
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
            UserCommand::RunCommand {
                name: "background".to_string(),
                args: "deadbeef".to_string(),
            },
            &work_dir,
        )
        .await;

        let mut saw_error = false;
        while let Ok(update) = agent_rx.try_recv() {
            if let AgentUpdate::Error(err) = update {
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
        let (agent_tx, _agent_rx) = tokio::sync::mpsc::unbounded_channel();
        let (agent, work_dir) = build_test_agent(MockClient::new(vec![]), Some(agent_tx));

        // `/mcp list` is a catalog command now; drive it the way the loop does,
        // with a view over this agent's router.
        let slot: std::sync::Arc<tokio::sync::Mutex<Option<tact::Agent>>> =
            std::sync::Arc::new(tokio::sync::Mutex::new(Some(agent)));
        let catalog = super::builtin_command_catalog(&slot, None, &work_dir);
        let outcome = catalog
            .run_async(&tact::command::CommandInvocation {
                name: "mcp".to_string(),
                args: "list".to_string(),
            })
            .await;
        let md = match outcome {
            tact::command::CommandOutcome::Emit(emitted) => match emitted.into_inner() {
                AgentUpdate::MdInfo(md) => md,
                other => panic!("expected MdInfo, got {other:?}"),
            },
            other => panic!("expected a listing, got {other:?}"),
        };
        assert!(md.contains("MCP Servers"), "md: {md}");
    }

    /// `/hooks list` reaches the loader and its listing is shown.
    ///
    /// The command owns the verb now, so this drives it directly: what matters is
    /// that the listing reaches the user rather than being dropped.
    #[tokio::test]
    async fn hooks_list_emits_the_review_listing() {
        install_test_config();
        let work_dir = std::env::temp_dir();
        let command = tact::command::hooks::HooksCommand::new(
            work_dir,
            std::sync::Arc::new(crate::hooks_cli::render_hooks_listing),
        );
        let outcome = tact::command::Command::run_async(
            &command,
            &tact::command::CommandInvocation {
                name: "hooks".to_string(),
                args: String::new(),
            },
        )
        .await;

        // Both wordings come from `hooks_cli::render_hooks_listing`, so the
        // assertion holds whether or not this machine has hooks configured.
        let text = match outcome {
            tact::command::CommandOutcome::Emit(emitted) => match emitted.into_inner() {
                AgentUpdate::MdInfo(md) => md,
                other => panic!("expected MdInfo, got {other:?}"),
            },
            other => panic!("expected an emitted listing, got {other:?}"),
        };
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
        let command = tact::command::hooks::HooksCommand::new(
            std::env::temp_dir(),
            std::sync::Arc::new(crate::hooks_cli::render_hooks_listing),
        );
        // A bare `trust` with no selector is the path that bails before touching
        // the review store.
        let outcome = tact::command::Command::run_async(
            &command,
            &tact::command::CommandInvocation {
                name: "hooks".to_string(),
                args: "trust".to_string(),
            },
        )
        .await;

        let message = match outcome {
            tact::command::CommandOutcome::Failed(text) => text,
            other => panic!("a refusal must be reported, not swallowed: {other:?}"),
        };
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
            .push_back(tact::subagent::SubagentResult {
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
                if let Some(AgentUpdate::TaskComplete(_)) = agent_rx.recv().await {
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
            if let AgentUpdate::TaskComplete(_) = update {
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
    async fn the_stats_fallback_arm_answers_instead_of_ignoring() {
        // The loop-level arm is the normal path; this covers a caller that hands
        // the command straight to `handle_user_command`. It used to do nothing,
        // so a `/stats` on that path silently opened no panel.
        install_test_config();
        let (agent_tx, mut agent_rx) = tokio::sync::mpsc::unbounded_channel();
        let (mut agent, work_dir) = build_test_agent(MockClient::new(vec![]), Some(agent_tx));

        super::handle_user_command(
            &mut agent,
            UserCommand::RunCommand {
                name: "stats".to_string(),
                args: String::new(),
            },
            &work_dir,
        )
        .await;

        let mut saw_popup = false;
        while let Ok(update) = agent_rx.try_recv() {
            if let AgentUpdate::PopupMarkdown { title, source } = update {
                assert!(title.contains("Statistics"), "{title}");
                assert!(source.contains("Session Stats"), "{source}");
                saw_popup = true;
            }
        }
        assert!(
            saw_popup,
            "the fallback arm must open the stats panel, not ignore the request"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn query_stats_responds_immediately_while_task_runs() {
        // The complete regression test is bounded so a future change that breaks
        // either the in-flight command or cleanup cannot block the entire suite.
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            query_stats_responds_immediately_while_task_runs_inner(),
        )
        .await
        .expect("stats responsiveness test exceeded its 10s safety bound");
    }

    async fn query_stats_responds_immediately_while_task_runs_inner() {
        use std::time::Duration;

        install_test_config();
        let (agent_tx, mut agent_rx) = tokio::sync::mpsc::unbounded_channel();
        // Deterministic "long running LLM call": the responder waits for the
        // test to release it, but has a hard cap. Without the cap, a failed
        // assertion before the cleanup below would strand a worker forever.
        let release = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let release_rx = release.clone();
        let mock = MockClient::with_responder(move |_request, _| {
            let deadline = std::time::Instant::now() + Duration::from_secs(2);
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

        // Start a task that is now stuck in the responder, then ask for stats.
        user_cmd_tx
            .send(UserCommand::SubmitTask("long task".into()))
            .unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        let start = std::time::Instant::now();
        user_cmd_tx
            .send(UserCommand::RunCommand {
                name: "stats".to_string(),
                args: String::new(),
            })
            .unwrap();

        // The stats snapshot must arrive while the task is still blocked. A
        // single deadline bounds the whole receive loop: a stream of unrelated
        // updates must not restart a short timeout forever.
        let received = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                match agent_rx.recv().await {
                    Some(AgentUpdate::PopupMarkdown { .. }) => return true,
                    Some(_) => continue,
                    None => return false,
                }
            }
        })
        .await;
        let saw_stats = matches!(received, Ok(true));
        let response_time = start.elapsed();

        // Always clean up before asserting. If the popup is missing, release the
        // mock responder and give the loop a bounded chance to exit; a failed
        // assertion must not strand a worker and make later tests appear hung.
        release.store(true, std::sync::atomic::Ordering::Relaxed);
        drop(user_cmd_tx);
        let loop_result = tokio::time::timeout(Duration::from_secs(5), loop_handle).await;

        assert!(
            loop_result.is_ok(),
            "command loop must finish after release"
        );
        assert!(
            saw_stats,
            "expected the stats popup while the task is still running; receive result: {received:?}"
        );
        assert!(
            response_time < Duration::from_millis(450),
            "QueryStats must NOT await the in-flight task (took {response_time:?})"
        );
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
            .send(UserCommand::RunCommand {
                name: "background".to_string(),
                args: String::new(),
            })
            .unwrap();

        let mut saw_popup = false;
        loop {
            match tokio::time::timeout(Duration::from_millis(300), agent_rx.recv()).await {
                Ok(Some(AgentUpdate::PopupMarkdown { title, source })) => {
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
