/// Input history state: supports navigating previously submitted inputs with arrow keys.
pub(crate) struct InputHistory {
    pub(crate) entries: Vec<String>,
    /// Current navigation position; None means not in navigation mode.
    pub(crate) index: Option<usize>,
    /// The input the user was editing before entering navigation (used to restore on ESC).
    pub(crate) saved: String,
}

impl InputHistory {
    pub(crate) fn new(entries: Vec<String>) -> Self {
        Self {
            entries,
            index: None,
            saved: String::new(),
        }
    }
}
