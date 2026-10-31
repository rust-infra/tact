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

/// P2 components that deliberately do not have a navigation entry yet.
pub const DEFERRED_P2_COMPONENTS: &[&str] =
    &["SearchResults", "ArtifactPanel", "DiffTable", "ContextCard"];

#[cfg(test)]
mod tests {
    use gpui::TestAppContext;

    use super::*;

    #[gpui::test]
    fn p2_components_are_explicitly_deferred(_cx: &mut TestAppContext) {
        assert_eq!(DEFERRED_P2_COMPONENTS.len(), 4);
    }
}
