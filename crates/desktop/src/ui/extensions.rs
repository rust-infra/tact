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

#[cfg(test)]
mod tests {

    use super::*;

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
