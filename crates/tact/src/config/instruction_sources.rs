//! Resolved set of project instruction files to inject into the system prompt.

/// Which instruction files to load into the system prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InstructionSource {
    /// Project / subdir `AGENTS.md` → `# Additional context`.
    AgentsMd,
    /// `~/.claude/CLAUDE.md` → inside `# Additional context` (before AGENTS.md).
    ClaudeMdUser,
    /// `<workdir>/CLAUDE.md` → inside `# Additional context`.
    ClaudeMdProject,
    /// `<cwd>/CLAUDE.md` when cwd differs from workdir
    ClaudeMdSubdir,
}

/// Resolved instruction-source flags (from `[agent].instruction_sources` in config).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstructionSources {
    pub agents_md: bool,
    pub claude_user: bool,
    pub claude_project: bool,
    pub claude_subdir: bool,
}

impl Default for InstructionSources {
    fn default() -> Self {
        Self { agents_md: true, claude_user: false, claude_project: false, claude_subdir: false }
    }
}

impl InstructionSources {
    /// Parse TOML `instruction_sources` list. Default: `["agents_md"]`.
    ///
    /// Shorthand `claude_md` enables all three CLAUDE.md discovery paths.
    pub fn from_config(values: Option<Vec<String>>) -> Result<Self, String> {
        let values = values.unwrap_or_else(|| vec!["agents_md".to_string()]);
        if values.is_empty() {
            return Err("instruction_sources must not be empty".into());
        }

        let mut out = Self { agents_md: false, claude_user: false, claude_project: false, claude_subdir: false };

        for raw in values {
            let key = raw.trim();
            if key.is_empty() {
                continue;
            }
            match key {
                "agents_md" => out.agents_md = true,
                "claude_md" => {
                    out.claude_user = true;
                    out.claude_project = true;
                    out.claude_subdir = true;
                },
                "claude_md_user" => out.claude_user = true,
                "claude_md_project" => out.claude_project = true,
                "claude_md_subdir" => out.claude_subdir = true,
                other => {
                    return Err(format!(
                        "unknown instruction_sources entry '{other}' \
                         (expected agents_md, claude_md, claude_md_user, \
                         claude_md_project, or claude_md_subdir)"
                    ));
                },
            }
        }

        if !out.agents_md && !out.claude_user && !out.claude_project && !out.claude_subdir {
            return Err("instruction_sources must enable at least one source".into());
        }

        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_agents_md_only() {
        let s = InstructionSources::default();
        assert!(s.agents_md);
        assert!(!s.claude_user);
        assert!(!s.claude_project);
        assert!(!s.claude_subdir);
    }

    #[test]
    fn claude_md_shorthand_enables_all_claude_paths() {
        let s = InstructionSources::from_config(Some(vec!["agents_md".into(), "claude_md".into()])).unwrap();
        assert!(s.agents_md);
        assert!(s.claude_user);
        assert!(s.claude_project);
        assert!(s.claude_subdir);
    }

    #[test]
    fn rejects_unknown_key() {
        assert!(InstructionSources::from_config(Some(vec!["foo".into()])).is_err());
    }
}
