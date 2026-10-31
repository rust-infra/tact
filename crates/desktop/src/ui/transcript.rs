use tact_desktop_core::{DesktopSnapshot, TranscriptRow};

use super::shell::TranscriptRowKind;

pub fn visible_row_kind(kind: TranscriptRowKind, show_hook_output: bool) -> bool {
    show_hook_output || kind != TranscriptRowKind::HookStatus
}

pub fn row_kind(row: &TranscriptRow) -> TranscriptRowKind {
    match &row.body {
        tact_desktop_core::RowBody::User(_) => TranscriptRowKind::User,
        tact_desktop_core::RowBody::Assistant(_) => TranscriptRowKind::Assistant,
        tact_desktop_core::RowBody::Thinking(_) => TranscriptRowKind::Thinking,
        tact_desktop_core::RowBody::ToolGroup(_) => TranscriptRowKind::ToolGroup,
        tact_desktop_core::RowBody::Diff(_) => TranscriptRowKind::Diff,
        tact_desktop_core::RowBody::Gate(_) => TranscriptRowKind::Gate,
        tact_desktop_core::RowBody::Notice(_) => TranscriptRowKind::Notice,
        tact_desktop_core::RowBody::HookContext(_) => TranscriptRowKind::HookContext,
        tact_desktop_core::RowBody::HookStatus(_) => TranscriptRowKind::HookStatus,
        tact_desktop_core::RowBody::Error(_) => TranscriptRowKind::Error,
    }
}

pub fn visible_row(row: &TranscriptRow, snapshot: &DesktopSnapshot) -> bool {
    visible_row_kind(row_kind(row), snapshot.show_hook_output)
}

#[cfg(test)]
mod tests {
    use gpui::TestAppContext;

    use super::*;

    #[gpui::test]
    fn hook_output_toggle_hides_hook_status_rows(_cx: &mut TestAppContext) {
        assert!(visible_row_kind(TranscriptRowKind::HookStatus, true));
        assert!(!visible_row_kind(TranscriptRowKind::HookStatus, false));
        assert!(visible_row_kind(TranscriptRowKind::Assistant, false));
    }
}
