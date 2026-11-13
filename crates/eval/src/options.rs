//! How one run is configured.
//!
//! Model, endpoint, config file, and permission mode are process-global in
//! Tact: `tact::config::install` runs once, and `bootstrap_session` reads the
//! installed settings. So they belong to the run, not to a case, and a case
//! file that tried to set them would be a knob the in-process runner could not
//! honour.
//!
//! Both runners take this type. The host runner turns it into
//! [`tact::config::CliArgs`]; the CLI runner turns it into the matching flags
//! for `tact-ui`. One definition, so the two paths cannot drift.

use std::path::PathBuf;

pub use tact_protocol::PermissionMode;

/// The configuration one suite run executes under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunOptions {
    pub config: Option<PathBuf>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub base_url: Option<String>,
    pub permission_mode: PermissionMode,
}

/// Eval's only deviation from the canonical defaults: a suite runs in `auto`
/// unless told otherwise.
///
/// Headless cannot answer a permission prompt, so `default` would fail every
/// case that needs a tool, and a suite that silently ran with ask-first
/// semantics would look like a model failure. Stated here, once, rather than as
/// a re-declared `PermissionMode` whose `Default` disagreed with the product's.
impl Default for RunOptions {
    fn default() -> Self {
        Self {
            config: None,
            provider: None,
            model: None,
            base_url: None,
            permission_mode: PermissionMode::Auto,
        }
    }
}

impl RunOptions {
    /// The flags `tact-ui` and the host bootstrap both understand.
    ///
    /// Kept as one ordered list rather than two hand-written builders: a flag
    /// added for one runner must reach the other, and a single list is what
    /// makes that visible.
    pub fn to_flags(&self) -> Vec<String> {
        let mut flags = Vec::new();
        let mut push = |name: &str, value: &str| {
            flags.push(name.to_string());
            flags.push(value.to_string());
        };
        if let Some(config) = &self.config {
            push("--config", &config.display().to_string());
        }
        if let Some(provider) = &self.provider {
            push("--provider", provider);
        }
        if let Some(model) = &self.model {
            push("--model", model);
        }
        if let Some(base_url) = &self.base_url {
            push("--base-url", base_url);
        }
        push("--permission-mode", self.permission_mode.as_str());
        flags
    }

    /// The same configuration as a `CliArgs` for in-process use.
    pub fn to_cli_args(&self) -> tact::config::CliArgs {
        tact::config::CliArgs {
            config: self.config.clone(),
            provider: self.provider.clone(),
            model: self.model.clone(),
            base_url: self.base_url.clone(),
            permission_mode: Some(self.permission_mode),
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_carry_every_configured_option() {
        let options = RunOptions {
            config: Some(PathBuf::from("/tmp/cfg.toml")),
            provider: Some("openai".into()),
            model: Some("m".into()),
            base_url: Some("http://x".into()),
            permission_mode: PermissionMode::Plan,
        };
        assert_eq!(
            options.to_flags(),
            [
                "--config",
                "/tmp/cfg.toml",
                "--provider",
                "openai",
                "--model",
                "m",
                "--base-url",
                "http://x",
                "--permission-mode",
                "plan"
            ]
        );
    }

    #[test]
    fn an_unconfigured_run_asks_for_auto() {
        // The one place eval differs from the product's default of `Default`.
        let options = RunOptions::default();
        assert_eq!(options.permission_mode, PermissionMode::Auto);
        assert_eq!(options.to_flags(), ["--permission-mode", "auto"]);
    }

    #[test]
    fn flags_and_cli_args_agree_on_permission_mode() {
        for mode in [
            PermissionMode::Default,
            PermissionMode::Plan,
            PermissionMode::Auto,
        ] {
            let options = RunOptions {
                permission_mode: mode,
                ..Default::default()
            };
            assert_eq!(options.to_cli_args().permission_mode, Some(mode));
        }
    }
}
