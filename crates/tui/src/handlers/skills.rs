//! Slash / palette skill invocation.
//!
//! Built-ins win over same-named skills. From the `/` popup, **Enter** invokes
//! immediately; **Tab** only fills `/name ` for optional args. The command line
//! parsing and the task text — the body wrapped in `<skill>`, Claude
//! Code–style bare `$ARGUMENTS` substitution, `ARGUMENTS:` appended when the
//! placeholder is absent and args are present — live in
//! [`tact::skill::slash_args`] / [`tact::skill::slash_task`], shared with the
//! desktop client so the two cannot drift. Shared [`submit_user_task`] matches
//! a normal Insert Enter submit (Planning / log / history).

use tact_protocol::UserCommand;

use super::CommandExecOutcome;
use crate::widgets::state::{App, SkillEntry, Status};

/// Extract args after `/{skill_name}` from the input box (empty if none / partial).
pub(super) fn skill_args_from_input(input: &str, skill_name: &str) -> String {
    tact::skill::slash_args(input, skill_name)
}

pub(super) fn find_skill<'a>(app: &'a App, cmd: &str) -> Option<&'a SkillEntry> {
    app.skills_data.iter().find(|s| s.name == cmd)
}

pub(crate) fn is_skill_command(app: &App, cmd: &str) -> bool {
    find_skill(app, cmd).is_some()
}

pub(crate) fn skill_name_set(app: &App) -> std::collections::HashSet<&str> {
    crate::render::slash_style::skill_name_set(&app.skills_data)
}

/// Build the agent-facing task text with the skill body wrapped like `load_skill`.
///
/// The framing is [`tact::skill::slash_task`]'s: argument handling matches
/// Claude Code (`$ARGUMENTS` or trailing `ARGUMENTS:`), and the system prompt
/// explains that slash-invoked `<skill>` blocks are user invocations, not
/// `load_skill` tool metadata.
pub(super) fn format_skill_agent_task(skill: &SkillEntry, args: &str) -> String {
    tact::skill::slash_task(&skill.name, &skill.body, args)
}

/// Shared task submission used by normal Enter and skill invoke.
///
/// Returns `true` when the task was accepted — either dispatched immediately
/// (agent idle) or queued while the agent is busy (Codex-style "submit after
/// the current task"; see [`flush_pending_when_idle`]).
pub(crate) fn submit_user_task(app: &mut App, display_text: String, agent_task: String) -> bool {
    if !task_within_limits(app, &display_text, &agent_task) {
        return false;
    }
    if matches!(app.status, Status::Planning | Status::Executing { .. }) {
        // The agent is busy: queue the message instead of rejecting it. It is
        // auto-submitted when the current task finishes (or immediately on Esc).
        app.queue_pending_message(display_text, agent_task);
        return true;
    }
    dispatch_user_task(app, display_text, agent_task)
}

/// Char-limit validation shared by the direct and queued submit paths.
fn task_within_limits(app: &mut App, display_text: &str, agent_task: &str) -> bool {
    let display_chars = display_text.chars().count();
    let agent_chars = agent_task.chars().count();
    if tact::consts::exceeds_input_char_limit(agent_chars) {
        let msg = app
            .msgs()
            .skill_task_too_long_tmpl
            .replace("{}", &tact::consts::MAX_INPUT_CHARS.to_string());
        app.add_system_message(msg);
        return false;
    }
    if tact::consts::exceeds_input_char_limit(display_chars) {
        let msg = app
            .msgs()
            .input_too_long_tmpl
            .replace("{}", &tact::consts::MAX_INPUT_CHARS.to_string());
        app.add_system_message(msg);
        return false;
    }
    true
}

/// Dispatch one task to the agent: record history, show the user bubble, and
/// send `SubmitTask`. Callers must already have validated limits and the busy
/// gate (see [`submit_user_task`]).
fn dispatch_user_task(app: &mut App, display_text: String, agent_task: String) -> bool {
    if app.input_history.entries.last() != Some(&display_text) {
        app.input_history.entries.push(display_text.clone());
        app.save_history(&display_text);
    }
    app.input_history.index = None;
    app.input_history.saved.clear();

    app.status = Status::Planning;
    app.add_user_message(display_text);
    app.plan_mut().reset();
    app.last_prompt_elapsed_secs = None;
    app.task_start_time = Some(chrono::Local::now());
    // Turn counters: this is the single choke point for user turns (direct
    // submits, queued flushes, skill dispatch), so count here. The per-task LLM
    // counter resets and is driven by `AgentUpdate::TurnStats` from then on.
    app.status_bar_mut().turn_user += 1;
    app.status_bar_mut().turn_llm = 0;
    app.status_bar_mut().turn_llm_cap = None;
    let _ = app.user_cmd_tx.send(UserCommand::SubmitTask(agent_task));
    true
}

/// Codex-style auto-submit: once the agent reaches Idle/Done, submit every
/// queued message as its own task. The command driver serializes them in
/// order, so each queued message becomes the next user turn.
pub(crate) fn flush_pending_when_idle(app: &mut App) {
    if app.pending_messages.is_empty() || !matches!(app.status, Status::Idle | Status::Done) {
        return;
    }
    let pending = std::mem::take(&mut app.pending_messages);
    for p in pending {
        let _ = dispatch_user_task(app, p.display, p.agent_task);
    }
}

/// Invoke `/skill-name` [args]: always runs (no equip step).
pub(super) fn handle_skill_command(app: &mut App, cmd: &str) -> Option<CommandExecOutcome> {
    // Borrow skill long enough to render the task, then drop before mutating `app`.
    let (display, agent_task) = {
        let skill = find_skill(app, cmd)?;
        let args = skill_args_from_input(&app.input, &skill.name);
        let display = if args.is_empty() {
            format!("/{}", skill.name)
        } else {
            format!("/{} {}", skill.name, args)
        };
        let agent_task = format_skill_agent_task(skill, &args);
        (display, agent_task)
    };
    app.slash_command.active = false;

    if submit_user_task(app, display, agent_task) {
        app.input.clear();
        app.input_cursor = 0;
    }

    Some(CommandExecOutcome {
        handled: true,
        clear_input: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- Codex-style queued submission (pending messages) ----

    fn make_app_with_cmds() -> (App, tokio::sync::mpsc::UnboundedReceiver<UserCommand>) {
        use std::path::PathBuf;

        use tact_protocol::AgentUpdate;
        use tokio::sync::mpsc::unbounded_channel;

        let (_agent_tx, agent_rx) = unbounded_channel::<AgentUpdate>();
        let (user_cmd_tx, user_cmd_rx) = unbounded_channel::<UserCommand>();
        let (plugin_tx, _plugin_request_rx) = unbounded_channel();
        let (_plugin_event_tx, plugin_rx) = unbounded_channel();
        let (history_tx, _history_rx) = unbounded_channel();
        let app = App::new(
            agent_rx,
            None,
            plugin_rx,
            plugin_tx,
            user_cmd_tx,
            PathBuf::from("."),
            Vec::new(),
            "test-session".to_string(),
            history_tx,
            "retro".to_string(),
            String::new(),
            Vec::new(),
        );
        (app, user_cmd_rx)
    }

    #[test]
    fn submit_user_task_queues_when_busy() {
        let (mut app, mut user_cmd_rx) = make_app_with_cmds();
        app.status = Status::Executing {
            current_step: 0,
            total: 1,
        };

        let ok = submit_user_task(&mut app, "hi".into(), "hi".into());

        assert!(ok, "queued task counts as accepted");
        assert_eq!(app.pending_messages.len(), 1);
        assert_eq!(app.pending_messages[0].display, "hi");
        assert_eq!(app.pending_messages[0].agent_task, "hi");
        assert!(
            user_cmd_rx.try_recv().is_err(),
            "queued task must not dispatch immediately"
        );
        assert!(
            matches!(app.status, Status::Executing { .. }),
            "busy status must be preserved while queued"
        );
    }

    #[test]
    fn submit_user_task_dispatches_when_idle() {
        let (mut app, mut user_cmd_rx) = make_app_with_cmds();
        app.status = Status::Idle;

        let ok = submit_user_task(&mut app, "go".into(), "go".into());

        assert!(ok);
        assert!(app.pending_messages.is_empty());
        assert!(matches!(app.status, Status::Planning));
        match user_cmd_rx.try_recv().expect("SubmitTask") {
            UserCommand::SubmitTask(task) => assert_eq!(task, "go"),
            other => panic!("expected SubmitTask, got {other:?}"),
        }
    }

    #[test]
    fn submit_user_task_counts_session_turn_and_resets_llm_counter() {
        let (mut app, _user_cmd_rx) = make_app_with_cmds();
        app.status = Status::Idle;
        // Stale per-task state from the previous turn must not leak.
        app.status_bar_mut().turn_llm = 7;
        app.status_bar_mut().turn_llm_cap = Some(50);

        let ok = submit_user_task(&mut app, "one".into(), "one".into());
        assert!(ok);
        assert_eq!(app.status_bar_mut().turn_user, 1);
        assert_eq!(
            app.status_bar_mut().turn_llm,
            0,
            "LLM counter resets per task"
        );
        assert_eq!(app.status_bar_mut().turn_llm_cap, None);
    }

    #[test]
    fn queued_messages_each_count_as_a_session_turn() {
        let (mut app, _user_cmd_rx) = make_app_with_cmds();
        app.status = Status::Idle;
        let _ = submit_user_task(&mut app, "one".into(), "one".into());
        assert_eq!(app.status_bar_mut().turn_user, 1);

        // Busy now: these two queue instead of dispatching, so the counter must
        // not move until the flush actually dispatches them.
        let _ = submit_user_task(&mut app, "two".into(), "two".into());
        let _ = submit_user_task(&mut app, "three".into(), "three".into());
        assert_eq!(app.pending_messages.len(), 2);
        assert_eq!(
            app.status_bar_mut().turn_user,
            1,
            "queueing is not dispatching — no turn counted yet"
        );

        app.status = Status::Done;
        flush_pending_when_idle(&mut app);
        assert_eq!(
            app.status_bar_mut().turn_user,
            3,
            "each flushed queued message counts as its own user turn"
        );
    }

    #[test]
    fn flush_pending_when_idle_submits_all_queued_in_order() {
        let (mut app, mut user_cmd_rx) = make_app_with_cmds();
        app.status = Status::Executing {
            current_step: 0,
            total: 1,
        };
        submit_user_task(&mut app, "one".into(), "one".into());
        submit_user_task(&mut app, "two".into(), "two".into());
        assert_eq!(app.pending_messages.len(), 2);

        // Still busy: flush must not fire.
        flush_pending_when_idle(&mut app);
        assert_eq!(app.pending_messages.len(), 2);
        assert!(user_cmd_rx.try_recv().is_err());

        // Agent reached Idle (e.g. TaskComplete): every queued message is
        // submitted, each as its own task, in queue order.
        app.status = Status::Idle;
        flush_pending_when_idle(&mut app);
        assert!(app.pending_messages.is_empty(), "queue drained by flush");
        let mut tasks = Vec::new();
        while let Ok(cmd) = user_cmd_rx.try_recv() {
            if let UserCommand::SubmitTask(task) = cmd {
                tasks.push(task);
            }
        }
        assert_eq!(tasks, vec!["one".to_string(), "two".to_string()]);
    }

    #[test]
    fn flush_pending_fires_on_done_too() {
        let (mut app, mut user_cmd_rx) = make_app_with_cmds();
        app.status = Status::Executing {
            current_step: 0,
            total: 1,
        };
        submit_user_task(&mut app, "x".into(), "x".into());

        app.status = Status::Done;
        flush_pending_when_idle(&mut app);

        assert!(app.pending_messages.is_empty());
        assert!(matches!(
            user_cmd_rx.try_recv(),
            Ok(UserCommand::SubmitTask(_))
        ));
    }
}
