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

/// Label for the run's current phase.
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

/// Label for a Goal's lifecycle status.
pub fn goal_status_label(status: GoalStatusSnapshot, locale: CatalogLocale) -> &'static str {
    use GoalStatusSnapshot as S;
    match status {
        S::Draft | S::Queued => i18n::text(locale, TextKey::Queue),
        S::Running => i18n::text(locale, TextKey::Running),
        S::Paused => i18n::text(locale, TextKey::Pause),
        S::WaitingUser | S::WaitingPermission => {
            i18n::text(locale, TextKey::PermissionOrQuestionGate)
        }
        S::Verifying => i18n::text(locale, TextKey::Verifying),
        S::Succeeded => i18n::text(locale, TextKey::Completed),
        S::Failed => i18n::text(locale, TextKey::Failed),
        S::Cancelled => i18n::text(locale, TextKey::Cancelled),
    }
}

/// Label for the Goal's broad execution phase.
pub fn goal_phase_label(phase: GoalPhaseSnapshot, locale: CatalogLocale) -> &'static str {
    match phase {
        GoalPhaseSnapshot::Planning => i18n::text(locale, TextKey::Thinking),
        GoalPhaseSnapshot::Execution => i18n::text(locale, TextKey::Running),
        GoalPhaseSnapshot::Verification => i18n::text(locale, TextKey::Verifying),
    }
}

/// Label for a subagent's lifecycle status.
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

/// Label for a background task's lifecycle status.
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

/// Label for an MCP server's connection state.
///
/// Owned rather than `&'static str` because a connected server appends its
/// tool count to a catalog string.
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

/// Translate a Goal's `wait_reason` payload, leaving free text untouched.
///
/// The payload is a stored goal event, not an enum: `tact_host`'s runner writes
/// `format!("{from:?}: {reason}")` (`goal_runner.rs::status_event`) while the
/// driver's pause path writes the bare reason. Both shapes are handled so a
/// status prefix does not silently drop the translation and leave English in a
/// Chinese UI. Unknown reasons — including payloads persisted by older
/// versions — pass through verbatim.
pub fn localized_wait_reason(reason: &str, locale: CatalogLocale) -> String {
    if locale != CatalogLocale::Zh {
        return reason.to_string();
    }
    // A prefix is only worth dropping when what follows it translates;
    // otherwise the payload's own words (prefix included) are all the reader
    // has, and returning the bare remainder would hide which status it came
    // from.
    for key in [reason, strip_status_prefix(reason)] {
        if let Some(localized) = localize_known_reason(key) {
            return localized.to_string();
        }
    }
    reason.to_string()
}

/// The Chinese label for a known wait reason, if this build knows it.
fn localize_known_reason(reason: &str) -> Option<&'static str> {
    match reason {
        "paused by user" => Some("用户已暂停"),
        "waiting for user" => Some("等待用户"),
        "waiting for permission" => Some("等待权限确认"),
        "run interrupted: lease holder is gone" => Some("运行中断：租约持有者已消失"),
        "lease holder is gone" => Some("租约持有者已消失"),
        "needs acceptance" => Some("待验收"),
        "needs explicit acceptance" => Some("待明确验收"),
        "review" => Some("待复核"),
        _ => None,
    }
}

/// Drop the `"<From status>: "` prefix the Goal runner puts on a status event.
///
/// Mirrors the `Debug` spelling of `tact::goal::GoalStatus`, which is what the
/// runner formats; matching it exactly keeps a reason that merely contains a
/// colon (for example `"run interrupted: lease holder is gone"`) intact.
fn strip_status_prefix(reason: &str) -> &str {
    const STATUSES: [&str; 10] = [
        "Draft",
        "Queued",
        "Running",
        "WaitingUser",
        "WaitingPermission",
        "Paused",
        "Verifying",
        "Succeeded",
        "Failed",
        "Cancelled",
    ];
    for status in STATUSES {
        if let Some(reason) = reason
            .strip_prefix(status)
            .and_then(|rest| rest.strip_prefix(": "))
        {
            return reason;
        }
    }
    reason
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
            "验收中"
        );
        assert_eq!(
            goal_phase_label(GoalPhaseSnapshot::Verification, CatalogLocale::Zh),
            "验收中",
            "the phase and the status name the same step"
        );
        assert_eq!(
            localized_wait_reason("waiting for permission", CatalogLocale::Zh),
            "等待权限确认"
        );
    }

    /// The runner stores `"{from:?}: {reason}"`; a status prefix must not cost
    /// the translation.
    #[test]
    fn a_runner_status_prefix_still_localizes() {
        for (payload, expected) in [
            (
                "Running: run interrupted: lease holder is gone",
                "运行中断：租约持有者已消失",
            ),
            ("Paused: paused by user", "用户已暂停"),
            ("WaitingUser: needs acceptance", "待验收"),
            ("Queued: review", "待复核"),
        ] {
            assert_eq!(
                localized_wait_reason(payload, CatalogLocale::Zh),
                expected,
                "payload shape from goal_runner::status_event: {payload}"
            );
        }
    }

    #[test]
    fn free_text_wait_reasons_pass_through_verbatim() {
        for reason in [
            "budget exhausted",
            "activation could not start; restored paused",
            "Running: budget exhausted",
        ] {
            assert_eq!(
                localized_wait_reason(reason, CatalogLocale::Zh),
                reason,
                "an unknown reason must keep its own words, prefix included"
            );
            assert_eq!(
                localized_wait_reason(reason, CatalogLocale::En),
                reason,
                "English is the payload's own language"
            );
        }
    }
}
