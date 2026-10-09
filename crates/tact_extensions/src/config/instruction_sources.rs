//! Resolved set of project instruction files to inject into the system prompt.

/// Which instruction files to load into the system prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InstructionSource {
    /// Project / subdir `AGENTS.md` → `# Additional context`.
    AgentsMd,
}

/// Resolved instruction-source flags (from `[agent].instruction_sources` in config).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstructionSources {
    pub agents_md: bool,
}

impl Default for InstructionSources {
    fn default() -> Self {
        Self { agents_md: true }
    }
}

impl InstructionSources {
    /// Parse TOML `instruction_sources` list. Default: `["agents_md"]`.
    pub fn from_config(values: Option<Vec<String>>) -> Result<Self, String> {
        let values = values.unwrap_or_else(|| vec!["agents_md".to_string()]);
        if values.is_empty() {
            return Err("instruction_sources must not be empty".into());
        }

        let mut out = Self { agents_md: false };

        for raw in values {
            let key = raw.trim();
            if key.is_empty() {
                continue;
            }
            match key {
                "agents_md" => out.agents_md = true,
                other => {
                    return Err(format!(
                        "unknown instruction_sources entry '{other}' \
                         (expected agents_md)"
                    ));
                }
            }
        }

        if !out.agents_md {
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
    }

    #[test]
    fn rejects_unknown_key() {
        assert!(InstructionSources::from_config(Some(vec!["claude_md".into()])).is_err());
        assert!(InstructionSources::from_config(Some(vec!["foo".into()])).is_err());
    }
}
