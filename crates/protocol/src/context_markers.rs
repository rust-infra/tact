//! The in-band context markers.
//!
//! Tact's message model has no `developer` role, so injected context reaches the
//! model as a *user* message wrapped in a marker that names its author — the
//! convention `<subagent-finished>` and `<context-handoff>` already use.
//!
//! Two readers depend on these markers, and both need the same answer:
//!
//! - the runtime, which must not count an injected block as a user turn;
//! - a view, which must render it as a labelled block rather than as something
//!   the user typed.
//!
//! Detection is by marker text rather than by the in-memory `MessageKind`
//! because a session reloaded from disk carries no kind: the marker is the only
//! signal that survives persistence.
//!
//! The markers live here rather than beside the producer because a view has to
//! recognize them without depending on the extension crate that writes them.

/// Opening tag of a hook's injected context.
pub const HOOK_CONTEXT_OPEN_TAG: &str = "<hook-context>";
/// Closing tag of a hook's injected context.
pub const HOOK_CONTEXT_CLOSE_TAG: &str = "</hook-context>";

/// Marker prefix every hook-context message starts with.
///
/// [`HOOK_CONTEXT_OPEN_TAG`] is the attribute-free spelling; a frame may carry
/// the source instead (`<hook-context source="plugin codex">`), so recognition
/// compares against the prefix rather than the whole tag.
const HOOK_CONTEXT_MARKER: &str = "<hook-context";

/// Opening tag of the Goal runner's per-activation context.
pub const GOAL_CONTEXT_OPEN_TAG: &str = "<goal-context>";
/// Closing tag of the Goal runner's per-activation context.
pub const GOAL_CONTEXT_CLOSE_TAG: &str = "</goal-context>";

/// Frames one hook-injected chunk as the user message the model reads.
///
/// The source rides on the tag rather than in the body: a reloaded session can
/// only recover what the text itself carries, and the reader is shown the same
/// attribution the model is. A label that cannot be spelled as a bare attribute
/// value is dropped rather than escaped — the body is what matters, and a
/// malformed frame would defeat the marker that identifies it.
#[must_use]
pub fn frame_hook_context(source: Option<&str>, body: &str) -> String {
    let attr = source
        .filter(|label| !label.is_empty() && !label.contains(['"', '\\', '\n']))
        .map(|label| format!(" source=\"{label}\""))
        .unwrap_or_default();
    format!("{HOOK_CONTEXT_MARKER}{attr}>\n{body}\n{HOOK_CONTEXT_CLOSE_TAG}")
}

/// Whether `text` is a hook-injected context cell.
///
/// Leading whitespace is tolerated: a reload may have trimmed or re-indented the
/// stored cell.
#[must_use]
pub fn is_hook_context_text(text: &str) -> bool {
    let trimmed = text.trim_start();
    trimmed.starts_with(HOOK_CONTEXT_OPEN_TAG)
        || trimmed
            .strip_prefix(HOOK_CONTEXT_MARKER)
            .is_some_and(|rest| rest.starts_with(' '))
}

/// The source recorded on a hook-context cell, when it carried one.
#[must_use]
pub fn hook_context_source(text: &str) -> Option<&str> {
    let rest = text.trim().strip_prefix(HOOK_CONTEXT_MARKER)?;
    let attrs = &rest[..rest.find('>')?];
    let value = attrs.trim().strip_prefix("source=")?.trim_matches('"');
    (!value.is_empty()).then_some(value)
}

/// The context carried by a `<hook-context>` cell, or `text` unchanged when it
/// is not one — the inverse of [`frame_hook_context`].
#[must_use]
pub fn hook_context_body(text: &str) -> &str {
    let trimmed = text.trim();
    let Some(rest) = trimmed.strip_prefix(HOOK_CONTEXT_MARKER) else {
        return text;
    };
    let Some(open_end) = rest.find('>') else {
        return text;
    };
    rest[open_end + 1..]
        .strip_suffix(HOOK_CONTEXT_CLOSE_TAG)
        .map(str::trim)
        .unwrap_or(text)
}

/// Whether `text` is the runner's injected Goal context rather than a user turn.
///
/// Detection is by marker, not by kind, for the same reason as
/// [`is_hook_context_text`]: a session reloaded from disk carries no kind.
#[must_use]
pub fn is_goal_context_text(text: &str) -> bool {
    text.trim_start().starts_with(GOAL_CONTEXT_OPEN_TAG)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A framed block is recognized and unwrapped back to its body.
    #[test]
    fn a_frame_round_trips_through_its_marker() {
        let framed = frame_hook_context(Some("plugin codex"), "hello");
        assert!(is_hook_context_text(&framed));
        assert_eq!(hook_context_source(&framed), Some("plugin codex"));
        assert_eq!(hook_context_body(&framed), "hello");
    }

    /// A frame without a source still carries the marker.
    ///
    /// `None` is a chunk that reached the queue without one; the marker is what
    /// identifies the cell, so it cannot depend on the attribute.
    #[test]
    fn a_source_less_frame_is_still_recognized() {
        let framed = frame_hook_context(None, "body");
        assert!(is_hook_context_text(&framed));
        assert_eq!(hook_context_source(&framed), None);
        assert_eq!(hook_context_body(&framed), "body");
    }

    /// A label that cannot be a bare attribute value is dropped, not escaped.
    ///
    /// Escaping would still produce a well-formed frame, but it hides a bad
    /// label behind an encoding the reader would see verbatim. Dropping it keeps
    /// the body — the part that matters — and the marker intact.
    #[test]
    fn an_unspellable_source_is_dropped() {
        for label in ["with\"quote", "with\\backslash", "with\nnewline", ""] {
            let framed = frame_hook_context(Some(label), "body");
            assert!(
                is_hook_context_text(&framed),
                "{label:?} must still produce a recognizable frame"
            );
            assert_eq!(hook_context_source(&framed), None, "{label:?}");
            assert_eq!(hook_context_body(&framed), "body", "{label:?}");
        }
    }

    /// A user turn is not mistaken for injected context.
    ///
    /// Both recognizers are the only guard against counting an injected block as
    /// something the user typed, so a false positive here would hide a real turn.
    #[test]
    fn a_plain_turn_is_not_context() {
        for text in ["hello", "<goal-contextx>", "  leading then text"] {
            assert!(!is_hook_context_text(text), "{text:?}");
            assert!(!is_goal_context_text(text), "{text:?}");
        }
    }

    /// Leading whitespace does not hide a marker.
    ///
    /// A reloaded session may have re-indented the stored cell.
    #[test]
    fn leading_whitespace_is_tolerated() {
        assert!(is_hook_context_text("  \n<hook-context>\nbody"));
        assert!(is_goal_context_text("  <goal-context>"));
    }

    /// Non-context text comes back unchanged from `hook_context_body`.
    ///
    /// Its callers pass every user-role cell through it, so returning a slice of
    /// the wrong text would silently rewrite a real turn.
    #[test]
    fn non_context_text_is_returned_unchanged() {
        assert_eq!(hook_context_body("plain words"), "plain words");
        assert_eq!(hook_context_body("<hook-context"), "<hook-context");
    }
}
