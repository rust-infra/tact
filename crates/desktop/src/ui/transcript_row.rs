use gpui::Role;
use tact_desktop_core::{CatalogLocale, RowBody, TranscriptRow};

use crate::i18n::{self, TextKey};
use crate::ui::shell::TranscriptRowKind;

pub fn accessibility_role(row: &TranscriptRow) -> Role {
    accessibility_role_for_kind(kind(row))
}

pub fn accessibility_label(row: &TranscriptRow, locale: CatalogLocale) -> &'static str {
    accessibility_label_for_kind(kind(row), locale)
}

fn kind(row: &TranscriptRow) -> TranscriptRowKind {
    match row.body {
        RowBody::User(_) => TranscriptRowKind::User,
        RowBody::Assistant(_) => TranscriptRowKind::Assistant,
        RowBody::Thinking(_) => TranscriptRowKind::Thinking,
        RowBody::ToolGroup(_) => TranscriptRowKind::ToolGroup,
        RowBody::Diff(_) => TranscriptRowKind::Diff,
        RowBody::Gate(_) => TranscriptRowKind::Gate,
        RowBody::Notice(_) => TranscriptRowKind::Notice,
        RowBody::HookContext(_) => TranscriptRowKind::HookContext,
        RowBody::HookStatus(_) => TranscriptRowKind::HookStatus,
        RowBody::Error(_) => TranscriptRowKind::Error,
    }
}

pub const fn accessibility_role_for_kind(kind: TranscriptRowKind) -> Role {
    match kind {
        TranscriptRowKind::User | TranscriptRowKind::Assistant => Role::Article,
        TranscriptRowKind::Gate | TranscriptRowKind::Error => Role::Alert,
        TranscriptRowKind::Notice | TranscriptRowKind::HookStatus => Role::Status,
        _ => Role::Group,
    }
}

pub fn accessibility_label_for_kind(
    kind: TranscriptRowKind,
    locale: CatalogLocale,
) -> &'static str {
    match kind {
        TranscriptRowKind::User => i18n::text(locale, TextKey::UserMessage),
        TranscriptRowKind::Assistant => i18n::text(locale, TextKey::AssistantResponse),
        TranscriptRowKind::Thinking => i18n::text(locale, TextKey::Thinking),
        TranscriptRowKind::ToolGroup => i18n::text(locale, TextKey::ToolGroup),
        TranscriptRowKind::Diff => i18n::text(locale, TextKey::Diff),
        TranscriptRowKind::Gate => i18n::text(locale, TextKey::PermissionOrQuestionGate),
        TranscriptRowKind::Notice => i18n::text(locale, TextKey::Notice),
        TranscriptRowKind::HookContext => i18n::text(locale, TextKey::HookContext),
        TranscriptRowKind::HookStatus => i18n::text(locale, TextKey::HookStatus),
        TranscriptRowKind::Error => i18n::text(locale, TextKey::Error),
    }
}

pub fn evidence_language(language: &str) -> &str {
    match language.to_ascii_lowercase().as_str() {
        "mermaid" | "ansi" => "text",
        _ => language,
    }
}

#[cfg(test)]
mod tests {
    use gpui::{
        InteractiveElement as _, IntoElement as _, Role, StatefulInteractiveElement as _,
        TestAppContext, div,
    };

    use super::*;
    use crate::ui::shell::TRANSCRIPT_ROW_KINDS;

    #[gpui::test]
    fn mermaid_and_ansi_use_readable_code_fallbacks(_cx: &mut TestAppContext) {
        assert_eq!(evidence_language("mermaid"), "text");
        assert_eq!(evidence_language("ansi"), "text");
        assert_eq!(evidence_language("rust"), "rust");
    }

    #[test]
    fn every_transcript_kind_writes_accesskit_role_and_localized_label() {
        for kind in TRANSCRIPT_ROW_KINDS {
            let role = accessibility_role_for_kind(kind);
            let label = accessibility_label_for_kind(kind, CatalogLocale::Zh);
            let element = div()
                .id("a11y-row")
                .role(role)
                .aria_label(label)
                .into_element();
            let mut node = gpui::accesskit::Node::new(Role::Unknown);
            gpui::Element::write_a11y_info(&element, &mut node);
            assert_eq!(gpui::Element::a11y_role(&element), Some(role), "{kind:?}");
            assert_eq!(node.label(), Some(label), "{kind:?}");
        }
    }
}
