use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};

use anyhow::{Context, Result};
use clap::Parser;
use serde::{Deserialize, Serialize};
use tact::config::CliArgs;

use crate::{CatalogLocale, DesktopStartError};

static PROCESS_CONFIG_INSTALLS: AtomicUsize = AtomicUsize::new(0);

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Appearance {
    #[default]
    Light,
    Dark,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReducedMotion {
    #[serde(alias = "respect")]
    #[default]
    System,
    Always,
    Never,
}

impl ReducedMotion {
    pub const fn reduce(self, system: bool) -> bool {
        match self {
            Self::System => system,
            Self::Always => true,
            Self::Never => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum InitialSession {
    None,
    New,
    ResumeLast,
    Resume(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostLaunchOptions {
    pub config_path: Option<PathBuf>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub api_key: Option<String>,
    pub base_url: Option<String>,
    pub max_tokens: Option<u32>,
    pub model_context_window: Option<usize>,
    pub snapshot_max_items: Option<usize>,
    pub micro_compact: Option<bool>,
    pub tokio_console: bool,
    pub skill_body_auto_inject: bool,
    pub permission_mode: PermissionModeView,
}

impl Default for HostLaunchOptions {
    fn default() -> Self {
        Self {
            config_path: None,
            provider: None,
            model: None,
            api_key: None,
            base_url: None,
            max_tokens: None,
            model_context_window: None,
            snapshot_max_items: None,
            micro_compact: None,
            tokio_console: false,
            skill_body_auto_inject: false,
            permission_mode: PermissionModeView::Default,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionModeView {
    #[default]
    Default,
    Plan,
    Auto,
}

#[derive(Debug, Clone)]
pub struct DesktopConfig {
    pub app_data_dir: PathBuf,
    pub initial_project: Option<PathBuf>,
    pub initial_session: InitialSession,
    pub host: HostLaunchOptions,
    pub update_batch: Duration,
    pub gate_reconcile: Duration,
    pub appearance: Appearance,
    pub language: CatalogLocale,
    pub reduced_motion: ReducedMotion,
    pub text_scale: f32,
    pub notifications: bool,
    pub show_hook_output: bool,
}

#[derive(Serialize, Deserialize)]
struct DesktopSettingsFile {
    initial_project: Option<PathBuf>,
    appearance: Appearance,
    language: String,
    reduced_motion: ReducedMotion,
    #[serde(default = "default_text_scale")]
    text_scale: f32,
    #[serde(default = "default_true")]
    notifications: bool,
    #[serde(default = "default_true")]
    show_hook_output: bool,
    #[serde(default)]
    host: HostLaunchOptions,
}

impl DesktopConfig {
    pub fn load(app_data_dir: PathBuf) -> Result<Self, DesktopStartError> {
        fs::create_dir_all(&app_data_dir)
            .map_err(|error| DesktopStartError::Config(error.to_string()))?;
        let path = app_data_dir.join("desktop-settings.json");
        let persisted = if path.exists() {
            let raw = fs::read_to_string(&path)
                .map_err(|error| DesktopStartError::Config(error.to_string()))?;
            Some(
                serde_json::from_str::<DesktopSettingsFile>(&raw)
                    .map_err(|error| DesktopStartError::Config(error.to_string()))?,
            )
        } else {
            None
        };
        Ok(Self {
            app_data_dir,
            initial_project: persisted
                .as_ref()
                .and_then(|settings| settings.initial_project.clone()),
            initial_session: InitialSession::ResumeLast,
            update_batch: Duration::from_millis(33),
            gate_reconcile: Duration::from_millis(100),
            appearance: persisted
                .as_ref()
                .map(|settings| settings.appearance)
                .unwrap_or_default(),
            language: persisted
                .as_ref()
                .map(|settings| match settings.language.as_str() {
                    "zh" => CatalogLocale::Zh,
                    _ => CatalogLocale::En,
                })
                .unwrap_or(CatalogLocale::En),
            reduced_motion: persisted
                .as_ref()
                .map(|settings| settings.reduced_motion)
                .unwrap_or_default(),
            text_scale: persisted
                .as_ref()
                .map(|settings| settings.text_scale)
                .unwrap_or(1.0)
                .clamp(1.0, 1.5),
            notifications: persisted
                .as_ref()
                .map(|settings| settings.notifications)
                .unwrap_or(true),
            show_hook_output: persisted
                .as_ref()
                .map(|settings| settings.show_hook_output)
                .unwrap_or(true),
            host: persisted
                .as_ref()
                .map(|settings| settings.host.clone())
                .unwrap_or_default(),
        })
    }

    pub fn save(&self) -> Result<(), DesktopStartError> {
        fs::create_dir_all(&self.app_data_dir)
            .map_err(|error| DesktopStartError::Config(error.to_string()))?;
        let persisted = DesktopSettingsFile {
            initial_project: self.initial_project.clone(),
            appearance: self.appearance,
            language: match self.language {
                CatalogLocale::En => "en".to_string(),
                CatalogLocale::Zh => "zh".to_string(),
            },
            reduced_motion: self.reduced_motion,
            text_scale: self.text_scale,
            notifications: self.notifications,
            show_hook_output: self.show_hook_output,
            host: self.host.clone(),
        };
        let bytes = serde_json::to_vec_pretty(&persisted)
            .map_err(|error| DesktopStartError::Config(error.to_string()))?;
        let path = self.app_data_dir.join("desktop-settings.json");
        let temp = self.app_data_dir.join("desktop-settings.json.tmp");
        fs::write(&temp, bytes)
            .and_then(|()| fs::rename(temp, path))
            .map_err(|error| DesktopStartError::Config(error.to_string()))
    }

    pub fn install_process_config(&self) -> Result<(), DesktopStartError> {
        let args = self.cli_args();
        tact_host::init_config(&args)
            .map_err(|error| DesktopStartError::Config(error.to_string()))?;
        PROCESS_CONFIG_INSTALLS.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    pub fn persist_subagent_model(&self, model: &str) -> Result<(), DesktopStartError> {
        let budget = tact::config::try_settings()
            .map(|settings| settings.agent.thinking_budget)
            .unwrap_or(0);
        tact::config::persist_subagent_model(model, budget)
            .map_err(|error| DesktopStartError::Config(error.to_string()))
    }

    pub fn with_initial_project(mut self, root: PathBuf) -> Self {
        self.initial_project = Some(root);
        self
    }

    pub fn with_initial_session(mut self, session: InitialSession) -> Self {
        self.initial_session = session;
        self
    }

    pub fn cli_args(&self) -> CliArgs {
        let mut args = CliArgs::parse_from(["tact-desktop"]);
        args.config = self.host.config_path.clone();
        args.provider = self.host.provider.clone();
        args.model = self.host.model.clone();
        args.api_key = self.host.api_key.clone();
        args.base_url = self.host.base_url.clone();
        args.max_tokens = self.host.max_tokens;
        args.model_context_window = self.host.model_context_window;
        args.snapshot_max_items = self.host.snapshot_max_items;
        args.no_micro_compact = self.host.micro_compact == Some(false);
        args.tokio_console = self.host.tokio_console;
        args.skill_body_auto_inject = self.host.skill_body_auto_inject;
        args.permission_mode = Some(match self.host.permission_mode {
            PermissionModeView::Default => tact_protocol::PermissionMode::Default,
            PermissionModeView::Plan => tact_protocol::PermissionMode::Plan,
            PermissionModeView::Auto => tact_protocol::PermissionMode::Auto,
        });
        args
    }

    pub fn process_config_install_count() -> usize {
        PROCESS_CONFIG_INSTALLS.load(Ordering::SeqCst)
    }
}

const fn default_text_scale() -> f32 {
    1.0
}

const fn default_true() -> bool {
    true
}

pub fn app_support_dir() -> Result<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        let home = std::env::var_os("HOME").context("HOME is not set")?;
        return Ok(Path::new(&home)
            .join("Library")
            .join("Application Support")
            .join("Tact"));
    }
    #[cfg(target_os = "windows")]
    {
        let base = std::env::var_os("APPDATA")
            .or_else(|| std::env::var_os("LOCALAPPDATA"))
            .context("APPDATA is not set")?;
        return Ok(Path::new(&base).join("Tact"));
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let base = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|home| Path::new(&home).join(".local").join("share"))
            })
            .context("XDG_DATA_HOME and HOME are not set")?;
        return Ok(base.join("tact"));
    }
    #[allow(unreachable_code)]
    Err(anyhow::anyhow!(
        "unsupported platform for Tact Desktop app-data"
    ))
}

#[cfg(test)]
mod tests {
    use super::ReducedMotion;

    #[test]
    fn reduced_motion_preference_composes_with_the_system_signal() {
        assert!(ReducedMotion::Always.reduce(false));
        assert!(!ReducedMotion::Never.reduce(true));
        assert!(ReducedMotion::System.reduce(true));
        assert!(!ReducedMotion::System.reduce(false));
    }
}
