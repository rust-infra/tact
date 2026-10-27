//! Tool presentation policy kept outside the provider-neutral kernel API.

use crate::tool::{
    ArgumentSummaryPolicy, DetailPolicy, PopupPolicy, TaskOperation, ToolDomain, ToolMetadata,
    ToolPresentation,
};
use serde_json::{Value, json};
use tact_protocol::{StepStatus, ToolDetailKind, ToolPopupKind, ToolPresentationInfo};

pub(crate) const MAX_TOOL_ARG_SUMMARY_CHARS: usize = 120;
const TASK_SUMMARY_CHARS: usize = 240;

fn str_field<'a>(input: &'a serde_json::Value, key: &str) -> &'a str {
    input
        .get(key)
        .and_then(|value| value.as_str())
        .unwrap_or("")
}

fn truncate(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    format!(
        "{}...",
        value
            .chars()
            .take(max_chars.saturating_sub(3))
            .collect::<String>()
    )
}

fn tool_arg_full(policy: ArgumentSummaryPolicy, input: &serde_json::Value) -> String {
    fn patch_title(input: &serde_json::Value) -> String {
        let patch = str_field(input, "patch");
        let dry = input
            .get("dry_run")
            .and_then(|value| value.as_bool())
            .unwrap_or(false);
        let label = if dry { "dry-run" } else { "patch" };
        let first_line = patch.lines().next().unwrap_or("").trim();
        if first_line.is_empty() {
            label.to_string()
        } else if first_line.len() <= 78 {
            format!("{label}: {first_line}")
        } else {
            format!("{label}: {}...", &first_line[..75])
        }
    }

    match policy {
        ArgumentSummaryPolicy::Json => input.to_string(),
        ArgumentSummaryPolicy::Path { field }
        | ArgumentSummaryPolicy::Command { field }
        | ArgumentSummaryPolicy::Question { field }
        | ArgumentSummaryPolicy::SubagentPrompt { field }
        | ArgumentSummaryPolicy::Id { field }
        | ArgumentSummaryPolicy::ReadOffsetLimit { path_field: field } => {
            str_field(input, field).to_string()
        }
        ArgumentSummaryPolicy::PatchPreview { .. } => patch_title(input),
    }
}

fn task_argument_pair(
    op: TaskOperation,
    input: &serde_json::Value,
    before: Option<&crate::task::TaskRecord>,
    after: Option<&crate::task::TaskRecord>,
) -> (String, String) {
    let full = crate::task::format_task_tool_title(op, input, before, after);
    let summary = truncate(&full, TASK_SUMMARY_CHARS);
    (full, summary)
}

/// Returns the untruncated and summary forms used by the host protocol.
pub(crate) fn argument_pair(
    metadata: &ToolMetadata,
    input: &serde_json::Value,
    before: Option<&crate::task::TaskRecord>,
    after: Option<&crate::task::TaskRecord>,
) -> (String, String) {
    if let ToolDomain::Task(op) = metadata.domain {
        return task_argument_pair(op, input, before, after);
    }
    let full = tool_arg_full(metadata.argument_summary, input);
    let summary = truncate(&full, MAX_TOOL_ARG_SUMMARY_CHARS);
    (full, summary)
}

/// Builds presentation metadata for a native tool.
pub(crate) fn presentation(metadata: &ToolMetadata) -> ToolPresentationInfo {
    let ToolPresentation {
        visual_kind,
        display_name,
        live_output,
        detail,
        popup,
        compact_result_to_meta,
    } = metadata.presentation;
    ToolPresentationInfo {
        visual_kind,
        display_name: display_name.to_string(),
        keep_full_live_output: matches!(live_output, crate::tool::LiveOutputPolicy::FullTranscript),
        keep_live: matches!(live_output, crate::tool::LiveOutputPolicy::Background),
        detail: match detail {
            DetailPolicy::None => ToolDetailKind::None,
            DetailPolicy::Result => ToolDetailKind::Result,
            DetailPolicy::InputField(field) => ToolDetailKind::InputField(field.to_string()),
        },
        popup: match popup {
            PopupPolicy::None => ToolPopupKind::None,
            PopupPolicy::SubagentTranscript => ToolPopupKind::SubagentTranscript,
        },
        compact_result_to_meta,
    }
}

/// Presentation for one invocation, including live background cards.
pub(crate) fn presentation_for(
    metadata: &ToolMetadata,
    input: &serde_json::Value,
) -> ToolPresentationInfo {
    let mut presentation = presentation(metadata);
    if metadata.name == "spawn_subagent"
        && input
            .get("run_in_background")
            .and_then(|value| value.as_bool())
            == Some(true)
    {
        presentation.keep_live = true;
    }
    presentation
}

/// Serialize host presentation policy into provider-neutral tool metadata.
///
/// The kernel only transports this object. Explicit tags keep field-bearing
/// variants such as `InputField` lossless without adding a product protocol
/// dependency to `tact_agent_api`.
pub(crate) fn presentation_metadata(presentation: &ToolPresentationInfo) -> Value {
    json!({
        "visual_kind": visual_kind_tag(presentation.visual_kind),
        "display_name": presentation.display_name,
        "keep_full_live_output": presentation.keep_full_live_output,
        "detail": detail_metadata(&presentation.detail),
        "popup": popup_tag(presentation.popup),
        "compact_result_to_meta": presentation.compact_result_to_meta,
        "keep_live": presentation.keep_live,
    })
}

/// Restore presentation policy at the Tact event boundary.
pub(crate) fn presentation_from_metadata(
    default_name: &str,
    value: Option<&Value>,
) -> ToolPresentationInfo {
    let mut presentation = ToolPresentationInfo::generic(default_name);
    let Some(value) = value else {
        return presentation;
    };

    presentation.visual_kind = match value
        .get("visual_kind")
        .and_then(Value::as_str)
        .unwrap_or("generic")
    {
        "file_write" => tact_protocol::ToolVisualKind::FileWrite,
        "file_read" => tact_protocol::ToolVisualKind::FileRead,
        "file_edit" => tact_protocol::ToolVisualKind::FileEdit,
        "command" => tact_protocol::ToolVisualKind::Command,
        "task" => tact_protocol::ToolVisualKind::Task,
        "subagent" => tact_protocol::ToolVisualKind::Subagent,
        "sleep" => tact_protocol::ToolVisualKind::Sleep,
        _ => tact_protocol::ToolVisualKind::Generic,
    };
    presentation.display_name = value
        .get("display_name")
        .and_then(Value::as_str)
        .unwrap_or(default_name)
        .to_string();
    presentation.keep_full_live_output = value
        .get("keep_full_live_output")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    presentation.detail = detail_from_metadata(value.get("detail"));
    presentation.popup = match value.get("popup").and_then(Value::as_str) {
        Some("subagent_transcript") => ToolPopupKind::SubagentTranscript,
        _ => ToolPopupKind::None,
    };
    presentation.compact_result_to_meta = value
        .get("compact_result_to_meta")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    presentation.keep_live = value
        .get("keep_live")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    presentation
}

fn visual_kind_tag(kind: tact_protocol::ToolVisualKind) -> &'static str {
    match kind {
        tact_protocol::ToolVisualKind::Generic => "generic",
        tact_protocol::ToolVisualKind::FileWrite => "file_write",
        tact_protocol::ToolVisualKind::FileRead => "file_read",
        tact_protocol::ToolVisualKind::FileEdit => "file_edit",
        tact_protocol::ToolVisualKind::Command => "command",
        tact_protocol::ToolVisualKind::Task => "task",
        tact_protocol::ToolVisualKind::Subagent => "subagent",
        tact_protocol::ToolVisualKind::Sleep => "sleep",
    }
}

fn detail_metadata(detail: &ToolDetailKind) -> Value {
    match detail {
        ToolDetailKind::None => json!({"kind": "none"}),
        ToolDetailKind::Result => json!({"kind": "result"}),
        ToolDetailKind::InputField(field) => {
            json!({"kind": "input_field", "field": field})
        }
    }
}

fn detail_from_metadata(value: Option<&Value>) -> ToolDetailKind {
    match value
        .and_then(|value| value.get("kind"))
        .and_then(Value::as_str)
    {
        Some("none") => ToolDetailKind::None,
        Some("input_field") => ToolDetailKind::InputField(
            value
                .and_then(|value| value.get("field"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        ),
        _ => ToolDetailKind::Result,
    }
}

fn popup_tag(popup: ToolPopupKind) -> &'static str {
    match popup {
        ToolPopupKind::None => "none",
        ToolPopupKind::SubagentTranscript => "subagent_transcript",
    }
}

/// Detail text for a host-facing presentation policy.
pub(crate) fn step_detail_for_presentation(
    detail: &ToolDetailKind,
    input: &serde_json::Value,
    output: &str,
    status: StepStatus,
) -> Option<String> {
    if matches!(status, StepStatus::Failed) {
        return Some(output.to_string());
    }
    match detail {
        ToolDetailKind::None => None,
        ToolDetailKind::Result => Some(output.to_string()),
        ToolDetailKind::InputField(field) => Some(str_field(input, field).to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn background_subagent_cards_stay_live() {
        let metadata = &crate::tool::SPAWN_SUBAGENT_METADATA;
        let foreground = presentation_for(metadata, &serde_json::json!({}));
        let background =
            presentation_for(metadata, &serde_json::json!({"run_in_background": true}));
        assert!(!foreground.keep_live);
        assert!(background.keep_live);
    }

    #[test]
    fn patch_preview_is_concise_and_stable() {
        let router = crate::tool::toolset();
        let base = *router.resolve("edit_file").expect("edit_file").metadata();
        let metadata = ToolMetadata {
            name: "apply_patch",
            argument_summary: ArgumentSummaryPolicy::PatchPreview {
                patch_field: "patch",
            },
            ..base
        };
        let (full, summary) = argument_pair(
            &metadata,
            &serde_json::json!({"patch": "*** Begin Patch\n*** End Patch"}),
            None,
            None,
        );
        assert_eq!(full, "patch: *** Begin Patch");
        assert_eq!(summary, full);
    }

    #[test]
    fn presentation_metadata_round_trips_input_field_detail() {
        let mut source = ToolPresentationInfo::generic("read_file");
        source.visual_kind = tact_protocol::ToolVisualKind::FileRead;
        source.detail = ToolDetailKind::InputField("path".into());
        source.keep_full_live_output = true;

        let metadata = presentation_metadata(&source);
        let restored = presentation_from_metadata("fallback", Some(&metadata));

        assert_eq!(restored, source);
    }
}
