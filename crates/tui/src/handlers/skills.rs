//! Slash / palette skill invocation.
//!
//! Built-ins win over same-named skills. From the `/` popup, **Enter** invokes
//! immediately; **Tab** only fills `/name ` for optional args. Invoke wraps the
//! body in `<skill>` and applies Claude Code–style bare `$ARGUMENTS`
//! substitution (or appends `ARGUMENTS:` when the placeholder is absent and
//! args are present). Indexed `$ARGUMENTS[N]` is left unchanged. Shared
//! [`submit_user_task`] matches a normal Insert Enter submit (Planning / log /
//! history).

use tact_protocol::UserCommand;

use super::CommandExecOutcome;
use crate::widgets::state::{App, SkillEntry, Status};

/// Extract args after `/{skill_name}` from the input box (empty if none / partial).
pub(super) fn skill_args_from_input(input: &str, skill_name: &str) -> String {
    let trimmed = input.trim();
    let Some(rest) = trimmed.strip_prefix('/') else {
        return String::new();
    };
    let Some(after_name) = rest.strip_prefix(skill_name) else {
        return String::new();
    };
    // End of token or whitespace boundary (avoid `/demo` matching `/demo-test`).
    if after_name.is_empty() {
        return String::new();
    }
    if !after_name.starts_with(char::is_whitespace) {
        return String::new();
    }
    after_name.trim().to_string()
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

/// True when `$ARGUMENTS` is a bare placeholder at this position (not indexed,
/// not a longer token like `$ARGUMENTS2`).
fn is_bare_arguments_placeholder(after: &str) -> bool {
    match after.chars().next() {
        None => true,
        Some('[') => false,
        Some(c) if c.is_ascii_alphanumeric() || c == '_' => false,
        Some(_) => true,
    }
}

/// True when body has a bare `$ARGUMENTS` placeholder.
fn has_bare_arguments_placeholder(body: &str) -> bool {
    let mut rest = body;
    while let Some(idx) = rest.find("$ARGUMENTS") {
        let after = &rest[idx + "$ARGUMENTS".len()..];
        if is_bare_arguments_placeholder(after) {
            return true;
        }
        rest = after;
    }
    false
}

/// Substitute bare `$ARGUMENTS` only — leave `$ARGUMENTS[N]` / `$ARGUMENTS2` untouched.
fn substitute_arguments(body: &str, args: &str) -> String {
    let mut out: String = String::with_capacity(body.len() + args.len());
    let mut rest = body;
    while let Some(idx) = rest.find("$ARGUMENTS") {
        out.push_str(&rest[..idx]);
        let after = &rest[idx + "$ARGUMENTS".len()..];
        if is_bare_arguments_placeholder(after) {
            out.push_str(args);
            rest = after;
        } else {
            out.push_str("$ARGUMENTS");
            rest = after;
        }
    }
    out.push_str(rest);
    out
}

/// Escape attribute text for skill name in `<skill name="…">`.
fn escape_xml_attr(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
}

/// Render skill body for the agent, Claude Code–style `$ARGUMENTS` / append.
pub(super) fn render_skill_body(skill: &SkillEntry, args: &str) -> String {
    let body = skill.body.trim();
    if has_bare_arguments_placeholder(body) {
        substitute_arguments(body, args)
    } else if args.is_empty() {
        body.to_string()
    } else {
        // Claude Code: when `$ARGUMENTS` is absent, append so the model still sees args.
        format!("{body}\n\nARGUMENTS: {args}")
    }
}

/// Build the agent-facing task text with skill body wrapped like `load_skill`.
///
/// Argument framing matches Claude Code (`$ARGUMENTS` or trailing `ARGUMENTS:`).
/// The system prompt explains that slash-invoked `<skill>` blocks (including
/// `ARGUMENTS:`) are user invocations, not `load_skill` tool metadata.
pub(super) fn format_skill_agent_task(skill: &SkillEntry, args: &str) -> String {
    format!(
        "<skill name=\"{}\">\n{}\n</skill>",
        escape_xml_attr(&skill.name),
        render_skill_body(skill, args)
    )
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

    #[test]
    fn skill_args_strips_command_prefix() {
        assert_eq!(
            skill_args_from_input("/code-reviewer fix auth", "code-reviewer"),
            "fix auth"
        );
        assert_eq!(skill_args_from_input("/code-reviewer", "code-reviewer"), "");
        assert_eq!(skill_args_from_input("/cod", "code-reviewer"), "");
        // Prefix skill must not steal args from a longer skill name.
        assert_eq!(skill_args_from_input("/demo-test x", "demo"), "");
    }

    #[test]
    fn format_skill_agent_task_wraps_body() {
        let skill = SkillEntry {
            name: "demo".into(),
            description: "d".into(),
            body: "Use Result.".into(),
        };
        let out = format_skill_agent_task(&skill, "refactor foo");
        assert!(out.contains("<skill name=\"demo\">"));
        assert!(out.contains("Use Result."));
        assert!(out.contains("ARGUMENTS: refactor foo"));
    }

    #[test]
    fn format_skill_substitutes_arguments_placeholder() {
        let skill = SkillEntry {
            name: "deploy".into(),
            description: "d".into(),
            body: "Deploy $ARGUMENTS to prod.".into(),
        };
        let out = format_skill_agent_task(&skill, "v2");
        assert!(out.contains("Deploy v2 to prod."));
        assert!(!out.contains("$ARGUMENTS"));
        assert!(!out.contains("ARGUMENTS:"));
    }

    #[test]
    fn format_skill_leaves_indexed_arguments_placeholder() {
        let skill = SkillEntry {
            name: "deploy".into(),
            description: "d".into(),
            body: "First $ARGUMENTS[0]; all $ARGUMENTS.".into(),
        };
        let out = format_skill_agent_task(&skill, "v2");
        assert!(out.contains("First $ARGUMENTS[0]; all v2."));
    }

    #[test]
    fn format_skill_leaves_longer_arguments_token() {
        let skill = SkillEntry {
            name: "deploy".into(),
            description: "d".into(),
            body: "See $ARGUMENTS2 and use $ARGUMENTS.".into(),
        };
        let out = format_skill_agent_task(&skill, "v2");
        assert!(out.contains("See $ARGUMENTS2 and use v2."));
    }

    #[test]
    fn format_skill_no_args_is_body_only() {
        let skill = SkillEntry {
            name: "demo".into(),
            description: "d".into(),
            body: "Just run.".into(),
        };
        let out = format_skill_agent_task(&skill, "");
        assert!(out.contains("Just run."));
        assert!(!out.contains("ARGUMENTS:"));
    }

    #[test]
    fn format_skill_escapes_name_attr() {
        let skill = SkillEntry {
            name: r#"weird"name"#.into(),
            description: "d".into(),
            body: "x".into(),
        };
        let out = format_skill_agent_task(&skill, "");
        assert!(out.contains(r#"<skill name="weird&quot;name">"#));
    }

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
