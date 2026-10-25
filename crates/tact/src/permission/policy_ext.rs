//! Permission-policy classification kept in the extension crate.
//!
//! The kernel owns the declarative `PermissionPolicy` enum and its pure
//! `target` parser. Deciding a concrete input's risk needs the security
//! scanner and the shell read-only classifier, which are extension concerns;
//! this trait keeps those methods available at the call sites that used to
//! call them inherently.

use serde_json::Value;

use crate::permission::CapabilityRisk;
use crate::security::sensitive::{Hit, Scanner};
use crate::tool::PermissionPolicy;

pub trait PermissionPolicyExt {
    fn resolve(&self, input: &Value) -> CapabilityRisk;
    fn sensitive(&self, input: &Value, scanner: &Scanner) -> Option<Hit>;
}

impl PermissionPolicyExt for PermissionPolicy {
    fn resolve(&self, input: &Value) -> CapabilityRisk {
        match self {
            PermissionPolicy::Read => CapabilityRisk::Read,
            PermissionPolicy::Write => CapabilityRisk::Write,
            PermissionPolicy::High => CapabilityRisk::High,
            PermissionPolicy::ReadPath { path_field } => {
                if path_target_is_sensitive(input, path_field) {
                    CapabilityRisk::High
                } else {
                    CapabilityRisk::Read
                }
            }
            PermissionPolicy::WritePath { path_field } => {
                if path_target_is_sensitive(input, path_field) {
                    CapabilityRisk::High
                } else {
                    CapabilityRisk::Write
                }
            }
            PermissionPolicy::PatchPaths => {
                if patch_targets_are_sensitive(input) {
                    CapabilityRisk::High
                } else {
                    CapabilityRisk::Write
                }
            }
            PermissionPolicy::ShellCommand { command_field } => {
                let command = input
                    .get(*command_field)
                    .and_then(|value| value.as_str())
                    .unwrap_or("");
                if crate::security::sensitive::classify_command(command).is_some() {
                    return CapabilityRisk::High;
                }
                if command.starts_with("sudo ") || command.starts_with("su ") {
                    CapabilityRisk::High
                } else if crate::tool::readonly_shell::is_read_only_shell_command(command) {
                    CapabilityRisk::Read
                } else {
                    CapabilityRisk::Write
                }
            }
        }
    }

    fn sensitive(&self, input: &Value, scanner: &Scanner) -> Option<Hit> {
        match self {
            PermissionPolicy::ReadPath { .. } | PermissionPolicy::WritePath { .. } => {
                let raw = self.target(input)?;
                if !target_is_workspace_relative(&raw) {
                    return None;
                }
                scanner.classify(&raw)
            }
            PermissionPolicy::PatchPaths => crate::tool::patch_target_paths(
                input.get("patch").and_then(|value| value.as_str())?,
            )
            .into_iter()
            .find_map(|path| scanner.classify(&path)),
            PermissionPolicy::ShellCommand { .. } => {
                let command = self.target(input)?;
                scanner.classify_command(&command)
            }
            PermissionPolicy::Read | PermissionPolicy::Write | PermissionPolicy::High => None,
        }
    }
}

fn target_is_workspace_relative(raw: &str) -> bool {
    let raw = raw.trim();
    !(raw.starts_with('/') || raw.starts_with('~'))
}

fn path_target_is_sensitive(input: &Value, path_field: &str) -> bool {
    input
        .get(path_field)
        .and_then(|value| value.as_str())
        .is_some_and(|raw| {
            target_is_workspace_relative(raw)
                && crate::security::sensitive::classify_path(raw).is_some()
        })
}

fn patch_targets_are_sensitive(input: &Value) -> bool {
    input
        .get("patch")
        .and_then(|value| value.as_str())
        .map(crate::tool::patch_target_paths)
        .unwrap_or_default()
        .iter()
        .any(|path| crate::security::sensitive::classify_path(path).is_some())
}
