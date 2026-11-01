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

#[cfg(test)]
mod tests {
    use gpui::TestAppContext;

    use super::*;

    #[gpui::test]
    fn p2_components_are_mounted_by_the_shell(_cx: &mut TestAppContext) {
        let root = include_str!("../root.rs");
        for (component, module) in MOUNTED_P2_COMPONENTS {
            let source = match *module {
                "root.rs" => root,
                _ => unreachable!("unknown P2 module {module}"),
            };
            assert!(
                source.contains(component),
                "P2 component {component} must be mounted in {module}"
            );
        }
        // The adapters without a vendor component have their own modules.
        let adapters = [
            include_str!("ansi.rs"),
            include_str!("mermaid.rs"),
            include_str!("web_search.rs"),
            include_str!("diff_review.rs"),
        ]
        .join("\n");
        for adapter in ["parse", "element", "files"] {
            assert!(
                adapters.contains(&format!("pub fn {adapter}")),
                "missing P2 adapter entry point {adapter}"
            );
        }
    }
}
