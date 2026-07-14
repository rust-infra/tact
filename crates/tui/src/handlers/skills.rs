//! Slash / palette skill invocation: equip or run with args.

use super::CommandExecOutcome;
use crate::widgets::state::{App, EquippedSkill, SkillEntry, Status};
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

pub(crate) fn skill_name_set(app: &App) -> std::collections::HashSet<&str> {
    app.skills_data.iter().map(|s| s.name.as_str()).collect()
}

/// Build the agent-facing task text with skill body wrapped like `load_skill`.
pub(super) fn format_skill_agent_task(skill: &SkillEntry, user_request: &str) -> String {
    format!(
        "<skill name=\"{}\">\n{}\n</skill>\n\n{}",
        skill.name,
        skill.body.trim(),
        user_request.trim()
    )
}

/// Shared task submission used by normal Enter and skill run-with-args.
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

/// Equip a skill (no args): preview + wait for the next natural-language task.
pub(super) fn equip_skill(app: &mut App, skill: &SkillEntry) {
    app.equipped_skill = Some(EquippedSkill {
        name: skill.name.clone(),
        description: skill.description.clone(),
        body: skill.body.clone(),
    });

    let msgs = app.msgs();
    let preview_lines: Vec<&str> = skill.body.lines().take(8).collect();
    let preview = preview_lines.join("\n");
    let ellipsis = if skill.body.lines().count() > 8 {
        "\n…"
    } else {
        ""
    };
    let desc = if skill.description.is_empty() {
        skill.name.as_str()
    } else {
        skill.description.as_str()
    };
    app.add_system_message(format!(
        "{}\n{}\n\n{}\n{}{}\n\n{}",
        msgs.skill_equipped_tmpl.replace("{}", &skill.name),
        desc,
        msgs.skill_preview_label,
        preview,
        ellipsis,
        msgs.skill_equipped_hint,
    ));
}

/// Handle `/skill-name` [args]: equip when args empty, otherwise run immediately.
pub(super) fn handle_skill_command(app: &mut App, cmd: &str) -> Option<CommandExecOutcome> {
    let skill = find_skill(app, cmd)?.clone();
    let args = skill_args_from_input(&app.input, &skill.name);
    app.slash_command.active = false;

    if args.is_empty() {
        app.input.clear();
        app.input_cursor = 0;
        equip_skill(app, &skill);
        return Some(CommandExecOutcome {
            handled: true,
            clear_input: false, // already cleared
        });
    }

    let agent_task = format_skill_agent_task(&skill, &args);
    if submit_user_task(app, args, agent_task) {
        app.input.clear();
        app.input_cursor = 0;
    }

    Some(CommandExecOutcome {
        handled: true,
        clear_input: false,
    })
}

/// Build agent task text for the current equipped skill (if any), without clearing it.
pub(super) fn peek_equipped_agent_task(app: &App, user_text: &str) -> String {
    if let Some(eq) = &app.equipped_skill {
        let entry = SkillEntry {
            name: eq.name.clone(),
            description: eq.description.clone(),
            body: eq.body.clone(),
        };
        format_skill_agent_task(&entry, user_text)
    } else {
        user_text.to_string()
    }
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
        assert!(out.contains("refactor foo"));
    }
}
