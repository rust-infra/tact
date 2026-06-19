/// Search state: manages search term, matching line indices, and current highlight.
pub(crate) struct SearchState {
    pub(crate) term: String,
    pub(crate) matches: Vec<usize>,
    pub(crate) current_match: usize,
}

impl SearchState {
    pub(crate) fn new() -> Self {
        Self {
            term: String::new(),
            matches: Vec::new(),
            current_match: 0,
        }
    }
}
