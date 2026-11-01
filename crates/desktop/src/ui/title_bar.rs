use tact_desktop_core::{CatalogLocale, SessionPhase, SessionView};

use crate::i18n::{self, TextKey};

pub fn status_text(session: &SessionView, locale: CatalogLocale) -> &'static str {
    match session.phase {
        SessionPhase::Empty => i18n::text(locale, TextKey::NoSession),
        SessionPhase::Opening => i18n::text(locale, TextKey::Opening),
        SessionPhase::Starting => i18n::text(locale, TextKey::Starting),
        SessionPhase::Ready => i18n::text(locale, TextKey::Ready),
        SessionPhase::Running => i18n::text(locale, TextKey::Running),
        SessionPhase::Switching => i18n::text(locale, TextKey::Switching),
        SessionPhase::Closing => i18n::text(locale, TextKey::Stopping),
        SessionPhase::Disconnected { .. } => i18n::text(locale, TextKey::Disconnected),
        SessionPhase::Failed { .. } => i18n::text(locale, TextKey::Failed),
    }
}

pub fn title(display_name: &str, branch: Option<&str>) -> String {
    match branch.filter(|branch| !branch.is_empty()) {
        Some(branch) => format!("{display_name} · {branch}"),
        None => display_name.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phase_labels_follow_the_catalog_locale() {
        let session = SessionView {
            phase: SessionPhase::Ready,
            ..SessionView::default()
        };
        assert_eq!(status_text(&session, CatalogLocale::En), "Ready");
        assert_eq!(status_text(&session, CatalogLocale::Zh), "就绪");
    }
}
