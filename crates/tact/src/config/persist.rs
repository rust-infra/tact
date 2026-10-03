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

/// Insert a scalar at `key`, keeping the decoration of the line it replaces.
///
/// `Table::insert` swaps the whole item, which takes the *old* item's decor
/// with it: a `model = "x"  # why` line lost its comment on every `/model`
/// write. Replacing only the value keeps the user's comment (and its
/// alignment) where they put it.
fn set_scalar(table: &mut toml_edit::Table, key: &str, value: impl Into<toml_edit::Value>) {
    let mut item = toml_edit::Item::Value(value.into());
    if let (Some(existing), Some(new)) = (
        table.get(key).and_then(toml_edit::Item::as_value),
        item.as_value_mut(),
    ) {
        *new.decor_mut() = existing.decor().clone();
    }
    table.insert(key, item);
}

/// Set `ui.theme` in `path` and rewrite the file.
///
/// `theme` is the canonical name (`ThemeName::as_str`), which is what the
/// resolver reads back on the next launch.
pub(super) fn update_ui_theme_in_toml(path: &Path, theme: &str) -> anyhow::Result<()> {
    update_toml(path, &["ui"], |t| {
        set_scalar(t, "theme", theme);
        Ok(())
    })
}

/// Set `ui.language` in `path` and rewrite the file.
///
/// `language` is the canonical name (`Language::as_str`), the same string
/// `TactTomlConfig.ui.language` resolves back into a locale on the next launch.
pub(super) fn update_ui_language_in_toml(path: &Path, language: &str) -> anyhow::Result<()> {
    update_toml(path, &["ui"], |t| {
        set_scalar(t, "language", language);
        Ok(())
    })
}

/// Set `llm.providers.<provider>.model` in `path` and rewrite the file.
pub(super) fn update_provider_model_in_toml(
    path: &Path,
    provider: &str,
    model: &str,
) -> anyhow::Result<()> {
    update_toml(path, &["llm", "providers", provider], |t| {
        set_scalar(t, "model", model);
        Ok(())
    })
}

/// Set `llm.providers.<provider>.model` and `thinking_budget` in `path` and rewrite the file.
///
/// Budget and effort semantics are mutually exclusive: writing a budget also
/// removes any stale `reasoning_effort` from the provider entry.
pub(super) fn update_provider_model_and_thinking_budget_in_toml(
    path: &Path,
    provider: &str,
    model: &str,
    thinking_budget: usize,
) -> anyhow::Result<()> {
    let budget = i64::try_from(thinking_budget)
        .map_err(|_| anyhow::anyhow!("thinking_budget exceeds TOML integer range"))?;
    update_toml(path, &["llm", "providers", provider], |t| {
        set_scalar(t, "model", model);
        set_scalar(t, "thinking_budget", budget);
        t.remove("reasoning_effort");
        Ok(())
    })
}

/// Set `agent.subagent.model` and `thinking_budget` in `path` and rewrite the file.
///
/// Writing a budget also removes any stale `reasoning_effort` (mutually
/// exclusive semantics, same as the main agent).
pub(super) fn update_subagent_model_in_toml(
    path: &Path,
    model: &str,
    thinking_budget: usize,
) -> anyhow::Result<()> {
    let budget = i64::try_from(thinking_budget)
        .map_err(|_| anyhow::anyhow!("thinking_budget exceeds TOML integer range"))?;
    update_toml(path, &["agent", "subagent"], |t| {
        set_scalar(t, "model", model);
        set_scalar(t, "thinking_budget", budget);
        t.remove("reasoning_effort");
        Ok(())
    })
}

/// Set `[llm.providers.<name>].model` + `reasoning_effort` in `path`.
///
/// Writing an effort also removes any stale `thinking_budget` from the provider
/// entry, so a model switch to effort semantics does not leave a meaningless
/// `think (32K)` behind in the status bar.
pub(super) fn update_provider_model_and_reasoning_effort_in_toml(
    path: &Path,
    provider: &str,
    model: &str,
    effort: &str,
) -> anyhow::Result<()> {
    update_toml(path, &["llm", "providers", provider], |t| {
        set_scalar(t, "model", model);
        set_scalar(t, "reasoning_effort", effort);
        t.remove("thinking_budget");
        Ok(())
    })
}

/// Set `[agent.subagent].model` + `reasoning_effort` in `path`.
///
/// Writing an effort also removes any stale `thinking_budget` (mutually
/// exclusive semantics, same as the main agent).
pub(super) fn update_subagent_model_and_reasoning_effort_in_toml(
    path: &Path,
    model: &str,
    effort: &str,
) -> anyhow::Result<()> {
    update_toml(path, &["agent", "subagent"], |t| {
        set_scalar(t, "model", model);
        set_scalar(t, "reasoning_effort", effort);
        t.remove("thinking_budget");
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    #[test]
    fn updates_ui_theme_keeping_the_rest_of_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            r#"[ui]
theme = "ink"   # keep my comment

[llm]
provider = "kimi"
"#,
        )
        .unwrap();

        update_ui_theme_in_toml(&path, "nord").unwrap();

        let updated = std::fs::read_to_string(&path).unwrap();
        let cfg: toml::Value = updated.parse().unwrap();
        assert_eq!(cfg["ui"]["theme"].as_str(), Some("nord"));
        assert_eq!(cfg["llm"]["provider"].as_str(), Some("kimi"));
        // `toml_edit` keeps the comment: the user's file is edited, not
        // regenerated.
        assert!(updated.contains("# keep my comment"), "{updated}");
    }

    #[test]
    fn ui_theme_is_created_when_the_table_is_missing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[llm]\nprovider = \"kimi\"\n").unwrap();

        update_ui_theme_in_toml(&path, "kawaii").unwrap();

        let cfg: toml::Value = std::fs::read_to_string(&path).unwrap().parse().unwrap();
        assert_eq!(cfg["ui"]["theme"].as_str(), Some("kawaii"));
    }

    /// The theme's own line must survive a language write: both live in `[ui]`,
    /// so a rewrite that rebuilt the table would drop the other preference.
    #[test]
    fn updates_ui_language_keeping_the_theme_line() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[ui]\ntheme = \"nord\"  # chosen by /theme\n").unwrap();

        update_ui_language_in_toml(&path, "zh").unwrap();

        let updated = std::fs::read_to_string(&path).unwrap();
        let cfg: toml::Value = updated.parse().unwrap();
        assert_eq!(cfg["ui"]["language"].as_str(), Some("zh"));
        assert_eq!(cfg["ui"]["theme"].as_str(), Some("nord"));
        assert!(updated.contains("# chosen by /theme"), "{updated}");
    }

    #[test]
    fn ui_language_is_created_when_the_table_is_missing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[llm]\nprovider = \"kimi\"\n").unwrap();

        update_ui_language_in_toml(&path, "zh").unwrap();

        let cfg: toml::Value = std::fs::read_to_string(&path).unwrap().parse().unwrap();
        assert_eq!(cfg["ui"]["language"].as_str(), Some("zh"));
        assert_eq!(cfg["llm"]["provider"].as_str(), Some("kimi"));
    }

    /// The `/model` write used to swap the whole item, taking the line's
    /// trailing comment with it — the user's note about *why* they pinned that
    /// model disappeared on the first `/model`.
    #[test]
    fn replacing_a_model_keeps_its_trailing_comment() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "[llm]\nprovider = \"kimi\"\n\n[llm.providers.kimi]\nmodel = \"old\"  # pinned for the ctx window\n",
        )
        .unwrap();

        update_provider_model_in_toml(&path, "kimi", "new").unwrap();

        let updated = std::fs::read_to_string(&path).unwrap();
        assert!(updated.contains("# pinned for the ctx window"), "{updated}");
        let cfg: toml::Value = updated.parse().unwrap();
        assert_eq!(
            cfg["llm"]["providers"]["kimi"]["model"].as_str(),
            Some("new")
        );
    }

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

    #[test]
    fn persisting_effort_removes_stale_thinking_budget() {
        // Regression: switching an effort-semantic model must not leave a
        // stale thinking_budget in config.toml — it would resurface as a
        // meaningless `think high(32K)` after restart.
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
"#,
        )
        .unwrap();

        update_provider_model_and_reasoning_effort_in_toml(&path, "kimi", "k3", "high").unwrap();

        let cfg: toml::Value = std::fs::read_to_string(&path).unwrap().parse().unwrap();
        assert_eq!(
            cfg["llm"]["providers"]["kimi"]["model"].as_str(),
            Some("k3")
        );
        assert_eq!(
            cfg["llm"]["providers"]["kimi"]["reasoning_effort"].as_str(),
            Some("high")
        );
        assert!(
            cfg["llm"]["providers"]["kimi"]
                .get("thinking_budget")
                .is_none(),
            "effort persist must remove the stale thinking_budget"
        );
    }

    #[test]
    fn persisting_budget_removes_stale_reasoning_effort() {
        // Regression: switching to a budget-semantic model must not leave a
        // stale reasoning_effort in config.toml (mutually exclusive semantics).
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            r#"[llm]
provider = "kimi"

[llm.providers.kimi]
api_key = "sk-test"
model = "k3"
reasoning_effort = "high"
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
        assert!(
            cfg["llm"]["providers"]["kimi"]
                .get("reasoning_effort")
                .is_none(),
            "budget persist must remove the stale reasoning_effort"
        );
    }
}
