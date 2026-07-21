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

/// One ordered text fragment in a tool-progress batch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolOutputChunk {
    pub stream: ToolOutputStream,
    pub text: String,
}

impl ToolOutputChunk {
    pub fn stdout(text: impl Into<String>) -> Self {
        Self { stream: ToolOutputStream::Stdout, text: text.into() }
    }

    pub fn stderr(text: impl Into<String>) -> Self {
        Self { stream: ToolOutputStream::Stderr, text: text.into() }
    }

    pub fn other(text: impl Into<String>) -> Self {
        Self { stream: ToolOutputStream::Other, text: text.into() }
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
        self.spans.push(ToolOutputSpan { stream, text: ch.to_string() });
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
            },
            Self::Ground => Some(ch),
            Self::Escape => {
                *self = match ch {
                    '[' => Self::Csi,
                    ']' => Self::Osc,
                    _ => Self::Ground,
                };
                None
            },
            Self::Csi => {
                if ('@'..='~').contains(&ch) {
                    *self = Self::Ground;
                }
                None
            },
            Self::Osc if ch == '\u{7}' => {
                *self = Self::Ground;
                None
            },
            Self::Osc if ch == '\u{1b}' => {
                *self = Self::OscEscape;
                None
            },
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
            },
        }
    }
}

/// Bounded plain-terminal state used for both live rendering and final output.
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
    total_committed: usize,
    ansi: [AnsiState; 3],
}

impl ToolOutputBuffer {
    pub fn new(detail_limit: usize) -> Self {
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
            total_committed: 0,
            ansi: [AnsiState::default(); 3],
        }
    }

    pub fn push_chunks(&mut self, chunks: &[ToolOutputChunk]) {
        for chunk in chunks {
            for ch in chunk.text.chars() {
                if let Some(ch) = self.ansi[chunk.stream.index()].filter(ch) {
                    self.push_char(chunk.stream, ch);
                }
            }
        }
    }

    pub fn preview_lines(&self, limit: usize) -> Vec<ToolOutputLine> {
        if limit == 0 {
            return Vec::new();
        }
        let current_count = usize::from(!self.current.is_empty());
        let skip = self.committed.len().saturating_add(current_count).saturating_sub(limit);
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

    pub fn logical_line_count(&self) -> usize {
        self.total_committed + usize::from(!self.current.is_empty())
    }

    fn push_char(&mut self, stream: ToolOutputStream, ch: char) {
        match ch {
            '\r' => self.clear_current(),
            '\n' => self.commit_current(),
            '\t' => self.push_content_char(stream, ch),
            _ if ch.is_control() => {},
            _ => self.push_content_char(stream, ch),
        }
    }

    fn push_content_char(&mut self, stream: ToolOutputStream, ch: char) {
        if self.current_chars < INLINE_LINE_LIMIT_CHARS {
            self.current.push_char(stream, ch);
            self.current_chars += 1;
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
        output
            .push_chunks(&[ToolOutputChunk::stdout("Downloading 10%\r"), ToolOutputChunk::stdout("Downloading 90%\n")]);

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
        output.push_chunks(&[ToolOutputChunk::stderr("\x1b[3"), ToolOutputChunk::stderr("1merror\x1b[0m\n")]);

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
        output.push_chunks(&[ToolOutputChunk::stdout("one\ntwo\nthree\nfour\nfive\nsix\n")]);

        let preview = output.preview_lines(5);
        let lines: Vec<String> = preview.iter().map(ToolOutputLine::plain_text).collect();
        assert_eq!(lines, ["two", "three", "four", "five", "six"]);
        assert_eq!(output.logical_line_count(), 6);
    }

    #[test]
    fn preview_includes_an_unterminated_current_line() {
        let mut output = ToolOutputBuffer::new(50_000);
        output.push_chunks(&[ToolOutputChunk::stdout("one\ntwo")]);

        let lines: Vec<String> = output.preview_lines(5).iter().map(ToolOutputLine::plain_text).collect();
        assert_eq!(lines, ["one", "two"]);
        assert_eq!(output.logical_line_count(), 2);
    }

    #[test]
    fn detail_limit_counts_characters_and_adds_one_marker() {
        let mut output = ToolOutputBuffer::new(5);
        output.push_chunks(&[ToolOutputChunk::stdout("你好ab"), ToolOutputChunk::stdout("cdef")]);

        assert_eq!(output.detail_text(), "你好abc\n[output truncated]");
        assert_eq!(output.detail_text().matches("[output truncated]").count(), 1);
    }

    #[test]
    fn adjacent_text_from_the_same_stream_reuses_one_span() {
        let mut output = ToolOutputBuffer::new(50_000);
        output.push_chunks(&[ToolOutputChunk::stdout("hel"), ToolOutputChunk::stdout("lo\n")]);

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
        assert_eq!(output.detail_text().chars().count(), 50_000 + TRUNCATION_MARKER.chars().count());
    }

    #[test]
    fn tool_progress_event_keeps_ordered_chunks() {
        let chunks = vec![ToolOutputChunk::stdout("out"), ToolOutputChunk::stderr("err")];
        let event = crate::AgentUpdate::ToolProgress { tool_id: "bash-1".to_string(), chunks: chunks.clone() };

        assert!(matches!(
            event,
            crate::AgentUpdate::ToolProgress { tool_id, chunks: actual }
                if tool_id == "bash-1" && actual == chunks
        ));
    }
}
