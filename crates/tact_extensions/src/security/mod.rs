//! Security guards that sit above the permission ladder.
//!
//! Two independent mechanisms, because they fail independently:
//!
//! - [`sensitive`] — a registry of paths and commands that are secret by their
//!   nature, plus the classifier the permission layer consults. Its
//!   [`sensitive::Tier::Credential`] outcome is decided **before** modes, hooks
//!   and settings rules, so no `Auto` mode and no persisted `allow` can reach a
//!   private key.
//! - `redact` — a backstop for everything the classifier cannot see, applied to
//!   tool results before they reach the transcript and the session store.
//!
//! Neither is a sandbox. They are same-process, name-based heuristics; the real
//! boundary is `crate::sandbox` (Linux-only and opt-in today).
//!
//! # Configuration
//!
//! Both parts are configured from the `.tact/settings.json` document that
//! already holds the permission rules, under `permissions`:
//!
//! ```jsonc
//! {
//!   "permissions": {
//!     "sensitive_paths": {
//!       "enabled": true,
//!       "extra":  ["**/my-vault/**"],
//!       "allow":  ["~/.ssh/config"]
//!     },
//!     "redaction": {
//!       "enabled": true,
//!       "level": "basic",
//!       "extra_patterns": ["MYCO-[0-9a-f]{32}"],
//!       "basic_only_paths": ["**/fixtures/**"]
//!     }
//!   }
//! }
//! ```
//!
//! Every field is optional and every malformed value degrades to its default —
//! the same tolerance the rule lists have. Nothing here is configured through
//! `config.toml`: security rules live in one place.

pub mod sensitive;

use std::collections::BTreeMap;

use serde_json::Value;

use sensitive::Scanner;

// The redaction policy is owned by the Runtime Kernel, because the Kernel's
// trajectory recorder is a persistence sink and must not depend on this crate
// for the policy it applies. Re-exported here so every existing
// `crate::security::{RedactionConfig, RedactionLevel}` path keeps working.
pub use tact::redact::{RedactionConfig, RedactionLevel};

/// The redaction engine, under its historical module path.
pub mod redact {
    pub use tact::redact::*;
}

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

/// `permissions.sensitive_paths`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SensitivePathsConfig {
    /// `None` means "not set in this layer", so a project file can leave the
    /// global file's choice alone instead of overriding it with a default.
    pub enabled: Option<bool>,
    pub extra: Vec<String>,
    pub allow: Vec<String>,
}

impl SensitivePathsConfig {
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.enabled.unwrap_or(true)
    }
}

/// The parsed `permissions.sensitive_paths` + `permissions.redaction` pair.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SecurityConfig {
    pub sensitive_paths: SensitivePathsConfig,
    pub redaction: RedactionConfig,
}

impl SecurityConfig {
    /// Parse from a settings document (the whole file, not just `permissions`).
    ///
    /// Tolerant by construction: a missing section, a wrong type, or a
    /// malformed entry is warned about and skipped, never fatal. This mirrors
    /// `extract_rule_lists`, because a settings file that a user hand-edits
    /// must not be able to break tool dispatch.
    #[must_use]
    pub fn from_document(doc: &Value) -> Self {
        let Some(permissions) = doc.get("permissions").and_then(Value::as_object) else {
            return Self::default();
        };

        let sensitive_paths = match permissions.get("sensitive_paths") {
            Some(Value::Object(map)) => SensitivePathsConfig {
                enabled: bool_field(map.get("enabled"), "sensitive_paths.enabled"),
                extra: string_list(map.get("extra"), "sensitive_paths.extra"),
                allow: string_list(map.get("allow"), "sensitive_paths.allow"),
            },
            Some(other) => {
                warn_type(
                    "sensitive_paths",
                    "an object",
                    other,
                    "the guard stays on with no overrides",
                );
                SensitivePathsConfig::default()
            }
            None => SensitivePathsConfig::default(),
        };

        let redaction = match permissions.get("redaction") {
            Some(Value::Object(map)) => RedactionConfig {
                enabled: bool_field(map.get("enabled"), "redaction.enabled"),
                level: match map.get("level") {
                    Some(Value::String(s)) => match RedactionLevel::parse(s) {
                        Some(level) => Some(level),
                        None => {
                            tracing::warn!(
                                "Unknown permissions.redaction.level '{s}'; \
                                 falling back to 'basic'. Valid values: off, basic, credential."
                            );
                            None
                        }
                    },
                    Some(other) => {
                        warn_type(
                            "redaction.level",
                            "a string",
                            other,
                            "falling back to 'basic'",
                        );
                        None
                    }
                    None => None,
                },
                extra_patterns: string_list(map.get("extra_patterns"), "redaction.extra_patterns"),
                basic_only_paths: string_list(
                    map.get("basic_only_paths"),
                    "redaction.basic_only_paths",
                ),
            },
            Some(other) => {
                warn_type(
                    "redaction",
                    "an object",
                    other,
                    "redaction stays on at the default level",
                );
                RedactionConfig::default()
            }
            None => RedactionConfig::default(),
        };

        Self {
            sensitive_paths,
            redaction,
        }
    }

    /// Merge a global layer with a project layer. The project wins where it
    /// speaks; lists union, so a global allowance cannot be removed by a
    /// project file that simply does not mention it.
    #[must_use]
    pub fn merge(global: &Self, project: &Self) -> Self {
        Self {
            sensitive_paths: SensitivePathsConfig {
                enabled: project
                    .sensitive_paths
                    .enabled
                    .or(global.sensitive_paths.enabled),
                extra: union(
                    &global.sensitive_paths.extra,
                    &project.sensitive_paths.extra,
                ),
                allow: union(
                    &global.sensitive_paths.allow,
                    &project.sensitive_paths.allow,
                ),
            },
            redaction: RedactionConfig {
                enabled: project.redaction.enabled.or(global.redaction.enabled),
                level: project.redaction.level.or(global.redaction.level),
                extra_patterns: union(
                    &global.redaction.extra_patterns,
                    &project.redaction.extra_patterns,
                ),
                basic_only_paths: union(
                    &global.redaction.basic_only_paths,
                    &project.redaction.basic_only_paths,
                ),
            },
        }
    }

    /// Build the path guard this configuration describes.
    #[must_use]
    pub fn scanner(&self) -> Scanner {
        Scanner::from_parts(
            self.sensitive_paths.is_enabled(),
            self.sensitive_paths.extra.clone(),
            self.sensitive_paths.allow.clone(),
        )
    }
}

fn union(a: &[String], b: &[String]) -> Vec<String> {
    let mut seen = BTreeMap::new();
    for s in a.iter().chain(b) {
        seen.entry(s.clone()).or_insert(());
    }
    seen.into_keys().collect()
}

fn bool_field(value: Option<&Value>, field: &str) -> Option<bool> {
    match value {
        Some(Value::Bool(b)) => Some(*b),
        Some(other) => {
            warn_type(field, "a boolean", other, "ignoring it");
            None
        }
        None => None,
    }
}

/// A list of strings; non-string entries are warned about and dropped, and a
/// non-array value degrades to an empty list.
fn string_list(value: Option<&Value>, field: &str) -> Vec<String> {
    match value {
        Some(Value::Array(arr)) => arr
            .iter()
            .filter_map(|v| match v.as_str() {
                Some(s) => Some(s.to_string()),
                None => {
                    tracing::warn!("Ignoring non-string entry in permissions.{field}: {v}");
                    None
                }
            })
            .collect(),
        Some(other) => {
            warn_type(field, "an array of strings", other, "ignoring it");
            Vec::new()
        }
        None => Vec::new(),
    }
}

fn warn_type(field: &str, expected: &str, got: &Value, consequence: &str) {
    tracing::warn!(
        "permissions.{field} should be {expected}, got {}; {consequence}",
        type_name(got)
    );
}

fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn absent_sections_default_to_on_and_basic() {
        let cfg = SecurityConfig::from_document(&json!({}));
        assert!(cfg.sensitive_paths.is_enabled());
        assert!(cfg.sensitive_paths.extra.is_empty());
        assert!(cfg.sensitive_paths.allow.is_empty());
        assert!(cfg.redaction.is_enabled());
        assert_eq!(cfg.redaction.resolved_level(), RedactionLevel::Basic);
    }

    #[test]
    fn parses_a_full_document() {
        let cfg = SecurityConfig::from_document(&json!({
            "permissions": {
                "sensitive_paths": {
                    "enabled": false,
                    "extra": ["*.vault"],
                    "allow": ["~/.ssh/config"]
                },
                "redaction": {
                    "enabled": true,
                    "level": "credential",
                    "extra_patterns": ["MYCO-[0-9a-f]{32}"],
                    "basic_only_paths": ["**/fixtures/**"]
                }
            }
        }));
        assert!(!cfg.sensitive_paths.is_enabled());
        assert_eq!(cfg.sensitive_paths.extra, vec!["*.vault"]);
        assert_eq!(cfg.sensitive_paths.allow, vec!["~/.ssh/config"]);
        assert_eq!(cfg.redaction.resolved_level(), RedactionLevel::Credential);
        assert_eq!(cfg.redaction.extra_patterns.len(), 1);
        assert_eq!(cfg.redaction.basic_only_paths, vec!["**/fixtures/**"]);
    }

    #[test]
    fn an_unknown_level_falls_back_to_basic_not_off() {
        let cfg = SecurityConfig::from_document(&json!({
            "permissions": { "redaction": { "level": "Of" } }
        }));
        assert_eq!(
            cfg.redaction.resolved_level(),
            RedactionLevel::Basic,
            "a typo must never silently disable redaction"
        );
    }

    #[test]
    fn malformed_sections_are_ignored_not_fatal() {
        let cfg = SecurityConfig::from_document(&json!({
            "permissions": {
                "sensitive_paths": "yes please",
                "redaction": { "enabled": "yes", "extra_patterns": "MYCO", "level": 7 }
            }
        }));
        assert!(cfg.sensitive_paths.is_enabled(), "the guard stays on");
        assert!(cfg.sensitive_paths.extra.is_empty());
        assert!(cfg.redaction.is_enabled(), "redaction stays on");
        assert!(cfg.redaction.extra_patterns.is_empty());
        assert_eq!(cfg.redaction.resolved_level(), RedactionLevel::Basic);
    }

    #[test]
    fn non_string_entries_are_dropped() {
        let cfg = SecurityConfig::from_document(&json!({
            "permissions": { "sensitive_paths": { "allow": ["ok", 5, null] } }
        }));
        assert_eq!(cfg.sensitive_paths.allow, vec!["ok"]);
    }

    #[test]
    fn project_overrides_enabled_and_unions_lists() {
        let global = SecurityConfig::from_document(&json!({
            "permissions": {
                "sensitive_paths": { "extra": ["*.global"], "allow": ["~/.netrc"] },
                "redaction": { "level": "basic" }
            }
        }));
        let project = SecurityConfig::from_document(&json!({
            "permissions": {
                "sensitive_paths": { "enabled": false, "extra": ["*.project"] },
                "redaction": { "level": "credential" }
            }
        }));
        let merged = SecurityConfig::merge(&global, &project);
        assert!(!merged.sensitive_paths.is_enabled(), "project wins");
        assert_eq!(merged.sensitive_paths.extra, vec!["*.global", "*.project"]);
        assert_eq!(
            merged.sensitive_paths.allow,
            vec!["~/.netrc"],
            "a project file that stays silent must not remove a global allowance"
        );
        assert_eq!(
            merged.redaction.resolved_level(),
            RedactionLevel::Credential
        );
    }

    #[test]
    fn level_parses_its_spellings() {
        for (input, expected) in [
            ("off", RedactionLevel::Off),
            ("OFF", RedactionLevel::Off),
            ("none", RedactionLevel::Off),
            ("basic", RedactionLevel::Basic),
            ("credential", RedactionLevel::Credential),
            ("strict", RedactionLevel::Credential),
        ] {
            assert_eq!(RedactionLevel::parse(input), Some(expected), "{input}");
        }
        assert_eq!(RedactionLevel::parse("nonsense"), None);
    }
}
