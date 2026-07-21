use std::path::PathBuf;

use anyhow::Context as _;

use super::types::TactTomlConfig;

fn config_search_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();

    let cwd = std::env::current_dir().unwrap_or_default();
    paths.push(cwd.join(".tact").join("config.toml"));
    paths.push(cwd.join("tact.toml"));

    if let Some(home) = dirs_next_home() {
        paths.push(home.join(".tact").join("config.toml"));
    }

    paths
}

fn dirs_next_home() -> Option<PathBuf> {
    std::env::var("HOME").ok().map(PathBuf::from).or_else(|| std::env::var("USERPROFILE").ok().map(PathBuf::from))
}

/// Load TOML config and return the path that was actually read (if any).
pub(super) fn load_toml_config(path: Option<&PathBuf>) -> anyhow::Result<(TactTomlConfig, Option<PathBuf>)> {
    if let Some(p) = path {
        let content = std::fs::read_to_string(p).with_context(|| format!("cannot read config file {:?}", p))?;
        let cfg: TactTomlConfig =
            toml::from_str(&content).with_context(|| format!("parse error in config file {:?}", p))?;
        eprintln!("[config] loaded {:?}", p);
        return Ok((cfg, Some(p.clone())));
    }

    for p in config_search_paths() {
        if !p.exists() {
            continue;
        }
        let content = std::fs::read_to_string(&p).with_context(|| format!("cannot read config file {:?}", p))?;
        let cfg: TactTomlConfig =
            toml::from_str(&content).with_context(|| format!("parse error in config file {:?}", p))?;
        eprintln!("[config] loaded {:?}", p);
        return Ok((cfg, Some(p)));
    }

    Ok((TactTomlConfig::default(), None))
}
