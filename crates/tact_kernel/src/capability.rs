//! Capability identity and filtering.
//!
//! A capability is one contribution a plugin makes to the kernel, such as a
//! tool or a prompt section. IDs are stable strings so a user can disable one
//! capability without disabling its whole plugin.

pub mod command;
pub mod prompt;
pub mod tool;
pub mod view;

use std::collections::BTreeSet;
use std::str::FromStr;

use anyhow::{Result, bail};

use crate::plugin::PluginId;

/// Validate a capability's short name.
///
/// Capability names and aliases are unquoted labels: no whitespace and no
/// ASCII control characters. The catalog and the registrar both use this so a
/// direct catalog insertion cannot bypass the rule.
pub(crate) fn validate_name(name: &str) -> Result<()> {
    if name.is_empty() {
        bail!("capability name cannot be empty");
    }
    if name.chars().any(char::is_whitespace) {
        bail!("capability name cannot contain whitespace: {name}");
    }
    if name.chars().any(|c| c.is_ascii_control()) {
        bail!("capability name cannot contain control characters: {name}");
    }
    Ok(())
}

/// The kind of capability a plugin contributes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CapabilityKind {
    Tool,
    Command,
    PromptSection,
    View,
    Agent,
    Chat,
    Session,
    Permission,
    Security,
    Hook,
}

impl CapabilityKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Tool => "tool",
            Self::Command => "command",
            Self::PromptSection => "prompt",
            Self::View => "view",
            Self::Agent => "agent",
            Self::Chat => "chat",
            Self::Session => "session",
            Self::Permission => "permission",
            Self::Security => "security",
            Self::Hook => "hook",
        }
    }
}

impl FromStr for CapabilityKind {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "tool" => Ok(Self::Tool),
            "command" => Ok(Self::Command),
            "prompt" => Ok(Self::PromptSection),
            "view" => Ok(Self::View),
            "agent" => Ok(Self::Agent),
            "chat" => Ok(Self::Chat),
            "session" => Ok(Self::Session),
            "permission" => Ok(Self::Permission),
            "security" => Ok(Self::Security),
            "hook" => Ok(Self::Hook),
            _ => bail!("unknown capability kind: {value}"),
        }
    }
}

/// Stable identity of one capability contribution.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct CapabilityId {
    pub kind: CapabilityKind,
    pub name: String,
}

impl CapabilityId {
    pub fn new(kind: CapabilityKind, name: impl Into<String>) -> Self {
        Self {
            kind,
            name: name.into(),
        }
    }
}

impl std::fmt::Display for CapabilityId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.kind.as_str(), self.name)
    }
}

impl FromStr for CapabilityId {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        let Some((kind, name)) = value.split_once(':') else {
            bail!("capability id needs <kind>:<name>: {value}");
        };
        if name.is_empty() {
            bail!("capability id needs a non-empty name: {value}");
        }
        validate_name(name)?;
        Ok(Self {
            kind: kind.parse()?,
            name: name.to_owned(),
        })
    }
}

/// The capabilities a runtime chose not to register.
#[derive(Debug, Clone, Default)]
pub struct CapabilityFilter {
    disabled: BTreeSet<CapabilityId>,
}

impl CapabilityFilter {
    pub fn new(disabled: impl IntoIterator<Item = CapabilityId>) -> Self {
        Self {
            disabled: disabled.into_iter().collect(),
        }
    }

    pub fn allows(&self, id: &CapabilityId) -> bool {
        !self.disabled.contains(id)
    }

    pub fn disabled(&self) -> impl Iterator<Item = &CapabilityId> {
        self.disabled.iter()
    }
}

/// Why a capability never reached its catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedCapability {
    pub plugin: PluginId,
    pub capability: CapabilityId,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capability_id_round_trips() {
        let id: CapabilityId = "tool:bash".parse().unwrap();
        assert_eq!(id.kind, CapabilityKind::Tool);
        assert_eq!(id.name, "bash");
        assert_eq!(id.to_string(), "tool:bash");
    }

    #[test]
    fn capability_id_rejects_unknown_kind_and_empty_name() {
        assert!("unknown:x".parse::<CapabilityId>().is_err());
        assert!("tool:".parse::<CapabilityId>().is_err());
        assert!("tool".parse::<CapabilityId>().is_err());
    }

    #[test]
    fn filter_allows_unlisted_and_rejects_listed() {
        let disabled: CapabilityId = "tool:bash".parse().unwrap();
        let filter = CapabilityFilter::new([disabled.clone()]);
        assert!(filter.allows(&"tool:read_file".parse().unwrap()));
        assert!(!filter.allows(&disabled));
        assert_eq!(filter.disabled().collect::<Vec<_>>(), vec![&disabled]);
    }
}
