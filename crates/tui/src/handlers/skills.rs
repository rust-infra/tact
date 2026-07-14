//! Slash / palette skill invocation (Claude-like: complete first, Enter to run).

use super::CommandExecOutcome;
use crate::widgets::state::{App, SkillEntry, Status};
use tact_protocol::UserCommand;

/// Extract args after `/{skill_name}` from the input box (empty if none / partial).
pub(super) fn skill_args_from_input(input: &str, skill_name: &str) -> String {
    let trimmed = input.trim();
    let prefix = format!("/{skill_name}");
    trimmed
        .strip_prefix(&prefix)
        .map(str::trim)
        .unwrap_or("")
        .to_string()
}

pub(super) fn find_skill<'a>(app: &'a App, cmd: &str) -> Option<&'a SkillEntry> {
    app.skills_data.iter().find(|s| s.name == cmd)
}

pub(crate) fn is_skill_command(app: &App, cmd: &str) -> bool {
    find_skill(app, cmd).is_some()
}

pub(crate) fn skill_name_set(app: &App) -> std::collections::HashSet<&str> {
    app.skills_data.iter().map(|s| s.name.as_str()).collect()
}

/// Render skill body for the agent, Claude-style `$ARGUMENTS` / append.
pub(super) fn render_skill_body(skill: &SkillEntry, args: &str) -> String {
    let body = skill.body.trim();
    if body.contains("$ARGUMENTS") {
        body.replace("$ARGUMENTS", args)
    } else if args.is_empty() {
        body.to_string()
    } else {
        // Claude Code default when `$ARGUMENTS` is absent.
        format!("{body}\n\nARGUMENTS: {args}")
    }
}

/// Build the agent-facing task text with skill body wrapped like `load_skill`.
pub(super) fn format_skill_agent_task(skill: &SkillEntry, args: &str) -> String {
    format!(
        "<skill name=\"{}\">\n{}\n</skill>",
        skill.name,
        render_skill_body(skill, args)
    )
}

/// Shared task submission used by normal Enter and skill invoke.
/// Returns `true` when the task was accepted and dispatched.
pub(crate) fn submit_user_task(app: &mut App, display_text: String, agent_task: String) -> bool {
    if matches!(app.status, Status::Planning | Status::Executing { .. }) {
        app.flash_msg = Some((
            app.msgs().input_busy_msg.to_string(),
            std::time::Instant::now(),
        ));
        return false;
    }

    let display_chars = display_text.chars().count();
    let agent_chars = agent_task.chars().count();
    let limit = app.context_limit_chars;
    if display_chars > limit || agent_chars > limit {
        let msg = app
            .msgs()
            .input_too_long_tmpl
            .replace("{}", &limit.to_string());
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
    let skill = find_skill(app, cmd)?.clone();
    let args = skill_args_from_input(&app.input, &skill.name);
    app.slash_command.active = false;

    let display = if args.is_empty() {
        format!("/{}", skill.name)
    } else {
        format!("/{} {}", skill.name, args)
    };
    let agent_task = format_skill_agent_task(&skill, &args);
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
        assert_eq!(
            skill_args_from_input("/code-reviewer", "code-reviewer"),
            ""
        );
        assert_eq!(skill_args_from_input("/cod", "code-reviewer"), "");
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
}
