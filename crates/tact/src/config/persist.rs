//! In-place TOML helpers for optional `/model` and thinking-budget persistence.

use std::path::Path;

use anyhow::Context as _;

/// Set `llm.providers.<provider>` model and thinking budget in `path` and rewrite the file.
///
/// Uses `toml::Value` round-trip; comments and original formatting may be lost.
pub(super) fn update_provider_model_and_thinking_budget_in_toml(
    path: &Path,
    provider: &str,
    model: &str,
    thinking_budget: usize,
) -> anyhow::Result<()> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("cannot read config file {:?}", path))?;
    let mut value: toml::Value = content
        .parse()
        .with_context(|| format!("parse error in config file {:?}", path))?;

    let llm = value
        .as_table_mut()
        .ok_or_else(|| anyhow::anyhow!("config root must be a table"))?
        .entry("llm".to_string())
        .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
    let llm_table = llm
        .as_table_mut()
        .ok_or_else(|| anyhow::anyhow!("llm must be a table"))?;
    let providers = llm_table
        .entry("providers".to_string())
        .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
    let providers_table = providers
        .as_table_mut()
        .ok_or_else(|| anyhow::anyhow!("llm.providers must be a table"))?;
    let entry = providers_table
        .entry(provider.to_string())
        .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
    let entry_table = entry
        .as_table_mut()
        .ok_or_else(|| anyhow::anyhow!("llm.providers.{provider} must be a table"))?;
    entry_table.insert("model".into(), toml::Value::String(model.to_string()));
    let thinking_budget = i64::try_from(thinking_budget)
        .map_err(|_| anyhow::anyhow!("thinking_budget exceeds TOML integer range"))?;
    entry_table.insert(
        "thinking_budget".into(),
        toml::Value::Integer(thinking_budget),
    );

    let serialized =
        toml::to_string_pretty(&value).with_context(|| format!("serialize config {:?}", path))?;
    std::fs::write(path, serialized)
        .with_context(|| format!("cannot write config file {:?}", path))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
