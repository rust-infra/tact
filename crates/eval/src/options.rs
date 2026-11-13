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

/// The configuration one suite run executes under.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunOptions {
    pub config: Option<PathBuf>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub base_url: Option<String>,
    /// `default`, `plan`, or `auto`.
    pub permission_mode: PermissionMode,
}

/// The three permission modes, mirrored here so `RunOptions` is constructible
/// without a clap parser.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PermissionMode {
    Default,
    Plan,
    #[default]
    Auto,
}

impl PermissionMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Plan => "plan",
            Self::Auto => "auto",
        }
    }
}

impl std::str::FromStr for PermissionMode {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "default" => Ok(Self::Default),
            "plan" => Ok(Self::Plan),
            "auto" => Ok(Self::Auto),
            other => Err(format!(
                "unknown permission mode {other:?}; expected default, plan, or auto"
            )),
        }
    }
}

impl RunOptions {
    /// The flags `tact-ui` and the host bootstrap both understand.
    ///
    /// Kept as an ordered list rather than two hand-written builders: a flag
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
            permission_mode: Some(self.permission_mode.into()),
            ..Default::default()
        }
    }
}

impl From<PermissionMode> for tact::permission::PermissionMode {
    fn from(mode: PermissionMode) -> Self {
        match mode {
            PermissionMode::Default => Self::Default,
            PermissionMode::Plan => Self::Plan,
            PermissionMode::Auto => Self::Auto,
        }
    }
}

impl From<tact::permission::PermissionMode> for PermissionMode {
    fn from(mode: tact::permission::PermissionMode) -> Self {
        match mode {
            tact::permission::PermissionMode::Default => Self::Default,
            tact::permission::PermissionMode::Plan => Self::Plan,
            tact::permission::PermissionMode::Auto => Self::Auto,
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
    fn unset_options_are_omitted_rather_than_sent_blank() {
        let options = RunOptions::default();
        assert_eq!(options.to_flags(), ["--permission-mode", "auto"]);
    }

    #[test]
    fn cli_args_and_flags_agree_on_permission_mode() {
        for mode in [
            PermissionMode::Default,
            PermissionMode::Plan,
            PermissionMode::Auto,
        ] {
            let options = RunOptions {
                permission_mode: mode,
                ..Default::default()
            };
            let expected = tact::permission::PermissionMode::from(mode);
            assert_eq!(options.to_cli_args().permission_mode, Some(expected));
        }
    }
}
