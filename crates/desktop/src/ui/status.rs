//! Localized labels for protocol status values.
//!
//! The shell renders several status domains whose wire types live in different
//! modules. Keeping the presentation mapping here prevents each surface from
//! growing its own English and Chinese match tables.

use tact_catalog::CatalogLocale;
use tact_desktop_core::{
    BackgroundTaskStatusSnapshot, GoalPhaseSnapshot, GoalStatusSnapshot, McpServerStatusSnapshot,
    RunPhase, SubagentStatusSnapshot,
};

use crate::i18n::{self, TextKey};

pub fn run_phase_label(phase: RunPhase, locale: CatalogLocale) -> &'static str {
    use RunPhase as P;
    match phase {
        P::Idle => i18n::text(locale, TextKey::Ready),
        P::Waiting => i18n::text(locale, TextKey::Queue),
        P::Thinking => i18n::text(locale, TextKey::Thinking),
        P::Streaming => i18n::text(locale, TextKey::Running),
        P::RunningTools => i18n::text(locale, TextKey::Tool),
        P::WaitingForGate => i18n::text(locale, TextKey::PermissionOrQuestionGate),
        P::Cancelling => i18n::text(locale, TextKey::Stopping),
        P::Complete => i18n::text(locale, TextKey::Completed),
        P::Failed => i18n::text(locale, TextKey::Failed),
    }
}

pub fn goal_status_label(status: GoalStatusSnapshot, locale: CatalogLocale) -> &'static str {
    use GoalStatusSnapshot as S;
    match status {
        S::Draft | S::Queued => i18n::text(locale, TextKey::Queue),
        S::Running => i18n::text(locale, TextKey::Running),
        S::Paused => i18n::text(locale, TextKey::Pause),
        S::WaitingUser | S::WaitingPermission => {
            i18n::text(locale, TextKey::PermissionOrQuestionGate)
        }
        S::Verifying => i18n::text(locale, TextKey::Review),
        S::Succeeded => i18n::text(locale, TextKey::Completed),
        S::Failed => i18n::text(locale, TextKey::Failed),
        S::Cancelled => i18n::text(locale, TextKey::Cancelled),
    }
}

pub fn goal_phase_label(phase: GoalPhaseSnapshot, locale: CatalogLocale) -> &'static str {
    match phase {
        GoalPhaseSnapshot::Planning => i18n::text(locale, TextKey::Thinking),
        GoalPhaseSnapshot::Execution => i18n::text(locale, TextKey::Running),
        GoalPhaseSnapshot::Verification => i18n::text(locale, TextKey::Review),
    }
}

pub fn subagent_status_label(
    status: SubagentStatusSnapshot,
    locale: CatalogLocale,
) -> &'static str {
    match status {
        SubagentStatusSnapshot::Running => i18n::text(locale, TextKey::Running),
        SubagentStatusSnapshot::Completed => i18n::text(locale, TextKey::Completed),
        SubagentStatusSnapshot::Failed => i18n::text(locale, TextKey::Failed),
        SubagentStatusSnapshot::Cancelled => i18n::text(locale, TextKey::Cancelled),
    }
}

pub fn background_status_label(
    status: BackgroundTaskStatusSnapshot,
    locale: CatalogLocale,
) -> &'static str {
    match status {
        BackgroundTaskStatusSnapshot::Running => i18n::text(locale, TextKey::Running),
        BackgroundTaskStatusSnapshot::Completed => i18n::text(locale, TextKey::Completed),
        BackgroundTaskStatusSnapshot::Error => i18n::text(locale, TextKey::Failed),
    }
}

pub fn mcp_status_label(status: &McpServerStatusSnapshot, locale: CatalogLocale) -> String {
    match status {
        McpServerStatusSnapshot::Connected { tools } => {
            format!("{} ({tools})", i18n::text(locale, TextKey::Ready))
        }
        McpServerStatusSnapshot::NeedsAuthorization => {
            i18n::text(locale, TextKey::Authorize).to_string()
        }
        McpServerStatusSnapshot::NotConnected => {
            i18n::text(locale, TextKey::Disconnected).to_string()
        }
        McpServerStatusSnapshot::Disabled => i18n::text(locale, TextKey::Off).to_string(),
    }
}

pub fn localized_wait_reason(reason: &str, locale: CatalogLocale) -> String {
    match (reason, locale) {
        ("paused by user", CatalogLocale::Zh) => "用户已暂停".into(),
        ("waiting for user", CatalogLocale::Zh) => "等待用户".into(),
        ("waiting for permission", CatalogLocale::Zh) => "等待权限确认".into(),
        ("run interrupted: lease holder is gone", CatalogLocale::Zh) => {
            "运行中断：租约持有者已消失".into()
        }
        ("lease holder is gone", CatalogLocale::Zh) => "租约持有者已消失".into(),
        ("needs acceptance", CatalogLocale::Zh) => "待验收".into(),
        ("needs explicit acceptance", CatalogLocale::Zh) => "待明确验收".into(),
        ("review", CatalogLocale::Zh) => "待复核".into(),
        _ => reason.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_domains_share_the_ui_locale() {
        assert_eq!(
            background_status_label(BackgroundTaskStatusSnapshot::Running, CatalogLocale::Zh),
            "运行中"
        );
        assert_eq!(
            subagent_status_label(SubagentStatusSnapshot::Cancelled, CatalogLocale::Zh),
            "已取消"
        );
        assert_eq!(
            goal_status_label(GoalStatusSnapshot::Verifying, CatalogLocale::Zh),
            "查看"
        );
        assert_eq!(
            localized_wait_reason("waiting for permission", CatalogLocale::Zh),
            "等待权限确认"
        );
    }
}
