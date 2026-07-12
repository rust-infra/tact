//! In-place TOML helpers for optional `/model` persistence.

use std::path::Path;

use anyhow::Context as _;

/// Set `llm.providers.<provider>.model` in `path` and rewrite the file.
///
/// Uses `toml::Value` round-trip; comments and original formatting may be lost.
pub(super) fn update_provider_model_in_toml(
    path: &Path,
    provider: &str,
    model: &str,
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

    let serialized =
        toml::to_string_pretty(&value).with_context(|| format!("serialize config {:?}", path))?;
    std::fs::write(path, serialized)
        .with_context(|| format!("cannot write config file {:?}", path))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

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
}
