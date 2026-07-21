//! Syntax highlighting for `/skill-name` [args] in the input box and user log lines.

use crate::theme::Theme;
use crate::widgets::state::SkillEntry;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use std::collections::HashSet;

/// Split a line that starts with a known skill slash into `(skill_token, rest)`.
/// `skill_token` includes the leading `/` (e.g. `/demo-test`); `rest` may start with spaces.
pub(crate) fn split_skill_slash<'a>(
    line: &'a str,
    skill_names: &HashSet<&str>,
) -> Option<(&'a str, &'a str)> {
    let trimmed = line.trim_start();
    let lead = line.len() - trimmed.len();
    if !trimmed.starts_with('/') {
        return None;
    }
    let after_slash = &trimmed[1..];
    let cmd_len = after_slash
        .find(|c: char| c.is_whitespace())
        .unwrap_or(after_slash.len());
    if cmd_len == 0 {
        return None;
    }
    let cmd = &after_slash[..cmd_len];
    if !skill_names.contains(cmd) {
        return None;
    }
    let skill_end_in_trimmed = 1 + cmd_len;
    let skill_end = lead + skill_end_in_trimmed;
    Some((&line[..skill_end], &line[skill_end..]))
}

/// Skill names eligible for slash highlighting / matching (excludes built-ins).
pub(crate) fn skill_name_set(skills: &[SkillEntry]) -> HashSet<&str> {
    let builtins: HashSet<&str> = crate::widgets::state::PALETTE_COMMANDS
        .iter()
        .map(|(n, _)| *n)
        .collect();
    skills
        .iter()
        .map(|s| s.name.as_str())
        .filter(|n| !builtins.contains(n))
        .collect()
}

/// Static prefix of an i18n template like `"💬 {}"` → `"💬 "`.
fn template_prefix(tmpl: &str) -> Option<&str> {
    tmpl.split_once("{}").map(|(pre, _)| pre)
}

/// Style `/skill args` for the input box.
pub(crate) fn style_input_skill_line(
    line: &str,
    skill_names: &HashSet<&str>,
    theme: &Theme,
) -> Option<Line<'static>> {
    let (skill, rest) = split_skill_slash(line, skill_names)?;
    let mut spans = Vec::with_capacity(2);
    spans.push(Span::styled(
        skill.to_string(),
        Style::default()
            .fg(theme.accent)
            .bg(theme.input_box_bg)
            .add_modifier(Modifier::BOLD),
    ));
    if !rest.is_empty() {
        spans.push(Span::styled(
            rest.to_string(),
            Style::default().fg(theme.fg).bg(theme.input_box_bg),
        ));
    }
    Some(Line::from(spans))
}

/// Style a user log line built from `user_msg_prefix` / `user_msg_cont` templates.
///
/// For skill invocations (`/skill-name`), the first-line prefix is changed from `💬`
/// (user chat bubble) to `⚡` (lightning bolt — "running a command") with `warning`
/// color to make plugin slash commands visually distinct from regular user messages.
pub(crate) fn style_user_skill_line(
    raw: &str,
    skill_names: &HashSet<&str>,
    theme: &Theme,
    user_prefix_tmpl: &str,
    user_cont_tmpl: &str,
) -> Option<Line<'static>> {
    let (lead, payload) = {
        let pre = template_prefix(user_prefix_tmpl)?;
        if let Some(rest) = raw.strip_prefix(pre) {
            (pre, rest)
        } else {
            let pre = template_prefix(user_cont_tmpl)?;
            let rest = raw.strip_prefix(pre)?;
            (pre, rest)
        }
    };

    let (skill, args) = split_skill_slash(payload, skill_names)?;
    let mut spans = Vec::with_capacity(3);

    // First line of a skill invocation: use ⚡ prefix with warning color to stand out.
    if lead.contains('💬') {
        spans.push(Span::styled(
            "⚡ ".to_string(),
            Style::default()
                .fg(theme.warning)
                .add_modifier(Modifier::BOLD),
        ));
    } else {
        spans.push(Span::styled(
            lead.to_string(),
            Style::default().fg(theme.success),
        ));
    }
    spans.push(Span::styled(
        skill.to_string(),
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD),
    ));
    if !args.is_empty() {
        spans.push(Span::styled(
            args.to_string(),
            Style::default().fg(theme.fg),
        ));
    }
    Some(Line::from(spans))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{Theme, ThemeName};

    #[test]
    fn split_skill_slash_separates_args() {
        let names: HashSet<&str> = ["demo-test"].into_iter().collect();
        let (skill, args) = split_skill_slash("/demo-test hi there", &names).unwrap();
        assert_eq!(skill, "/demo-test");
        assert_eq!(args, " hi there");
    }

    #[test]
    fn split_skill_slash_rejects_unknown() {
        let names: HashSet<&str> = ["demo-test"].into_iter().collect();
        assert!(split_skill_slash("/quit now", &names).is_none());
    }

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

    #[test]
    fn style_input_skill_line_uses_distinct_colors() {
        let theme = Theme::from(ThemeName::Japanese);
        let names: HashSet<&str> = ["demo-test"].into_iter().collect();
        let line = style_input_skill_line("/demo-test hi", &names, &theme).unwrap();
        assert_eq!(line.spans.len(), 2);
        assert_eq!(line.spans[0].content.as_ref(), "/demo-test");
        assert_eq!(line.spans[0].style.fg, Some(theme.accent));
        assert!(line.spans[0].style.add_modifier.contains(Modifier::BOLD));
        assert_eq!(line.spans[1].content.as_ref(), " hi");
        assert_eq!(line.spans[1].style.fg, Some(theme.fg));
        assert_ne!(theme.accent, theme.fg);
    }

    #[test]
    fn style_user_skill_line_uses_i18n_prefix() {
        let theme = Theme::from(ThemeName::Japanese);
        let names: HashSet<&str> = ["demo"].into_iter().collect();
        let line = style_user_skill_line("💬 /demo hi", &names, &theme, "💬 {}", "  {}").unwrap();
        assert_eq!(line.spans[0].content.as_ref(), "⚡ ");
        assert_eq!(
            line.spans[0].style.fg,
            Some(theme.warning),
            "skill prefix should use warning color"
        );
        assert_eq!(line.spans[1].content.as_ref(), "/demo");
        assert_eq!(line.spans[2].content.as_ref(), " hi");
    }

    #[test]
    fn style_continuation_skill_line_uses_original_lead() {
        let theme = Theme::from(ThemeName::Japanese);
        let names: HashSet<&str> = ["demo"].into_iter().collect();
        let line =
            style_user_skill_line("  /demo hi", &names, &theme, "💬 {}", "  {}").unwrap();
        assert_eq!(line.spans[0].content.as_ref(), "  ");
        assert_eq!(
            line.spans[0].style.fg,
            Some(theme.success),
            "continuation lead should keep success color"
        );
    }
}
