//! Syntax highlighting for `/skill-name` [args] — app-layer wrapper.
//!
//! The pure functions moved to `agent_tui_kit::render::slash_style`; this
//! module injects the app-layer builtin-command set and re-exports the kit
//! functions so existing call sites keep their paths.

use std::collections::HashSet;

pub(crate) use agent_tui_kit::render::slash_style::style_user_skill_line;

use crate::widgets::state::{SkillEntry, SlashCommand};

/// Builtin palette commands that must not be treated as skills.
fn builtin_command_names() -> HashSet<&'static str> {
    SlashCommand::ALL.iter().map(|cmd| cmd.name()).collect()
}

/// Skill names eligible for slash highlighting / matching (excludes builtins).
pub(crate) fn skill_name_set(skills: &[SkillEntry]) -> HashSet<&str> {
    let builtins = builtin_command_names();
    agent_tui_kit::render::slash_style::skill_name_set(skills, &builtins)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skill_name_set_excludes_builtin_names() {
        let skills = vec![
            SkillEntry {
                name: "help".into(),
                description: "skill help".into(),
                body: "x".into(),
            },
            SkillEntry {
                name: "demo".into(),
                description: "d".into(),
                body: "y".into(),
            },
        ];
        let names = skill_name_set(&skills);
        assert!(!names.contains("help"));
        assert!(names.contains("demo"));
    }

    /// The kit hardcodes the gather command (it owns no command table); this is
    /// the link that makes a rename fail loudly here instead of silently
    /// unhighlighting `/skill <name>` everywhere.
    #[test]
    fn the_kit_gather_command_is_the_hosts_skill_command() {
        assert_eq!(
            agent_tui_kit::render::slash_style::SKILL_COMMAND,
            SlashCommand::Skill.name()
        );
    }

    #[test]
    fn the_skill_command_form_highlights_the_skill_name_too() {
        use agent_tui_kit::render::slash_style::split_skill_slash;

        let skills = vec![SkillEntry {
            name: "demo".into(),
            description: "d".into(),
            body: "y".into(),
        }];
        let names = skill_name_set(&skills);

        let (token, args) = split_skill_slash("/skill demo fix auth", &names).unwrap();

        assert_eq!(token, "/skill demo");
        assert_eq!(args, " fix auth");
    }
}
