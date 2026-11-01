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

/// Build the agent-facing task text with skill body wrapped like `load_skill`.
///
/// Argument framing matches Claude Code (`$ARGUMENTS` or trailing `ARGUMENTS:`).
/// The system prompt explains that slash-invoked `<skill>` blocks (including
/// `ARGUMENTS:`) are user invocations, not `load_skill` tool metadata.
pub(super) fn format_skill_agent_task(skill: &SkillEntry, args: &str) -> String {
    tact::skill::format_skill_task(
        &skill.name,
        std::path::Path::new(&skill.dir),
        &skill.body,
        args,
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

/// `/skill` — the built-in command: `/skill list` shows the table, `/skill
/// reload` rescans the roots, `/skill disable <name>` / `/skill enable <name>`
/// flip whether a skill appears in the system prompt's catalog, and
/// `/skill <name> [args]` runs one skill.
///
/// Subcommand parsing lives in the command's own module, like `/mcp` and
/// `/plugin`: the palette only ever knows the command name, and the input box
/// holds the rest. Running a skill from here is the same code path as
/// `/{name}` (see [`invoke_skill`]) — that keeps one implementation of
/// `$ARGUMENTS` and one log line builder, with only the echo differing.
pub(super) fn handle_skill_builtin_command(app: &mut App) -> CommandExecOutcome {
    // Owned tokens: the arms below mutate `app`, and the input is what they
    // read the subcommand from.
    let parts: Vec<String> = app.input.split_whitespace().map(str::to_owned).collect();
    let parts: Vec<&str> = parts.iter().map(String::as_str).collect();
    match parts.as_slice() {
        ["/skill", "list"] => {
            // Purely local rendering (no agent round-trip), so unlike
            // `/mcp list` this stays available while a task is in flight.
            super::show_skills_command(app);
            CommandExecOutcome {
                handled: true,
                clear_input: true,
            }
        }
        ["/skill", "reload"] => {
            // Off-loop: the reload scans the filesystem; the loop reports the
            // outcome via the background-task poll.
            app.start_skills_reload(
                crate::widgets::state::app::background::SkillsReloadSource::Command,
            );
            CommandExecOutcome {
                handled: true,
                clear_input: true,
            }
        }
        ["/skill", "disable", name, ..] => set_skill_catalog_visibility(app, name, false),
        ["/skill", "enable", name, ..] => set_skill_catalog_visibility(app, name, true),
        // The declared subcommands (`list` / `reload` / `disable` / `enable`)
        // win a name collision, exactly as a built-in wins it at the first
        // level.
        ["/skill", name, ..] if find_skill(app, name).is_some() => {
            let args = skill_args_from_subcommand_input(&app.input, name);
            let slash_line = if args.is_empty() {
                format!("/skill {name}")
            } else {
                format!("/skill {name} {args}")
            };
            invoke_skill(app, name, &args, &slash_line);
            CommandExecOutcome {
                handled: true,
                clear_input: false,
            }
        }
        _ => {
            // Bare `/skill` (and any unknown subcommand) leaves the usage hint
            // in the input box so the user can complete the command.
            app.save_undo();
            app.input = "/skill ".into();
            app.input_cursor = app.input.len();
            app.flash_msg = Some((app.msgs().skill_usage.to_owned(), std::time::Instant::now()));
            CommandExecOutcome {
                handled: true,
                clear_input: false,
            }
        }
    }
}

/// `/skill disable <name>` / `/skill enable <name>` — keep a skill out of the
/// system prompt's catalog, or put it back, and write `[agent].skill_disable`.
///
/// The switch governs automatic discovery, not capability: a hidden skill still
/// runs from `/skill <name>` and still resolves through `load_skill`. That is
/// why the report says *hidden from the catalog* rather than *disabled*.
///
/// The registry is shared with the agent, so flipping it here changes the next
/// turn's `# Available skills` with no reload — and the TUI's own list is
/// rebuilt from that same registry, so the row is marked in the same breath
/// instead of at the next `/skill reload`.
fn set_skill_catalog_visibility(app: &mut App, name: &str, show: bool) -> CommandExecOutcome {
    let msgs = app.msgs();

    // Read the flag out before touching `app`: `find_skill` borrows it, and the
    // arms below mutate it.
    let Some(disabled) = find_skill(app, name).map(|skill| skill.disabled) else {
        app.flash_msg = Some((
            msgs.skill_unknown_tmpl.replace("{}", name),
            std::time::Instant::now(),
        ));
        return CommandExecOutcome {
            handled: true,
            clear_input: false,
        };
    };

    if disabled == !show {
        let template = if show {
            msgs.skill_already_shown_tmpl
        } else {
            msgs.skill_already_hidden_tmpl
        };
        app.add_system_message(template.replace("{}", name));
        return CommandExecOutcome {
            handled: true,
            clear_input: true,
        };
    }

    // Rebuild the whole set rather than applying a delta: this Vec is what gets
    // written to the config file, and a partial list would silently re-enable
    // everything left out of it.
    let names = {
        let mut reg = tact::skill::lock_skills(&app.skill_registry);
        let mut names: Vec<String> = reg.disabled().iter().cloned().collect();
        if show {
            names.retain(|disabled| disabled != name);
        } else if !names.iter().any(|disabled| disabled == name) {
            names.push(name.to_string());
        }
        names.sort();
        reg.set_disabled(names.clone());
        names
    };

    {
        let reg = tact::skill::lock_skills(&app.skill_registry);
        app.skills_data = super::skills_snapshot_from(&reg);
    }

    let outcome = if !app.ui_config_available() {
        msgs.skill_catalog_session_only
    } else {
        match tact::config::persist_skill_disable(&names) {
            Ok(()) => msgs.skill_catalog_persisted,
            Err(error) => {
                app.add_system_message(
                    msgs.skill_catalog_persist_failed_tmpl
                        .replace("{}", &error.to_string()),
                );
                return CommandExecOutcome {
                    handled: true,
                    clear_input: true,
                };
            }
        }
    };

    let template = if show {
        msgs.skill_catalog_shown_tmpl
    } else {
        msgs.skill_catalog_hidden_tmpl
    };
    app.add_system_message(template.replacen("{}", name, 1).replacen("{}", outcome, 1));
    CommandExecOutcome {
        handled: true,
        clear_input: true,
    }
}

/// Args typed after `/skill <name>` (empty when there are none).
///
/// The name must be a whole token: `/skill demo-test x` gives nothing for the
/// skill `demo`, the same guard [`skill_args_from_input`] applies to `/demo`.
pub(super) fn skill_args_from_subcommand_input(input: &str, name: &str) -> String {
    let Some(rest) = input.trim().strip_prefix("/skill") else {
        return String::new();
    };
    let Some(after_name) = rest.trim_start().strip_prefix(name) else {
        return String::new();
    };
    if !after_name.is_empty() && !after_name.starts_with(char::is_whitespace) {
        return String::new();
    }
    after_name.trim().to_string()
}

/// Run one skill: build the `<skill>` task, submit it, and echo `slash_line`.
///
/// Shared by `/skill <name>` and `/{name}` so the two cannot drift; the caller
/// owns the echo because that is the only difference between them.
fn invoke_skill(app: &mut App, name: &str, args: &str, slash_line: &str) {
    // Borrow the skill long enough to render the task, then drop before
    // mutating `app`.
    let Some(agent_task) = find_skill(app, name).map(|skill| format_skill_agent_task(skill, args))
    else {
        return;
    };
    app.slash_command.active = false;

    if submit_user_task(app, slash_line.to_owned(), agent_task) {
        app.input.clear();
        app.input_cursor = 0;
    }
}

/// Invoke `/skill-name` [args] (the direct form): always runs, no equip step.
///
/// `None` for a name no skill answers to, which is what lets the dispatcher
/// fall through and send the text as a normal message.
pub(super) fn handle_skill_command(app: &mut App, cmd: &str) -> Option<CommandExecOutcome> {
    find_skill(app, cmd)?;
    let args = skill_args_from_input(&app.input, cmd);
    let slash_line = if args.is_empty() {
        format!("/{cmd}")
    } else {
        format!("/{cmd} {args}")
    };
    invoke_skill(app, cmd, &args, &slash_line);

    Some(CommandExecOutcome {
        handled: true,
        clear_input: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_fixtures::TestApp;

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
    fn skill_args_from_the_skill_command_strips_the_whole_prefix() {
        assert_eq!(
            skill_args_from_subcommand_input("/skill code-reviewer fix auth", "code-reviewer"),
            "fix auth"
        );
        assert_eq!(
            skill_args_from_subcommand_input("/skill   code-reviewer   ", "code-reviewer"),
            ""
        );
        assert_eq!(
            skill_args_from_subcommand_input("/skill code-reviewer", "code-reviewer"),
            ""
        );
        // A partial name is not the name.
        assert_eq!(
            skill_args_from_subcommand_input("/skill code", "code-reviewer"),
            ""
        );
        // The name must be a whole token: `/skill demo-test x` gives `demo`
        // nothing, exactly like the direct form.
        assert_eq!(
            skill_args_from_subcommand_input("/skill demo-test x", "demo"),
            ""
        );
    }

    #[test]
    fn the_skill_command_runs_a_skill_with_args() {
        use tokio::sync::mpsc::unbounded_channel;

        use crate::widgets::state::App;

        let (_agent_tx, agent_rx) = unbounded_channel::<tact_protocol::AgentUpdate>();
        let (user_cmd_tx, mut user_cmd_rx) = unbounded_channel();
        let (plugin_tx, _plugin_rx) = unbounded_channel();
        let (_event_tx, plugin_event_rx) = unbounded_channel();
        let (history_tx, _history_rx) = unbounded_channel();
        let mut app = App::new(
            agent_rx,
            None,
            plugin_event_rx,
            plugin_tx,
            user_cmd_tx,
            std::path::PathBuf::from("."),
            Vec::new(),
            "test-session".to_string(),
            history_tx,
            "retro".to_string(),
            String::new(),
            Vec::new(),
        );
        app.skills_data = vec![SkillEntry {
            name: "demo".into(),
            description: "d".into(),
            body: "Follow the checklist.".into(),

            dir: String::new(),
            disabled: false,
        }];
        app.input = "/skill demo fix auth".into();
        app.input_cursor = app.input.len();

        let outcome = handle_skill_builtin_command(&mut app);

        assert!(outcome.handled);
        assert!(app.input.is_empty(), "an invoked skill clears the input");
        match user_cmd_rx.try_recv().expect("skill must submit a task") {
            tact_protocol::UserCommand::SubmitTask(task) => {
                assert!(task.contains("<skill name=\"demo\""), "{task}");
                assert!(task.contains("Follow the checklist."), "{task}");
                assert!(task.contains("ARGUMENTS: fix auth"), "{task}");
            }
            other => panic!("expected SubmitTask, got {other:?}"),
        }
    }

    #[test]
    fn an_unknown_skill_name_gets_the_usage() {
        let mut app = crate::render::test_harness::make_app();
        app.input = "/skill nope".into();
        app.input_cursor = app.input.len();

        let outcome = handle_skill_builtin_command(&mut app);

        assert!(outcome.handled);
        assert_eq!(app.input, "/skill ", "the usage hint stays in the box");
        assert!(app.flash_msg.is_some());
    }

    #[test]
    fn format_skill_agent_task_wraps_body() {
        let skill = SkillEntry {
            name: "demo".into(),
            description: "d".into(),
            body: "Use Result.".into(),

            dir: String::new(),
            disabled: false,
        };
        let out = format_skill_agent_task(&skill, "refactor foo");
        assert!(out.contains("<skill name=\"demo\""), "{out}");
        assert!(out.contains("Use Result."));
        assert!(out.contains("ARGUMENTS: refactor foo"));
    }

    /// The slash path and the `load_skill` tool produce the same wrapper — a
    /// skill must not see its `dir` on one path and not the other.
    #[test]
    fn format_skill_agent_task_carries_the_skill_directory() {
        let skill = SkillEntry {
            name: "demo".into(),
            description: "d".into(),
            body: "Read references/formats.md.".into(),
            dir: "/skills/demo".into(),
            disabled: false,
        };

        let out = format_skill_agent_task(&skill, "");

        assert!(
            out.contains(r#"<skill name="demo" dir="/skills/demo">"#),
            "{out}"
        );
        assert!(out.contains("Read references/formats.md."));
    }

    #[test]
    fn format_skill_substitutes_arguments_placeholder() {
        let skill = SkillEntry {
            name: "deploy".into(),
            description: "d".into(),
            body: "Deploy $ARGUMENTS to prod.".into(),

            dir: String::new(),
            disabled: false,
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

            dir: String::new(),
            disabled: false,
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

            dir: String::new(),
            disabled: false,
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

            dir: String::new(),
            disabled: false,
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

            dir: String::new(),
            disabled: false,
        };
        let out = format_skill_agent_task(&skill, "");
        assert!(out.contains(r#"<skill name="weird&quot;name""#), "{out}");
    }

    // ---- Codex-style queued submission (pending messages) ----

    #[test]
    fn submit_user_task_queues_when_busy() {
        let (mut app, mut user_cmd_rx) = TestApp::new().into_commands();
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
        let (mut app, mut user_cmd_rx) = TestApp::new().into_commands();
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
        let (mut app, _user_cmd_rx) = TestApp::new().into_commands();
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
        let (mut app, _user_cmd_rx) = TestApp::new().into_commands();
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
        let (mut app, mut user_cmd_rx) = TestApp::new().into_commands();
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
        let (mut app, mut user_cmd_rx) = TestApp::new().into_commands();
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
