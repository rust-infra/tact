/// Streaming output state: manages line buffer, table buffer, paragraph buffer, and code block buffer.
#[derive(Default)]
pub(crate) struct StreamState {
    pub(crate) buffer: String,
    pub(crate) table_buffer: Vec<String>,
    pub(crate) paragraph: String,
    pub(crate) code_block: bool,
    pub(crate) code_block_buffer: Vec<String>,
    pub(crate) code_block_lang: String,
    pub(crate) code_block_start_idx: Option<usize>,
    pub(crate) code_block_line_count: usize,
    /// Whether the buffered fence opened with a `mermaid` language tag.
    ///
    /// Set when the opening fence is seen, reset whenever the buffered block
    /// is finalized (valid diagram spliced in, or code-card fallback).
    pub(crate) code_block_is_mermaid: bool,
}
