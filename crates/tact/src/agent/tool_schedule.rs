//! Product resource-scope contracts consumed by tool metadata and the kernel
//! tool adapter.

use std::path::PathBuf;

/// The workspace files a tool reads and/or writes, used by the product adapter
/// to produce kernel `ResourceAccess`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolResources {
    pub reads: Vec<PathBuf>,
    pub writes: Vec<PathBuf>,
    /// `true` for tools whose effects cannot be scoped (bash, MCP listings,
    /// subagents without worktrees, patch application, state mutations, ...).
    pub barrier: bool,
}

impl ToolResources {
    pub fn barrier() -> Self {
        Self {
            barrier: true,
            ..Default::default()
        }
    }

    /// A tool that touches no workspace file (for example `web_search`).
    pub fn independent() -> Self {
        Self::default()
    }
}

/// MCP tools on the same server serialize; different servers may run in
/// parallel.
pub(crate) fn mcp_server_resources(server: &str) -> ToolResources {
    if server.is_empty() {
        return ToolResources::barrier();
    }
    ToolResources {
        writes: vec![PathBuf::from(format!("__mcp__{server}"))],
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    #[test]
    fn resources_read_file_path_is_normalized() {
        let resources = crate::tool::ResourcePolicy::ReadPath { field: "path" }.resolve(
            &serde_json::json!({"path": "src/a.rs"}),
            std::path::Path::new("/work"),
        );
        assert_eq!(resources.reads, vec![PathBuf::from("/work/src/a.rs")]);
        assert!(resources.writes.is_empty() && !resources.barrier);
    }

    #[test]
    fn resources_absolute_path_kept() {
        let resources = crate::tool::ResourcePolicy::ReadPath { field: "path" }.resolve(
            &serde_json::json!({"path": "/abs/a.rs"}),
            std::path::Path::new("/work"),
        );
        assert_eq!(resources.reads, vec![PathBuf::from("/abs/a.rs")]);
    }

    #[test]
    fn shared_state_policy_uses_a_product_namespace() {
        let resources = crate::tool::ResourcePolicy::SharedState { scope: "task" }.resolve(
            &serde_json::json!({"task_id": 1}),
            std::path::Path::new("/work"),
        );
        assert_eq!(resources.writes, vec![PathBuf::from("__tact_task__")]);
        assert!(!resources.barrier);
    }
}
