use tact_desktop_core::{ExtensionsTab, ExtensionsView};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionDialogState {
    pub tab: ExtensionsTab,
    pub query: String,
}

pub fn manage(view: &ExtensionsView) -> ExtensionDialogState {
    ExtensionDialogState {
        tab: view.tab,
        query: view.query.clone(),
    }
}

pub fn matches_extension(query: &str, value: &str) -> bool {
    query.trim().is_empty() || tact_catalog::fuzzy_score(query, value).is_some()
}

#[cfg(test)]
mod tests {
    use gpui::TestAppContext;

    use super::*;

    #[gpui::test]
    fn extensions_manager_carries_query_into_full_dialog(_cx: &mut TestAppContext) {
        let view = ExtensionsView {
            tab: ExtensionsTab::Plugins,
            query: "git".into(),
            ..ExtensionsView::default()
        };
        assert_eq!(
            manage(&view),
            ExtensionDialogState {
                tab: ExtensionsTab::Plugins,
                query: "git".into()
            }
        );
    }

    #[test]
    fn extension_search_uses_the_shared_fuzzy_matcher() {
        assert!(matches_extension("gthb", "GitHub Marketplace"));
        assert!(!matches_extension("zzzz", "GitHub Marketplace"));
    }
}
