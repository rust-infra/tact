//! Prompts the protocol itself defines.
//!
//! Some slash commands are not a frontend gesture but a fixed message sent to
//! the model. The text belongs next to [`UserCommand`](crate::UserCommand)
//! because every frontend already depends on this crate, and a copy per
//! frontend would let the wording drift.

/// What a cut-short turn continues with.
///
/// A cut-off turn may have left a command running or half-applied, so this is
/// written about *evidence* rather than encouragement: check before repeating a
/// state change, and say so when the state cannot be determined.
pub const TRY_AGAIN_PROMPT: &str = "\
Continue where you left off. Your last turn was cut short, so a command or other \
action it started may have finished or may still be running. Before you repeat one \
that changes state, confirm it did not take effect and is not still running; if you \
cannot tell, say so instead of repeating it.";

#[cfg(test)]
mod tests {
    use super::TRY_AGAIN_PROMPT;

    /// The line continuations in the literal must not smuggle in a stray space
    /// or drop one; the paragraph has to read as the single sentence it is.
    #[test]
    fn the_prompt_is_one_space_separated_paragraph() {
        assert!(!TRY_AGAIN_PROMPT.contains('\n'), "no newlines");
        assert!(
            !TRY_AGAIN_PROMPT.contains("  "),
            "no double spaces: {TRY_AGAIN_PROMPT:?}"
        );
        assert!(TRY_AGAIN_PROMPT.starts_with("Continue where you left off."));
        assert!(TRY_AGAIN_PROMPT.ends_with("instead of repeating it."));
    }
}
