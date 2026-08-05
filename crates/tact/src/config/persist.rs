//! In-place TOML helpers for optional `/model` persistence.

use std::path::Path;

use anyhow::Context as _;

/// Rewrite `path` after applying `set` to the table reached by walking
/// `keys` (creating missing tables as needed).
///
/// Uses `toml::Value` round-trip; comments and original formatting may be lost.
fn update_toml<F>(path: &Path, keys: &[&str], set: F) -> anyhow::Result<()>
where
    F: FnOnce(&mut toml::map::Map<String, toml::Value>) -> anyhow::Result<()>,
{
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("cannot read config file {:?}", path))?;
    let mut value: toml::Value = content
        .parse()
        .with_context(|| format!("parse error in config file {:?}", path))?;

    let root = value
        .as_table_mut()
        .ok_or_else(|| anyhow::anyhow!("config root must be a table"))?;

    // Walk `keys`, creating missing intermediate tables along the way.
    let mut table = root;
    let mut walked = String::from("config root");
    for key in keys {
        let entry = table
            .entry((*key).to_string())
            .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
        table = entry
            .as_table_mut()
            .ok_or_else(|| anyhow::anyhow!("{walked}.{key} must be a table"))?;
        walked.push('.');
        walked.push_str(key);
    }

    set(table)?;

    let serialized =
        toml::to_string_pretty(&value).with_context(|| format!("serialize config {:?}", path))?;
    std::fs::write(path, serialized)
        .with_context(|| format!("cannot write config file {:?}", path))?;
    Ok(())
}

/// Set `llm.providers.<provider>.model` in `path` and rewrite the file.
///
/// Uses `toml::Value` round-trip; comments and original formatting may be lost.
pub(super) fn update_provider_model_in_toml(
    path: &Path,
    provider: &str,
    model: &str,
) -> anyhow::Result<()> {
    update_toml(path, &["llm", "providers", provider], |t| {
        t.insert("model".into(), toml::Value::String(model.to_string()));
        Ok(())
    })
}

/// Set `llm.providers.<provider>.model` and `thinking_budget` in `path` and rewrite the file.
///
/// Uses `toml::Value` round-trip; comments and original formatting may be lost.
pub(super) fn update_provider_model_and_thinking_budget_in_toml(
    path: &Path,
    provider: &str,
    model: &str,
    thinking_budget: usize,
) -> anyhow::Result<()> {
    let budget = i64::try_from(thinking_budget)
        .map_err(|_| anyhow::anyhow!("thinking_budget exceeds TOML integer range"))?;
    update_toml(path, &["llm", "providers", provider], |t| {
        t.insert("model".into(), toml::Value::String(model.to_string()));
        t.insert("thinking_budget".into(), toml::Value::Integer(budget));
        Ok(())
    })
}

/// Set `agent.subagent.model` and `thinking_budget` in `path` and rewrite the file.
///
/// Uses `toml::Value` round-trip; comments and original formatting may be lost.
pub(super) fn update_subagent_model_in_toml(
    path: &Path,
    model: &str,
    thinking_budget: usize,
) -> anyhow::Result<()> {
    let budget = i64::try_from(thinking_budget)
        .map_err(|_| anyhow::anyhow!("thinking_budget exceeds TOML integer range"))?;
    update_toml(path, &["agent", "subagent"], |t| {
        t.insert("model".into(), toml::Value::String(model.to_string()));
        t.insert("thinking_budget".into(), toml::Value::Integer(budget));
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
        t.insert("model".into(), toml::Value::String(model.to_string()));
        t.insert(
            "reasoning_effort".into(),
            toml::Value::String(effort.to_string()),
        );
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
        t.insert("model".into(), toml::Value::String(model.to_string()));
        t.insert(
            "reasoning_effort".into(),
            toml::Value::String(effort.to_string()),
        );
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
