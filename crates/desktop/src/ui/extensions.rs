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

pub fn score_extension(query: &str, values: &[&str]) -> i32 {
    if query.trim().is_empty() {
        return 0;
    }
    values
        .iter()
        .filter_map(|value| tact_catalog::fuzzy_score(query, value))
        .max()
        .unwrap_or(-1)
}

pub fn matches_extension(query: &str, value: &str) -> bool {
    score_extension(query, &[value]) >= 0
}

/// Whether a skill row belongs in the filtered list.
///
/// The popover renders this row and `Enter` inserts its first match, so both
/// must ask the same question: a skill the list hides is a skill `Enter` must
/// not insert. The description is part of the answer because it is visible on
/// the row.
pub fn matches_skill(query: &str, name: &str, description: &str) -> bool {
    score_extension(query, &[name, description]) >= 0
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

    #[test]
    fn extension_scores_rank_prefixes_above_subsequences() {
        let prefix = score_extension("git", &["GitHub Marketplace", "git"]);
        let subsequence = score_extension("gthb", &["GitHub Marketplace"]);
        assert!(prefix > subsequence);
    }
}
