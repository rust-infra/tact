use std::path::PathBuf;

use anyhow::Context as _;

use super::types::TactTomlConfig;

/// Template written to `~/.tact/config.toml` on first run when no config file
/// exists anywhere. Embedded at compile time so the default stays in sync
/// with the checked-in example — editing `config.example.toml` updates the
/// first-run template too.
const DEFAULT_CONFIG_TEMPLATE: &str = include_str!("../../../../config.example.toml");

fn config_search_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();

    let cwd = std::env::current_dir().unwrap_or_default();
    paths.push(cwd.join(".tact").join("config.toml"));
    paths.push(cwd.join("config.toml"));

    if let Some(home) = dirs_next_home() {
        paths.push(home.join(".tact").join("config.toml"));
    }

    paths
}

fn dirs_next_home() -> Option<PathBuf> {
    std::env::var("HOME")
        .ok()
        .map(PathBuf::from)
        .or_else(|| std::env::var("USERPROFILE").ok().map(PathBuf::from))
}

/// Write the default template to `~/.tact/config.toml` and return its path.
///
/// Only the user-global location is auto-created: the project-level
/// candidates are skipped so a repo is never polluted with a generated file.
/// Returns `None` when the home directory is unknown or the file cannot be
/// written — callers then fall back to empty defaults and the regular
/// "not configured" resolve error guides the user.
fn write_default_config() -> Option<PathBuf> {
    let home = dirs_next_home()?;
    let dir = home.join(".tact");
    let path = dir.join("config.toml");
    if path.exists() {
        return Some(path);
    }
    std::fs::create_dir_all(&dir).ok()?;
    std::fs::write(&path, DEFAULT_CONFIG_TEMPLATE).ok()?;
    eprintln!(
        "[config] no config found; wrote default template to {} — edit it to add your API key",
        path.display()
    );
    Some(path)
}

/// Load TOML config and return the path that was actually read (if any).
pub(super) fn load_toml_config(
    path: Option<&PathBuf>,
) -> anyhow::Result<(TactTomlConfig, Option<PathBuf>)> {
    if let Some(p) = path {
        let content = std::fs::read_to_string(p)
            .with_context(|| format!("cannot read config file {:?}", p))?;
        let cfg: TactTomlConfig = toml::from_str(&content)
            .with_context(|| format!("parse error in config file {:?}", p))?;
        // eprintln!("[config] loaded {:?}", p);
        return Ok((cfg, Some(p.clone())));
    }

    for p in config_search_paths() {
        if !p.exists() {
            continue;
        }
        let content = std::fs::read_to_string(&p)
            .with_context(|| format!("cannot read config file {:?}", p))?;
        let cfg: TactTomlConfig = toml::from_str(&content)
            .with_context(|| format!("parse error in config file {:?}", p))?;
        // eprintln!("[config] loaded {:?}", p);
        return Ok((cfg, Some(p)));
    }

    // First run: no config anywhere. Write a default template to
    // ~/.tact/config.toml so the user has a concrete file to edit.
    if let Some(p) = write_default_config() {
        let content = std::fs::read_to_string(&p)
            .with_context(|| format!("cannot read config file {:?}", p))?;
        let cfg: TactTomlConfig = toml::from_str(&content)
            .with_context(|| format!("parse error in config file {:?}", p))?;
        return Ok((cfg, Some(p)));
    }

    Ok((TactTomlConfig::default(), None))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::sync::Mutex;

    /// HOME is process-global; serialize tests that mutate it so they cannot
    /// observe each other's environment.
    static HOME_LOCK: Mutex<()> = Mutex::new(());

    fn test_home(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("tact-config-load-{name}-{}", std::process::id()))
    }

    fn set_home(home: &PathBuf) {
        // Edition 2024: setting env vars is unsafe.
        unsafe { std::env::set_var("HOME", home) };
    }

    /// Restore the previous HOME. Never removes the variable unless it was
    /// already absent, because other tests (e.g. skill resolution) run in
    /// parallel and expect HOME to exist.
    fn restore_home(prev: Option<OsString>) {
        match prev {
            Some(v) => unsafe { std::env::set_var("HOME", v) },
            None => unsafe { std::env::remove_var("HOME") },
        }
    }

    fn snapshot_home() -> Option<OsString> {
        std::env::var_os("HOME")
    }

    #[test]
    fn first_run_writes_default_config_to_home() {
        let _guard = HOME_LOCK.lock().unwrap();
        let prev = snapshot_home();
        let home = test_home("first-run");
        std::fs::create_dir_all(&home).unwrap();
        set_home(&home);

        let (cfg, path) = load_toml_config(None).unwrap();

        restore_home(prev);
        let path = path.expect("first run should produce a config path");
        assert_eq!(path, home.join(".tact").join("config.toml"));
        assert!(path.exists(), "default config should be written");
        // Template tracks config.example.toml: deepseek active, no responses.
        assert_eq!(cfg.llm.provider.as_deref(), Some("deepseek"));
        let written = std::fs::read_to_string(&path).unwrap();
        assert!(written.contains("[llm.providers.deepseek]"));
        assert!(written.contains("protocol = \"chat_completions\""));
        std::fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn existing_home_config_is_never_overwritten() {
        let _guard = HOME_LOCK.lock().unwrap();
        let prev = snapshot_home();
        let home = test_home("existing");
        let dir = home.join(".tact");
        std::fs::create_dir_all(&dir).unwrap();
        let cfg_path = dir.join("config.toml");
        std::fs::write(&cfg_path, "[llm]\nprovider = \"openai\"\n").unwrap();
        set_home(&home);

        let (cfg, path) = load_toml_config(None).unwrap();

        restore_home(prev);
        assert_eq!(cfg.llm.provider.as_deref(), Some("openai"));
        assert_eq!(path.as_deref(), Some(cfg_path.as_path()));
        let content = std::fs::read_to_string(&cfg_path).unwrap();
        assert_eq!(content, "[llm]\nprovider = \"openai\"\n");
        std::fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn unwritable_home_falls_back_to_empty_defaults() {
        let _guard = HOME_LOCK.lock().unwrap();
        let prev = snapshot_home();
        // HOME pointing at a regular file makes create_dir_all(<home>/.tact)
        // fail, exercising the write-failure fallback. HOME stays set so
        // parallel tests that expect the variable to exist keep passing.
        let home_file = test_home("unwritable");
        std::fs::write(&home_file, "not a directory").unwrap();
        set_home(&home_file);

        let (cfg, path) = load_toml_config(None).unwrap();

        restore_home(prev);
        std::fs::remove_file(&home_file).unwrap();
        assert!(path.is_none());
        assert_eq!(cfg.llm.provider, None);
    }
}
