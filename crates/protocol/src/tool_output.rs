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

/// Semantic segment kind carried by a tool-output fragment.
///
/// `None` means "plain output" (e.g. bash stdout); subagent transcripts tag
/// each fragment so the popup can render role-aware blocks. The enum is
/// intentionally copyable — it rides on every span of a logical line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChunkKind {
    /// User prompt (injected, not from the event stream).
    User,
    /// System notice (Info / session hints).
    System,
    /// Assistant ordinary text (StreamChunk).
    AssistantText,
    /// Assistant reasoning block (ThinkingChunk deltas, one span per block).
    Thinking,
    /// Tool invocation started (StepStarted).
    ToolCall,
    /// Tool invocation succeeded (StepFinished).
    ToolResult,
    /// Tool invocation failed (StepFailed).
    ToolError,
}

/// One ordered text fragment in a tool-progress batch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolOutputChunk {
    pub stream: ToolOutputStream,
    /// Optional semantic segment kind (`None` = plain output).
    pub kind: Option<ChunkKind>,
    pub text: String,
}

impl ToolOutputChunk {
    pub fn stdout(text: impl Into<String>) -> Self {
        Self {
            stream: ToolOutputStream::Stdout,
            kind: None,
            text: text.into(),
        }
    }

    pub fn stderr(text: impl Into<String>) -> Self {
        Self {
            stream: ToolOutputStream::Stderr,
            kind: None,
            text: text.into(),
        }
    }

    pub fn other(text: impl Into<String>) -> Self {
        Self {
            stream: ToolOutputStream::Other,
            kind: None,
            text: text.into(),
        }
    }

    /// Tag this fragment with a semantic segment kind.
    pub fn with_kind(mut self, kind: ChunkKind) -> Self {
        self.kind = Some(kind);
        self
    }
}

/// Styled segment of one logical output line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolOutputSpan {
    pub stream: ToolOutputStream,
    /// Semantic segment kind of this span (None = plain output).
    pub kind: Option<ChunkKind>,
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

    /// The dominant segment kind of the line (first non-None span wins),
    /// used by block renderers to decide the line's role.
    pub fn kind(&self) -> Option<ChunkKind> {
        self.spans
            .iter()
            .find_map(|span| span.kind)
    }

    fn push_char(&mut self, stream: ToolOutputStream, kind: Option<ChunkKind>, ch: char) {
        if let Some(last) = self.spans.last_mut()
            && last.stream == stream
            && last.kind == kind
        {
            last.text.push(ch);
            return;
        }
        self.spans.push(ToolOutputSpan {
            stream,
            kind,
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
    /// Committed structured lines (spans carry `kind`) kept only when
    /// `full_enabled`. Index-addressed so the popup can lay out incrementally
    /// without cloning history.
    full_lines: Vec<ToolOutputLine>,
    total_committed: usize,
    ansi: [AnsiState; 3],
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
            full_lines: Vec::new(),
            total_committed: 0,
            ansi: [AnsiState::default(); 3],
        }
    }

    pub fn push_chunks(&mut self, chunks: &[ToolOutputChunk]) {
        for chunk in chunks {
            for ch in chunk.text.chars() {
                if let Some(ch) = self.ansi[chunk.stream.index()].filter(ch) {
                    self.push_char(chunk.stream, chunk.kind, ch);
                }
            }
        }
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

    /// Number of committed structured lines (only meaningful when
    /// `full_enabled`). Used as the watermark for incremental layout.
    pub fn structured_line_count(&self) -> usize {
        self.full_lines.len()
    }

    /// Cheap per-chunk fingerprint for live-card coalescing: (committed line
    /// count, capped detail length, in-progress line length). Monotonic under
    /// both capped (`new`) and full (`new_full`) buffers, so unchanged views
    /// skip rebuilding the preview.
    pub fn progress_fingerprint(&self) -> (usize, usize, usize) {
        let detail_len = self.detail.len().saturating_add(self.current_detail.len());
        let tail_len = self.current.plain_text().len();
        (self.total_committed, detail_len, tail_len)
    }

    /// Borrow the `i`-th committed structured line (spans carry `kind`).
    pub fn structured_line_at(&self, i: usize) -> Option<&ToolOutputLine> {
        self.full_lines.get(i)
    }

    /// Borrow the in-progress (unterminated) line, if any.
    pub fn current_structured_line(&self) -> Option<&ToolOutputLine> {
        (!self.current.is_empty()).then_some(&self.current)
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

    fn push_char(&mut self, stream: ToolOutputStream, kind: Option<ChunkKind>, ch: char) {
        match ch {
            '\r' => self.clear_current(),
            '\n' => self.commit_current(),
            '\t' => self.push_content_char(stream, kind, ch),
            _ if ch.is_control() => {}
            _ => self.push_content_char(stream, kind, ch),
        }
    }

    fn push_content_char(&mut self, stream: ToolOutputStream, kind: Option<ChunkKind>, ch: char) {
        if self.current_chars < INLINE_LINE_LIMIT_CHARS {
            self.current.push_char(stream, kind, ch);
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
            self.full_lines.push(self.current.clone());
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
    fn kind_tags_survive_struct_layout() {
        let mut output = ToolOutputBuffer::new_full(50_000);
        output.push_chunks(&[
            ToolOutputChunk::other("assistant text\n")
                .with_kind(ChunkKind::AssistantText),
            ToolOutputChunk::other("reasoning\n").with_kind(ChunkKind::Thinking),
            ToolOutputChunk::other("→ bash ls\n").with_kind(ChunkKind::ToolCall),
        ]);

        assert_eq!(output.structured_line_count(), 3);
        assert_eq!(
            output.structured_line_at(0).unwrap().kind(),
            Some(ChunkKind::AssistantText)
        );
        assert_eq!(
            output.structured_line_at(1).unwrap().kind(),
            Some(ChunkKind::Thinking)
        );
        assert_eq!(
            output.structured_line_at(2).unwrap().kind(),
            Some(ChunkKind::ToolCall)
        );
    }

    #[test]
    fn kind_mixes_within_one_line_merge_only_adjacent_same_kind() {
        let mut output = ToolOutputBuffer::new_full(50_000);
        output.push_chunks(&[
            ToolOutputChunk::stdout("plain "),
            ToolOutputChunk::stderr("warn ").with_kind(ChunkKind::ToolError),
            ToolOutputChunk::stdout("done\n"),
        ]);

        let line = output.structured_line_at(0).unwrap();
        assert_eq!(line.spans.len(), 3, "kind change splits spans");
        assert_eq!(line.spans[1].kind, Some(ChunkKind::ToolError));
        // Dominant kind: first non-None span wins.
        assert_eq!(line.kind(), Some(ChunkKind::ToolError));
    }

    #[test]
    fn current_structured_line_reports_unterminated_tail() {
        let mut output = ToolOutputBuffer::new_full(50_000);
        output.push_chunks(&[
            ToolOutputChunk::other("one\n").with_kind(ChunkKind::System),
        ]);
        output.push_chunks(&[ToolOutputChunk::other("t").with_kind(ChunkKind::Thinking)]);

        assert_eq!(output.structured_line_count(), 1);
        let current = output.current_structured_line().unwrap();
        assert_eq!(current.kind(), Some(ChunkKind::Thinking));
        assert_eq!(current.plain_text(), "t");
    }
}
