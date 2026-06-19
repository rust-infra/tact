/// Streaming output state: manages line buffer, table buffer, paragraph buffer, and code block buffer.
pub(crate) struct StreamState {
    pub(crate) buffer: String,
    pub(crate) table_buffer: Vec<String>,
    pub(crate) paragraph: String,
    pub(crate) code_block: bool,
    pub(crate) code_block_buffer: Vec<String>,
    pub(crate) code_block_lang: String,
    pub(crate) code_block_start_idx: Option<usize>,
    pub(crate) code_block_line_count: usize,
}

impl StreamState {
    pub(crate) fn new() -> Self {
        Self {
            buffer: String::new(),
            table_buffer: Vec::new(),
            paragraph: String::new(),
            code_block: false,
            code_block_buffer: Vec::new(),
            code_block_lang: String::new(),
            code_block_start_idx: None,
            code_block_line_count: 0,
        }
    }
}
