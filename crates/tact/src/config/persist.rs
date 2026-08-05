//! In-place TOML helpers for optional `/model` persistence.
//!
//! Uses `toml_edit` to preserve comments and original formatting when
//! rewriting only the targeted keys.

use std::path::Path;

use anyhow::Context as _;

/// Rewrite `path` after applying `set` to the table reached by walking
/// `keys` (creating missing tables as needed).
///
/// Uses `toml_edit::DocumentMut` to preserve comments and formatting.
fn update_toml<F>(path: &Path, keys: &[&str], set: F) -> anyhow::Result<()>
where
    F: FnOnce(&mut toml_edit::Table) -> anyhow::Result<()>,
{
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("cannot read config file {:?}", path))?;
    let mut doc: toml_edit::DocumentMut = content
        .parse()
        .with_context(|| format!("parse error in config file {:?}", path))?;

    // Walk `keys`, creating missing intermediate tables along the way.
    let mut table = doc.as_table_mut();
    let mut walked = String::from("config root");
    for key in keys {
        let entry = table
            .entry(key)
            .or_insert_with(|| toml_edit::Item::Table(toml_edit::Table::new()));
        table = entry
            .as_table_mut()
            .ok_or_else(|| anyhow::anyhow!("{walked}.{key} must be a table"))?;
        walked.push('.');
        walked.push_str(key);
    }

    set(table)?;

    let serialized = doc.to_string();
    std::fs::write(path, serialized)
        .with_context(|| format!("cannot write config file {:?}", path))?;
    Ok(())
}

/// Set `llm.providers.<provider>.model` in `path` and rewrite the file.
pub(super) fn update_provider_model_in_toml(
    path: &Path,
    provider: &str,
    model: &str,
) -> anyhow::Result<()> {
    update_toml(path, &["llm", "providers", provider], |t| {
        t.insert("model", toml_edit::value(model));
        Ok(())
    })
}

/// Set `llm.providers.<provider>.model` and `thinking_budget` in `path` and rewrite the file.
pub(super) fn update_provider_model_and_thinking_budget_in_toml(
    path: &Path,
    provider: &str,
    model: &str,
    thinking_budget: usize,
) -> anyhow::Result<()> {
    let budget = i64::try_from(thinking_budget)
        .map_err(|_| anyhow::anyhow!("thinking_budget exceeds TOML integer range"))?;
    update_toml(path, &["llm", "providers", provider], |t| {
        t.insert("model", toml_edit::value(model));
        t.insert("thinking_budget", toml_edit::value(budget));
        Ok(())
    })
}

/// Set `agent.subagent.model` and `thinking_budget` in `path` and rewrite the file.
pub(super) fn update_subagent_model_in_toml(
    path: &Path,
    model: &str,
    thinking_budget: usize,
) -> anyhow::Result<()> {
    let budget = i64::try_from(thinking_budget)
        .map_err(|_| anyhow::anyhow!("thinking_budget exceeds TOML integer range"))?;
    update_toml(path, &["agent", "subagent"], |t| {
        t.insert("model", toml_edit::value(model));
        t.insert("thinking_budget", toml_edit::value(budget));
        Ok(())
    })
}

/// Set `[llm.providers.<name>].model` + `reasoning_effort` in `path`.
pub(super) fn update_provider_model_and_reasoning_effort_in_toml(
    path: &Path,
    provider: &str,
    model: &str,
    effort: &str,
) -> anyhow::Result<()> {
    update_toml(path, &["llm", "providers", provider], |t| {
        t.insert("model", toml_edit::value(model));
        t.insert("reasoning_effort", toml_edit::value(effort));
        Ok(())
    })
}

/// Set `[agent.subagent].model` + `reasoning_effort` in `path`.
pub(super) fn update_subagent_model_and_reasoning_effort_in_toml(
    path: &Path,
    model: &str,
    effort: &str,
) -> anyhow::Result<()> {
    update_toml(path, &["agent", "subagent"], |t| {
        t.insert("model", toml_edit::value(model));
        t.insert("reasoning_effort", toml_edit::value(effort));
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    #[test]
    fn updates_model_under_active_provider_section() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let mut f = std::fs::File::create(&path).unwrap();
        write!(
            f,
            r#"[llm]
provider = "kimi"

[llm.providers.kimi]
api_key = "sk-test"
model = "old-model"
models = ["old-model", "new-model"]

[llm.providers.openai]
api_key = "sk-other"
model = "gpt-4o"
"#
        )
        .unwrap();

        update_provider_model_in_toml(&path, "kimi", "new-model").unwrap();

        let updated = std::fs::read_to_string(&path).unwrap();
        let cfg: toml::Value = updated.parse().unwrap();
        assert_eq!(
            cfg["llm"]["providers"]["kimi"]["model"].as_str(),
            Some("new-model")
        );
        assert_eq!(
            cfg["llm"]["providers"]["openai"]["model"].as_str(),
            Some("gpt-4o")
        );
        assert_eq!(
            cfg["llm"]["providers"]["kimi"]["api_key"].as_str(),
            Some("sk-test")
        );
    }

    #[test]
    fn preserves_comments_and_formatting() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let original = r#"# Tact configuration
[llm]
# Default provider
provider = "kimi"

[llm.providers.kimi]
# Your API key from https://platform.moonshot.cn
api_key = "sk-test"
model = "old-model"
models = ["old-model", "new-model"]
  # inline comment after array

# OpenAI fallback (unused by default)
[llm.providers.openai]
api_key = "sk-other"
model = "gpt-4o"
"#;
        std::fs::write(&path, original).unwrap();

        update_provider_model_in_toml(&path, "kimi", "new-model").unwrap();

        let updated = std::fs::read_to_string(&path).unwrap();
        // Verify comments are preserved
        assert!(updated.contains("# Tact configuration"));
        assert!(updated.contains("# Default provider"));
        assert!(updated.contains("# Your API key from https://platform.moonshot.cn"));
        assert!(updated.contains("# inline comment after array"));
        assert!(updated.contains("# OpenAI fallback (unused by default)"));
        // Verify the model was updated
        assert!(updated.contains("model = \"new-model\""));
    }

    #[test]
    fn updates_model_and_thinking_budget_under_active_provider_section() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            r#"[llm]
provider = "kimi"

[llm.providers.kimi]
api_key = "sk-test"
model = "old-model"
thinking_budget = 32000

[llm.providers.openai]
api_key = "sk-other"
model = "gpt-4o"
thinking_budget = 64000
"#,
        )
        .unwrap();

        update_provider_model_and_thinking_budget_in_toml(&path, "kimi", "kimi-for-coding", 64_000)
            .unwrap();

        let cfg: toml::Value = std::fs::read_to_string(&path).unwrap().parse().unwrap();
        assert_eq!(
            cfg["llm"]["providers"]["kimi"]["model"].as_str(),
            Some("kimi-for-coding")
        );
        assert_eq!(
            cfg["llm"]["providers"]["kimi"]["thinking_budget"].as_integer(),
            Some(64_000)
        );
        assert_eq!(
            cfg["llm"]["providers"]["openai"]["model"].as_str(),
            Some("gpt-4o")
        );
        assert_eq!(
            cfg["llm"]["providers"]["openai"]["thinking_budget"].as_integer(),
            Some(64_000)
        );
    }

    #[test]
    fn persists_zero_budget_instead_of_omitting_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[llm.providers.kimi]\nmodel = \"old\"\n").unwrap();

        update_provider_model_and_thinking_budget_in_toml(&path, "kimi", "new", 0).unwrap();

        let cfg: toml::Value = std::fs::read_to_string(&path).unwrap().parse().unwrap();
        assert_eq!(
            cfg["llm"]["providers"]["kimi"]["model"].as_str(),
            Some("new")
        );
        assert_eq!(
            cfg["llm"]["providers"]["kimi"]["thinking_budget"].as_integer(),
            Some(0)
        );
    }
}
