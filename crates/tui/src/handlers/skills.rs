//! Slash / palette skill invocation (complete first, Enter to run).
//!
//! Built-ins win over same-named skills. Invoke wraps the body in `<skill>` and
//! applies Claude Code–style bare `$ARGUMENTS` substitution (or appends
//! `ARGUMENTS:` when the placeholder is absent and args are present). Indexed
//! `$ARGUMENTS[N]` is left unchanged. Shared [`submit_user_task`] matches a
//! normal Insert Enter submit (Planning / log / history).

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
    s.replace('&', "&amp;").replace('"', "&quot;").replace('<', "&lt;")
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
    format!("<skill name=\"{}\">\n{}\n</skill>", escape_xml_attr(&skill.name), render_skill_body(skill, args))
}

/// Shared task submission used by normal Enter and skill invoke.
/// Returns `true` when the task was accepted and dispatched.
pub(crate) fn submit_user_task(app: &mut App, display_text: String, agent_task: String) -> bool {
    if matches!(app.status, Status::Planning | Status::Executing { .. }) {
        app.flash_msg = Some((app.msgs().input_busy_msg.to_string(), std::time::Instant::now()));
        return false;
    }

    let display_chars = display_text.chars().count();
    let agent_chars = agent_task.chars().count();
    if tact::consts::exceeds_input_char_limit(agent_chars) {
        let msg = app.msgs().skill_task_too_long_tmpl.replace("{}", &tact::consts::MAX_INPUT_CHARS.to_string());
        app.add_system_message(msg);
        return false;
    }
    if tact::consts::exceeds_input_char_limit(display_chars) {
        let msg = app.msgs().input_too_long_tmpl.replace("{}", &tact::consts::MAX_INPUT_CHARS.to_string());
        app.add_system_message(msg);
        return false;
    }

    if app.input_history.entries.last() != Some(&display_text) {
        app.input_history.entries.push(display_text.clone());
        app.save_history(&display_text);
    }
    app.input_history.index = None;
    app.input_history.saved.clear();

    app.status = Status::Planning;
    app.add_user_message(display_text);
    app.plan.reset();
    app.last_prompt_elapsed_secs = None;
    app.task_start_time = Some(chrono::Local::now());
    let _ = app.user_cmd_tx.send(UserCommand::SubmitTask(agent_task));
    true
}

/// Invoke `/skill-name` [args]: always runs (no equip step).
pub(super) fn handle_skill_command(app: &mut App, cmd: &str) -> Option<CommandExecOutcome> {
    // Borrow skill long enough to render the task, then drop before mutating `app`.
    let (display, agent_task) = {
        let skill = find_skill(app, cmd)?;
        let args = skill_args_from_input(&app.input, &skill.name);
        let display = if args.is_empty() { format!("/{}", skill.name) } else { format!("/{} {}", skill.name, args) };
        let agent_task = format_skill_agent_task(skill, &args);
        (display, agent_task)
    };
    app.slash_command.active = false;

    if submit_user_task(app, display, agent_task) {
        app.input.clear();
        app.input_cursor = 0;
    }

    Some(CommandExecOutcome { handled: true, clear_input: false })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skill_args_strips_command_prefix() {
        assert_eq!(skill_args_from_input("/code-reviewer fix auth", "code-reviewer"), "fix auth");
        assert_eq!(skill_args_from_input("/code-reviewer", "code-reviewer"), "");
        assert_eq!(skill_args_from_input("/cod", "code-reviewer"), "");
        // Prefix skill must not steal args from a longer skill name.
        assert_eq!(skill_args_from_input("/demo-test x", "demo"), "");
    }

    #[test]
    fn format_skill_agent_task_wraps_body() {
        let skill = SkillEntry { name: "demo".into(), description: "d".into(), body: "Use Result.".into() };
        let out = format_skill_agent_task(&skill, "refactor foo");
        assert!(out.contains("<skill name=\"demo\">"));
        assert!(out.contains("Use Result."));
        assert!(out.contains("ARGUMENTS: refactor foo"));
    }

    #[test]
    fn format_skill_substitutes_arguments_placeholder() {
        let skill =
            SkillEntry { name: "deploy".into(), description: "d".into(), body: "Deploy $ARGUMENTS to prod.".into() };
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
        let skill = SkillEntry { name: "demo".into(), description: "d".into(), body: "Just run.".into() };
        let out = format_skill_agent_task(&skill, "");
        assert!(out.contains("Just run."));
        assert!(!out.contains("ARGUMENTS:"));
    }

    #[test]
    fn format_skill_escapes_name_attr() {
        let skill = SkillEntry { name: r#"weird"name"#.into(), description: "d".into(), body: "x".into() };
        let out = format_skill_agent_task(&skill, "");
        assert!(out.contains(r#"<skill name="weird&quot;name">"#));
    }
}
