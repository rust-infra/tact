#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlayKind {
    CommandCenter,
    Settings,
    Form,
    Reader,
    Extensions,
    SessionManager,
    Subagent,
    TaskDag,
    Balance,
    SystemPrompt,
    SubagentModel,
}

impl OverlayKind {
    pub const fn dismissible_with_escape(self) -> bool {
        true
    }
}

/// The P2 surfaces the shell mounts, and the module that owns each.
pub const MOUNTED_P2_COMPONENTS: &[(&str, &str)] = &[
    ("SearchResults", "root.rs"),
    ("ArtifactPanel", "root.rs"),
    ("DiffTable", "root.rs"),
    ("ContextCard", "root.rs"),
];


