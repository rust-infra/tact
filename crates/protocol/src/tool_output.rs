//! Incremental output types shared by tool producers and the TUI.

use std::collections::VecDeque;

const INLINE_HISTORY_LINES: usize = 5;
const INLINE_LINE_LIMIT_CHARS: usize = 10_000;
const TRUNCATION_MARKER: &str = "\n[output truncated]";

/// Origin of an incremental tool-output fragment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolOutputStream {
    Stdout,
    Stderr,
    Other,
}

impl ToolOutputStream {
    const fn index(self) -> usize {
        match self {
            Self::Stdout => 0,
            Self::Stderr => 1,
            Self::Other => 2,
        }
    }
}

/// Which section of a subagent transcript a chunk belongs to.
///
/// The subagent forwarder tags every `ToolProgress` chunk so the sectioned
/// popup can group thinking blocks, tool steps and streamed context into
/// separate sections. Non-subagent chunks keep the default (`Context`);
/// the flat card stream never reads the tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SubagentSection {
    /// Streamed assistant text, info messages, errors, and the initial prompt.
    #[default]
    Context,
    /// A reasoning / thinking block.
    Thinking,
    /// A tool invocation: the start line, its result, or its error.
    Tool,
}

impl SubagentSection {
    /// Canonical display order for the sectioned subagent popup.
    pub const ORDERED: [SubagentSection; 3] = [
        SubagentSection::Thinking,
        SubagentSection::Tool,
        SubagentSection::Context,
    ];
}

/// Marker the subagent forwarder prefixes to every thinking block in the flat
/// card stream, and the sectioned popup strips before rendering its own single
/// section header. Lives here so both sides read the same string.
pub const THINKING_SECTION_HEADER: &str = "🧠 Thinking";

/// One contiguous body of a subagent transcript section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubagentSectionBlock {
    pub section: SubagentSection,
    pub text: String,
}

/// One ordered text fragment in a tool-progress batch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolOutputChunk {
    pub stream: ToolOutputStream,
    pub text: String,
    /// Section tag consumed by the subagent popup; the flat card stream
    /// ignores it. Non-subagent chunks keep the default (`Context`).
    pub section: SubagentSection,
}

impl ToolOutputChunk {
    pub fn stdout(text: impl Into<String>) -> Self {
        Self {
            stream: ToolOutputStream::Stdout,
            text: text.into(),
            section: SubagentSection::Context,
        }
    }

    pub fn stderr(text: impl Into<String>) -> Self {
        Self {
            stream: ToolOutputStream::Stderr,
            text: text.into(),
            section: SubagentSection::Context,
        }
    }

    pub fn other(text: impl Into<String>) -> Self {
        Self {
            stream: ToolOutputStream::Other,
            text: text.into(),
            section: SubagentSection::Context,
        }
    }

    /// Tag this chunk as belonging to a subagent transcript section.
    pub fn with_section(mut self, section: SubagentSection) -> Self {
        self.section = section;
        self
    }
}

/// Styled segment of one logical output line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolOutputSpan {
    pub stream: ToolOutputStream,
    pub text: String,
}

/// One logical output line, preserving the origin of adjacent segments.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ToolOutputLine {
    pub spans: Vec<ToolOutputSpan>,
}

impl ToolOutputLine {
    pub fn plain_text(&self) -> String {
        self.spans.iter().map(|span| span.text.as_str()).collect()
    }

    fn push_char(&mut self, stream: ToolOutputStream, ch: char) {
        if let Some(last) = self.spans.last_mut()
            && last.stream == stream
        {
            last.text.push(ch);
            return;
        }
        self.spans.push(ToolOutputSpan {
            stream,
            text: ch.to_string(),
        });
    }

    fn is_empty(&self) -> bool {
        self.spans.is_empty()
    }
}

#[derive(Debug, Clone, Copy, Default)]
enum AnsiState {
    #[default]
    Ground,
    Escape,
    Csi,
    Osc,
    OscEscape,
}

impl AnsiState {
    fn filter(&mut self, ch: char) -> Option<char> {
        match *self {
            Self::Ground if ch == '\u{1b}' => {
                *self = Self::Escape;
                None
            }
            Self::Ground => Some(ch),
            Self::Escape => {
                *self = match ch {
                    '[' => Self::Csi,
                    ']' => Self::Osc,
                    _ => Self::Ground,
                };
                None
            }
            Self::Csi => {
                if ('@'..='~').contains(&ch) {
                    *self = Self::Ground;
                }
                None
            }
            Self::Osc if ch == '\u{7}' => {
                *self = Self::Ground;
                None
            }
            Self::Osc if ch == '\u{1b}' => {
                *self = Self::OscEscape;
                None
            }
            Self::Osc => None,
            Self::OscEscape => {
                *self = if ch == '\\' {
                    Self::Ground
                } else if ch == '\u{1b}' {
                    Self::OscEscape
                } else {
                    Self::Osc
                };
                None
            }
        }
    }
}

/// Plain-terminal state for live rendering (capped detail) and full output capture.
#[derive(Debug, Clone)]
pub struct ToolOutputBuffer {
    committed: VecDeque<ToolOutputLine>,
    current: ToolOutputLine,
    current_chars: usize,
    detail: String,
    detail_chars: usize,
    current_detail: String,
    current_detail_chars: usize,
    current_detail_truncated: bool,
    detail_truncated: bool,
    detail_limit: usize,
    /// When set, accumulate every character (after ANSI stripping) without a
    /// limit so the complete output can be recovered. Left off for buffers that
    /// only feed the capped live preview (e.g. bash's TUI card).
    full_enabled: bool,
    /// Unlimited accumulation, only populated when `full_enabled`.
    full_detail: String,
    full_current_detail: String,
    total_committed: usize,
    ansi: [AnsiState; 3],
    /// Structured transcript sections (subagent popup). Each chunk's
    /// ANSI-filtered text is appended to the last block when the section
    /// matches, otherwise a new block is opened. Non-subagent buffers keep a
    /// single `Context` block that nobody reads.
    sections: Vec<SubagentSectionBlock>,
}

impl ToolOutputBuffer {
    /// Capped buffer: keeps only the live preview and `detail_limit` chars of detail.
    pub fn new(detail_limit: usize) -> Self {
        Self::build(detail_limit, false)
    }

    /// Like [`new`], but also accumulates the complete output for
    /// [`full_detail_text`]/[`take_full_detail`].
    pub fn new_full(detail_limit: usize) -> Self {
        Self::build(detail_limit, true)
    }

    fn build(detail_limit: usize, full_enabled: bool) -> Self {
        Self {
            committed: VecDeque::with_capacity(INLINE_HISTORY_LINES),
            current: ToolOutputLine::default(),
            current_chars: 0,
            detail: String::new(),
            detail_chars: 0,
            current_detail: String::new(),
            current_detail_chars: 0,
            current_detail_truncated: false,
            detail_truncated: false,
            detail_limit,
            full_enabled,
            full_detail: String::new(),
            full_current_detail: String::new(),
            total_committed: 0,
            ansi: [AnsiState::default(); 3],
            sections: Vec::new(),
        }
    }

    pub fn push_chunks(&mut self, chunks: &[ToolOutputChunk]) {
        for chunk in chunks {
            // Filter ANSI once, then feed both the flat line buffer and the
            // structured section transcript so their text always agrees.
            let mut filtered = String::new();
            for ch in chunk.text.chars() {
                if let Some(ch) = self.ansi[chunk.stream.index()].filter(ch) {
                    filtered.push(ch);
                }
            }
            if !filtered.is_empty() {
                self.append_section(chunk.section, &filtered);
            }
            for ch in filtered.chars() {
                self.push_char(chunk.stream, ch);
            }
        }
    }

    fn append_section(&mut self, section: SubagentSection, text: &str) {
        if let Some(last) = self.sections.last_mut()
            && last.section == section
        {
            last.text.push_str(text);
            return;
        }
        self.sections.push(SubagentSectionBlock {
            section,
            text: text.to_string(),
        });
    }

    /// Structured transcript sections (subagent popup), in arrival order.
    pub fn sections(&self) -> &[SubagentSectionBlock] {
        &self.sections
    }

    /// Takes the structured sections, leaving the buffer with none.
    pub fn take_sections(&mut self) -> Vec<SubagentSectionBlock> {
        std::mem::take(&mut self.sections)
    }

    pub fn preview_lines(&self, limit: usize) -> Vec<ToolOutputLine> {
        if limit == 0 {
            return Vec::new();
        }
        let current_count = usize::from(!self.current.is_empty());
        let skip = self
            .committed
            .len()
            .saturating_add(current_count)
            .saturating_sub(limit);
        self.committed
            .iter()
            .cloned()
            .chain((!self.current.is_empty()).then(|| self.current.clone()))
            .skip(skip)
            .collect()
    }

    pub fn detail_text(&self) -> String {
        let mut text = self.detail.clone();
        if !self.detail_truncated {
            text.push_str(&self.current_detail);
        }
        if self.detail_truncated || self.current_detail_truncated {
            text.push_str(TRUNCATION_MARKER);
        }
        text
    }

    /// Returns the complete (un-truncated) accumulated output.
    pub fn full_detail_text(&self) -> String {
        let mut text = self.full_detail.clone();
        text.push_str(&self.full_current_detail);
        text
    }

    /// Byte length of [`full_detail_text`] without materializing it. Cheap
    /// fingerprint for caches that only need to know when the output grew.
    pub fn full_detail_len(&self) -> usize {
        self.full_detail.len() + self.full_current_detail.len()
    }

    /// Takes ownership of the unlimited full-detail buffers, leaving them empty.
    pub fn take_full_detail(&mut self) -> String {
        let mut text = std::mem::take(&mut self.full_detail);
        text.push_str(&self.full_current_detail);
        self.full_current_detail.clear();
        text
    }

    pub fn logical_line_count(&self) -> usize {
        self.total_committed + usize::from(!self.current.is_empty())
    }

    fn push_char(&mut self, stream: ToolOutputStream, ch: char) {
        match ch {
            '\r' => self.clear_current(),
            '\n' => self.commit_current(),
            '\t' => self.push_content_char(stream, ch),
            _ if ch.is_control() => {}
            _ => self.push_content_char(stream, ch),
        }
    }

    fn push_content_char(&mut self, stream: ToolOutputStream, ch: char) {
        if self.current_chars < INLINE_LINE_LIMIT_CHARS {
            self.current.push_char(stream, ch);
            self.current_chars += 1;
        }
        if self.full_enabled {
            self.full_current_detail.push(ch);
        }
        if !self.detail_truncated && !self.current_detail_truncated {
            if self.detail_chars + self.current_detail_chars < self.detail_limit {
                self.current_detail.push(ch);
                self.current_detail_chars += 1;
            } else {
                self.current_detail_truncated = true;
            }
        }
    }

    fn clear_current(&mut self) {
        self.current = ToolOutputLine::default();
        self.current_chars = 0;
        self.current_detail.clear();
        self.current_detail_chars = 0;
        self.current_detail_truncated = false;
        self.full_current_detail.clear();
    }

    fn commit_current(&mut self) {
        let current_detail = std::mem::take(&mut self.current_detail);
        self.current_detail_chars = 0;
        let current_detail_truncated = std::mem::take(&mut self.current_detail_truncated);
        self.append_detail(&current_detail);
        if current_detail_truncated {
            self.detail_truncated = true;
        } else {
            self.append_detail("\n");
        }
        if self.full_enabled {
            self.full_detail.push_str(&self.full_current_detail);
            self.full_detail.push('\n');
            self.full_current_detail.clear();
        }
        self.committed.push_back(std::mem::take(&mut self.current));
        self.current_chars = 0;
        while self.committed.len() > INLINE_HISTORY_LINES {
            self.committed.pop_front();
        }
        self.total_committed += 1;
    }

    fn append_detail(&mut self, text: &str) {
        if self.detail_truncated {
            return;
        }
        let remaining = self.detail_limit.saturating_sub(self.detail_chars);
        self.detail.extend(text.chars().take(remaining));
        let appended = text.chars().count().min(remaining);
        self.detail_chars += appended;
        if text.chars().count() > appended {
            self.detail_truncated = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn carriage_return_replaces_the_current_line() {
        let mut output = ToolOutputBuffer::new(50_000);
        output.push_chunks(&[
            ToolOutputChunk::stdout("Downloading 10%\r"),
            ToolOutputChunk::stdout("Downloading 90%\n"),
        ]);

        assert_eq!(output.detail_text(), "Downloading 90%\n");
        assert_eq!(output.logical_line_count(), 1);
    }

    #[test]
    fn mixed_streams_keep_order_and_identity() {
        let mut output = ToolOutputBuffer::new(50_000);
        output.push_chunks(&[
            ToolOutputChunk::stdout("build "),
            ToolOutputChunk::stderr("warning"),
            ToolOutputChunk::stdout(" done\n"),
        ]);

        let line = output.preview_lines(5).pop().unwrap();
        assert_eq!(line.plain_text(), "build warning done");
        assert_eq!(line.spans[0].stream, ToolOutputStream::Stdout);
        assert_eq!(line.spans[1].stream, ToolOutputStream::Stderr);
        assert_eq!(line.spans[2].stream, ToolOutputStream::Stdout);
    }

    #[test]
    fn ansi_csi_sequences_can_span_chunks() {
        let mut output = ToolOutputBuffer::new(50_000);
        output.push_chunks(&[
            ToolOutputChunk::stderr("\x1b[3"),
            ToolOutputChunk::stderr("1merror\x1b[0m\n"),
        ]);

        assert_eq!(output.detail_text(), "error\n");
    }

    #[test]
    fn ansi_osc_sequences_are_removed() {
        let mut output = ToolOutputBuffer::new(50_000);
        output.push_chunks(&[
            ToolOutputChunk::stdout("before\x1b]0;secret"),
            ToolOutputChunk::stdout(" title\x1b\\after\n"),
        ]);

        assert_eq!(output.detail_text(), "beforeafter\n");
    }

    #[test]
    fn preview_keeps_only_the_latest_five_lines() {
        let mut output = ToolOutputBuffer::new(50_000);
        output.push_chunks(&[ToolOutputChunk::stdout(
            "one\ntwo\nthree\nfour\nfive\nsix\n",
        )]);

        let preview = output.preview_lines(5);
        let lines: Vec<String> = preview.iter().map(ToolOutputLine::plain_text).collect();
        assert_eq!(lines, ["two", "three", "four", "five", "six"]);
        assert_eq!(output.logical_line_count(), 6);
    }

    #[test]
    fn preview_includes_an_unterminated_current_line() {
        let mut output = ToolOutputBuffer::new(50_000);
        output.push_chunks(&[ToolOutputChunk::stdout("one\ntwo")]);

        let lines: Vec<String> = output
            .preview_lines(5)
            .iter()
            .map(ToolOutputLine::plain_text)
            .collect();
        assert_eq!(lines, ["one", "two"]);
        assert_eq!(output.logical_line_count(), 2);
    }

    #[test]
    fn detail_limit_counts_characters_and_adds_one_marker() {
        let mut output = ToolOutputBuffer::new_full(5);
        output.push_chunks(&[
            ToolOutputChunk::stdout("你好ab"),
            ToolOutputChunk::stdout("cdef"),
        ]);

        assert_eq!(output.detail_text(), "你好abc\n[output truncated]");
        assert_eq!(
            output.detail_text().matches("[output truncated]").count(),
            1
        );
        // full_detail keeps everything past the capped detail_limit.
        assert_eq!(output.full_detail_text(), "你好abcdef");
    }

    #[test]
    fn capped_buffer_does_not_accumulate_full_detail() {
        let mut output = ToolOutputBuffer::new(50_000);
        output.push_chunks(&[ToolOutputChunk::stdout("hello\nworld")]);

        assert_eq!(output.detail_text(), "hello\nworld");
        assert!(output.full_detail_text().is_empty());
    }

    #[test]
    fn take_full_detail_moves_and_clears() {
        let mut output = ToolOutputBuffer::new_full(50_000);
        output.push_chunks(&[ToolOutputChunk::stdout("hello\nworld")]);

        assert_eq!(output.take_full_detail(), "hello\nworld");
        assert!(output.full_detail_text().is_empty());
        // Capped detail is independent of take_full_detail.
        assert_eq!(output.detail_text(), "hello\nworld");
    }

    #[test]
    fn full_detail_respects_carriage_return() {
        let mut output = ToolOutputBuffer::new_full(5);
        output.push_chunks(&[
            ToolOutputChunk::stdout("Downloading 10%\r"),
            ToolOutputChunk::stdout("Downloading 90%\n"),
        ]);

        assert_eq!(output.full_detail_text(), "Downloading 90%\n");
    }

    #[test]
    fn adjacent_text_from_the_same_stream_reuses_one_span() {
        let mut output = ToolOutputBuffer::new(50_000);
        output.push_chunks(&[
            ToolOutputChunk::stdout("hel"),
            ToolOutputChunk::stdout("lo\n"),
        ]);

        let line = output.preview_lines(5).pop().unwrap();
        assert_eq!(line.spans.len(), 1);
        assert_eq!(line.spans[0].text, "hello");
    }

    #[test]
    fn unterminated_line_does_not_grow_the_live_preview_without_bound() {
        let mut output = ToolOutputBuffer::new(50_000);
        output.push_chunks(&[ToolOutputChunk::stdout("x".repeat(100_000))]);

        let preview = output.preview_lines(5).pop().unwrap().plain_text();
        assert_eq!(preview.chars().count(), INLINE_LINE_LIMIT_CHARS);
        assert_eq!(
            output.detail_text().chars().count(),
            50_000 + TRUNCATION_MARKER.chars().count()
        );
    }

    #[test]
    fn tool_progress_event_keeps_ordered_chunks() {
        let chunks = vec![
            ToolOutputChunk::stdout("out"),
            ToolOutputChunk::stderr("err"),
        ];
        let event = crate::AgentUpdate::ToolProgress {
            tool_id: "bash-1".to_string(),
            chunks: chunks.clone(),
        };

        assert!(matches!(
            event,
            crate::AgentUpdate::ToolProgress { tool_id, chunks: actual }
                if tool_id == "bash-1" && actual == chunks
        ));
    }

    #[test]
    fn section_tag_round_trips_on_chunk() {
        let chunk = ToolOutputChunk::other("body")
            .with_section(SubagentSection::Thinking)
            .with_section(SubagentSection::Tool);

        assert_eq!(chunk.section, SubagentSection::Tool);
        assert_eq!(chunk.text, "body");
        assert_eq!(chunk.stream, ToolOutputStream::Other);
        // Defaults are Context for all plain constructors.
        assert_eq!(
            ToolOutputChunk::stdout("s").section,
            SubagentSection::Context
        );
        assert_eq!(
            ToolOutputChunk::stderr("s").section,
            SubagentSection::Context
        );
        assert_eq!(
            ToolOutputChunk::other("s").section,
            SubagentSection::Context
        );
    }

    #[test]
    fn sections_merge_same_section_and_open_new_blocks_on_change() {
        let mut output = ToolOutputBuffer::new_full(50_000);
        output.push_chunks(&[
            ToolOutputChunk::other("stream one").with_section(SubagentSection::Context),
            ToolOutputChunk::other("stream two").with_section(SubagentSection::Context),
            ToolOutputChunk::other("\n\nplan\n").with_section(SubagentSection::Thinking),
            ToolOutputChunk::other("→ bash ls\n\n").with_section(SubagentSection::Tool),
        ]);

        let sections = output.sections();
        assert_eq!(sections.len(), 3);
        assert_eq!(sections[0].section, SubagentSection::Context);
        assert_eq!(sections[0].text, "stream onestream two");
        assert_eq!(sections[1].section, SubagentSection::Thinking);
        assert_eq!(sections[1].text, "\n\nplan\n");
        assert_eq!(sections[2].section, SubagentSection::Tool);
        assert_eq!(sections[2].text, "→ bash ls\n\n");
        // The flat stream stays byte-identical to the section text.
        assert_eq!(
            output.full_detail_text(),
            "stream onestream two\n\nplan\n→ bash ls\n\n"
        );
    }

    #[test]
    fn sections_strip_ansi_like_the_flat_stream() {
        let mut output = ToolOutputBuffer::new_full(50_000);
        output
            .push_chunks(&[ToolOutputChunk::other("\x1b[31merror\x1b[0m\n")
                .with_section(SubagentSection::Context)]);

        assert_eq!(output.sections()[0].text, "error\n");
        assert_eq!(output.full_detail_text(), "error\n");
    }

    #[test]
    fn take_sections_drains_the_buffer() {
        let mut output = ToolOutputBuffer::new_full(50_000);
        output.push_chunks(&[ToolOutputChunk::other("body").with_section(SubagentSection::Tool)]);

        let taken = output.take_sections();
        assert_eq!(taken.len(), 1);
        assert_eq!(taken[0].section, SubagentSection::Tool);
        assert!(output.sections().is_empty());
    }
}
