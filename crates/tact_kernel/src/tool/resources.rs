//! Tool scheduling resources and path policies.
//!
//! A tool declares how it touches the workspace; the Agent's wave scheduler
//! turns those declarations into reads/writes/barrier claims. The types are
//! kernel contracts because they sit beside tool metadata and are consumed by
//! the scheduler without any extension-specific service.

use std::path::{Path, PathBuf};

use serde_json::Value;

/// The workspace files a tool reads and/or writes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolResources {
    pub reads: Vec<PathBuf>,
    pub writes: Vec<PathBuf>,
    /// `true` for tools whose effects cannot be scoped (bash, MCP, subagents,
    /// patch application, state mutations, …). A barrier conflicts with every
    /// other call and is therefore always scheduled alone.
    pub barrier: bool,
}

impl ToolResources {
    pub fn barrier() -> Self {
        Self {
            barrier: true,
            ..Default::default()
        }
    }

    pub fn independent() -> Self {
        Self::default()
    }
}

/// The resource claim a tool's input makes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourcePolicy {
    Independent,
    Barrier,
    ReadPath {
        field: &'static str,
    },
    WritePath {
        field: &'static str,
    },
    SharedState {
        scope: &'static str,
    },
    PatchFiles {
        patch_field: &'static str,
        dry_run_field: &'static str,
    },
}

impl ResourcePolicy {
    /// Resolve concrete tool input to workspace claims for wave scheduling.
    pub fn resolve(&self, input: &Value, work_dir: &Path) -> ToolResources {
        match self {
            ResourcePolicy::Independent => ToolResources::independent(),
            ResourcePolicy::Barrier => ToolResources::barrier(),
            ResourcePolicy::ReadPath { field } => {
                let mut resources = ToolResources::independent();
                if let Some(path) = input.get(*field).and_then(|value| value.as_str()) {
                    resources.reads.push(work_dir.join(path));
                }
                resources
            }
            ResourcePolicy::WritePath { field } => {
                let mut resources = ToolResources::independent();
                if let Some(path) = input.get(*field).and_then(|value| value.as_str()) {
                    resources.writes.push(work_dir.join(path));
                }
                resources
            }
            ResourcePolicy::SharedState { scope } => {
                let mut resources = ToolResources::independent();
                resources
                    .writes
                    .push(PathBuf::from(format!("__tact_{scope}__")));
                resources
            }
            ResourcePolicy::PatchFiles {
                patch_field,
                dry_run_field,
            } => {
                let mut resources = ToolResources::independent();
                if input
                    .get(*dry_run_field)
                    .and_then(|value| value.as_bool())
                    .unwrap_or(false)
                {
                    return resources;
                }
                if let Some(patch) = input.get(*patch_field).and_then(|value| value.as_str()) {
                    for path in patch::extract_patch_paths(patch) {
                        resources.writes.push(work_dir.join(path));
                    }
                }
                resources
            }
        }
    }

    /// Extract recent file paths for display purposes.
    pub fn recent_paths(&self, input: &Value) -> Vec<String> {
        match self {
            ResourcePolicy::ReadPath { field } | ResourcePolicy::WritePath { field } => input
                .get(*field)
                .and_then(|value| value.as_str())
                .map(|path| vec![path.to_string()])
                .unwrap_or_default(),
            ResourcePolicy::PatchFiles { patch_field, .. } => input
                .get(*patch_field)
                .and_then(|value| value.as_str())
                .map(patch::extract_patch_paths)
                .unwrap_or_default(),
            _ => Vec::new(),
        }
    }
}

pub mod patch {
    //! Unified-diff path extraction.
    //!
    //! One parser is shared by the permission policy, the "always allow" rule
    //! generator and the scheduler so they cannot disagree about which files a
    //! patch touches.

    /// The target paths a unified diff names.
    #[must_use]
    pub fn patch_target_paths(patch: &str) -> Vec<String> {
        extract_patch_paths(patch)
    }

    /// All target paths a unified diff names.
    ///
    /// Handles both `+++ b/<path>` and a bare `+++ <path>`; `/dev/null` marks a
    /// deletion and is skipped.
    #[must_use]
    pub fn extract_patch_paths(patch: &str) -> Vec<String> {
        patch
            .lines()
            .filter_map(|line| {
                let rest = line
                    .strip_prefix("+++ b/")
                    .or_else(|| line.strip_prefix("+++ "))?;
                let path = rest.split('\t').next().unwrap_or(rest).trim();
                if path.is_empty() || path == "/dev/null" {
                    return None;
                }
                Some(path.to_string())
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn path_resource_policy_resolves_and_reports_recent_path() {
        let policy = ResourcePolicy::WritePath { field: "path" };
        let input = json!({ "path": "src/lib.rs" });
        let resources = policy.resolve(&input, Path::new("/repo"));

        assert_eq!(resources.writes, vec![Path::new("/repo/src/lib.rs")]);
        assert_eq!(policy.recent_paths(&input), vec!["src/lib.rs"]);
    }

    #[test]
    fn patch_target_paths_extracts_git_and_bare_headers() {
        assert_eq!(
            patch::patch_target_paths("+++ b/src/lib.rs\n+++ /dev/null\n"),
            vec!["src/lib.rs"]
        );
        assert_eq!(
            patch::patch_target_paths("+++ secrets.json\n"),
            vec!["secrets.json"]
        );
    }

    #[test]
    fn barrier_and_independent_claims_are_distinct() {
        assert!(!ToolResources::independent().barrier);
        assert!(ToolResources::barrier().barrier);
    }
}
