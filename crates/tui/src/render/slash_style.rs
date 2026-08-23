//! Syntax highlighting for `/skill-name` [args] — app-layer wrapper.
//!
//! The pure functions moved to `agent_tui_kit::render::slash_style`; this
//! module injects the app-layer builtin-command set (`PALETTE_COMMANDS`) and
//! re-exports the kit functions so existing call sites keep their paths.

use std::collections::HashSet;

pub(crate) use agent_tui_kit::render::slash_style::style_user_skill_line;

use crate::widgets::state::SkillEntry;

/// Builtin palette commands that must not be treated as skills.
fn builtin_command_names() -> HashSet<&'static str> {
    crate::widgets::state::PALETTE_COMMANDS
        .iter()
        .map(|(n, _)| *n)
        .collect()
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
}
