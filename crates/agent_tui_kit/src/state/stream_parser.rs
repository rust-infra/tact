//! Streaming-text parser: turns `StreamChunk` text into structured events.
//!
//! Extracted from `App::apply_stream_chunk` (step 9 prep): the parse
//! decisions (fence detection, table/paragraph buffering, code-block
//! lifecycle) live here as a pure state machine on [`StreamState`]; the host
//! receives [`StreamEvent`]s and applies them to its log/UI (rendering,
//! streaming indicators, mermaid splicing).
//!
//! The parser never touches a log or theme — only the buffered fields on
//! [`StreamState`]. Event order matches the original implementation exactly.

use super::StreamState;

/// One parsed unit handed to the host for log/UI application.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamEvent {
    /// A complete markdown paragraph to render (`render_markdown_tui`).
    MarkdownParagraph { text: String },
    /// A complete table (raw rows, laid out by `format_table_lines`).
    Table { rows: Vec<String> },
    /// A blank separator row.
    Blank,
    /// A code fence opened; the host records its log anchor and draws the
    /// `╭─ lang` header.
    OpenCodeBlock { lang: String, is_mermaid: bool },
    /// One streaming code line; the host appends it with the live indicator
    /// and updates the previous row.
    CodeLine { text: String },
    /// A code fence closed; the host finalizes the block (mermaid splice or
    /// code-card fallback) using its recorded anchor.
    CloseCodeBlock {
        lang: String,
        lines: Vec<String>,
        line_count: usize,
        is_mermaid: bool,
    },
}

impl StreamState {
    /// Feed one chunk of streamed text; returns the events the host must
    /// apply in order. Unfinished input stays buffered until the next chunk
    /// or the host's final flush.
    pub fn push_chunk(&mut self, text: &str) -> Vec<StreamEvent> {
        self.buffer.push_str(text);
        let mut events = Vec::new();

        while let Some(idx) = self.buffer.find('\n') {
            let line = self.buffer[..idx].to_string();
            self.buffer = self.buffer[idx + 1..].to_string();

            let trimmed = line.trim();
            let is_code_fence = trimmed.starts_with("```");
            let is_code_fence_close = trimmed == "```" && self.code_block;

            if is_code_fence_close {
                let lang = std::mem::take(&mut self.code_block_lang);
                let lines = std::mem::take(&mut self.code_block_buffer);
                let is_mermaid = std::mem::take(&mut self.code_block_is_mermaid);
                let line_count = self.code_block_line_count;
                self.code_block = false;
                self.code_block_line_count = 0;
                events.push(StreamEvent::CloseCodeBlock {
                    lang,
                    lines,
                    line_count,
                    is_mermaid,
                });
            } else if self.code_block {
                self.code_block_buffer.push(line.clone());
                self.code_block_line_count += 1;
                events.push(StreamEvent::CodeLine { text: line });
            } else if is_code_fence {
                let lang = trimmed.strip_prefix("```").unwrap_or("").trim().to_string();

                // If an empty-language fence appears immediately after an
                // in-progress markdown paragraph/list, keep it in normal
                // markdown flow instead of promoting it into a standalone code
                // card. This avoids surprising card extraction for malformed or
                // explanatory fence snippets embedded in prose.
                if lang.is_empty() && !self.paragraph.is_empty() {
                    if !self.table_buffer.is_empty() {
                        events.push(StreamEvent::Table {
                            rows: std::mem::take(&mut self.table_buffer),
                        });
                    }
                    self.paragraph.push('\n');
                    self.paragraph.push_str(&line);
                    continue;
                }

                // Open new code block: flush pending content first
                if !self.paragraph.is_empty() {
                    events.push(StreamEvent::MarkdownParagraph {
                        text: std::mem::take(&mut self.paragraph),
                    });
                }
                if !self.table_buffer.is_empty() {
                    events.push(StreamEvent::Table {
                        rows: std::mem::take(&mut self.table_buffer),
                    });
                }

                self.code_block = true;
                self.code_block_buffer.clear();
                self.code_block_lang = lang.clone();
                // Match `mermaid_fence_opener`: detect Mermaid from the first
                // whitespace-separated info token, case-insensitively, without
                // changing the stored language metadata for ordinary code.
                self.code_block_is_mermaid = lang
                    .split_whitespace()
                    .next()
                    .is_some_and(|token| token.eq_ignore_ascii_case("mermaid"));
                self.code_block_line_count = 1;
                events.push(StreamEvent::OpenCodeBlock {
                    lang,
                    is_mermaid: self.code_block_is_mermaid,
                });
            } else {
                // Regular line handling
                let is_table_line = trimmed.starts_with('|');
                let is_blank = trimmed.is_empty();
                let is_hr = crate::render::render_md::is_horizontal_rule(&line);

                if is_table_line {
                    if !self.paragraph.is_empty() {
                        events.push(StreamEvent::MarkdownParagraph {
                            text: std::mem::take(&mut self.paragraph),
                        });
                    }
                    self.table_buffer.push(line);
                } else if is_blank || is_hr {
                    if !self.paragraph.is_empty() {
                        events.push(StreamEvent::MarkdownParagraph {
                            text: std::mem::take(&mut self.paragraph),
                        });
                    }
                    if !self.table_buffer.is_empty() {
                        events.push(StreamEvent::Table {
                            rows: std::mem::take(&mut self.table_buffer),
                        });
                    }
                    if !is_hr {
                        events.push(StreamEvent::Blank);
                    }
                } else {
                    if !self.table_buffer.is_empty() {
                        events.push(StreamEvent::Table {
                            rows: std::mem::take(&mut self.table_buffer),
                        });
                    }
                    if !self.paragraph.is_empty() {
                        self.paragraph.push('\n');
                    }
                    self.paragraph.push_str(&line);
                }
            }
        }

        events
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn push(state: &mut StreamState, text: &str) -> Vec<StreamEvent> {
        state.push_chunk(text)
    }

    #[test]
    fn plain_lines_buffer_until_a_flush_trigger() {
        // A bare "line\n" buffers into the paragraph (matching the original
        // implementation); a following blank row triggers the flush.
        let mut s = StreamState::default();
        let events = push(&mut s, "line one\nline two\n");
        assert!(events.is_empty(), "paragraph is buffered until a trigger");
        assert_eq!(s.paragraph, "line one\nline two");

        let events = push(&mut s, "\n");
        assert_eq!(
            events,
            vec![
                StreamEvent::MarkdownParagraph {
                    text: "line one\nline two".into()
                },
                StreamEvent::Blank,
            ]
        );
        assert!(s.paragraph.is_empty());
        assert!(s.buffer.is_empty());
    }

    #[test]
    fn blank_line_flushes_paragraph_then_emits_blank() {
        let mut s = StreamState::default();
        let events = push(&mut s, "hello\n\nworld\n\n");
        assert_eq!(
            events,
            vec![
                StreamEvent::MarkdownParagraph {
                    text: "hello".into()
                },
                StreamEvent::Blank,
                StreamEvent::MarkdownParagraph {
                    text: "world".into()
                },
                StreamEvent::Blank,
            ]
        );
    }

    #[test]
    fn table_rows_buffer_until_blank_or_paragraph() {
        let mut s = StreamState::default();
        let events = push(&mut s, "| a | b |\n| 1 | 2 |\n\n");
        assert_eq!(
            events,
            vec![
                StreamEvent::Table {
                    rows: vec!["| a | b |".into(), "| 1 | 2 |".into()]
                },
                StreamEvent::Blank,
            ]
        );
    }

    #[test]
    fn code_block_emits_open_lines_close() {
        let mut s = StreamState::default();
        let events = push(&mut s, "```rust\nfn main() {}\n```\n");
        assert_eq!(
            events,
            vec![
                StreamEvent::OpenCodeBlock {
                    lang: "rust".into(),
                    is_mermaid: false,
                },
                StreamEvent::CodeLine {
                    text: "fn main() {}".into()
                },
                StreamEvent::CloseCodeBlock {
                    lang: "rust".into(),
                    lines: vec!["fn main() {}".into()],
                    // 2 = the fence/header row + one code line (matches the
                    // original implementation's span bookkeeping).
                    line_count: 2,
                    is_mermaid: false,
                },
            ]
        );
        assert!(!s.code_block);
    }

    #[test]
    fn mermaid_fence_is_detected_case_insensitively() {
        let mut s = StreamState::default();
        let events = push(&mut s, "```Mermaid\na-->b\n```\n");
        assert!(matches!(
            events[0],
            StreamEvent::OpenCodeBlock {
                is_mermaid: true,
                ..
            }
        ));
        assert!(matches!(
            events[2],
            StreamEvent::CloseCodeBlock {
                is_mermaid: true,
                ..
            }
        ));
    }

    #[test]
    fn empty_fence_inside_paragraph_stays_in_markdown_flow() {
        let mut s = StreamState::default();
        push(&mut s, "some prose\n```\nmore prose\n");
        assert!(s.paragraph.contains("```"), "fence stays in paragraph flow");
        // A blank row flushes the whole paragraph including the fence line.
        let events = push(&mut s, "\n");
        assert_eq!(
            events,
            vec![
                StreamEvent::MarkdownParagraph {
                    text: "some prose\n```\nmore prose".into()
                },
                StreamEvent::Blank,
            ]
        );
    }

    #[test]
    fn unfinished_input_stays_buffered() {
        let mut s = StreamState::default();
        let events = push(&mut s, "partial li");
        assert!(events.is_empty());
        assert_eq!(s.buffer, "partial li");

        let events = push(&mut s, "ne\n");
        // The completed line joins the paragraph buffer (no flush trigger yet).
        assert!(events.is_empty());
        assert_eq!(s.paragraph, "partial line");
        assert!(s.buffer.is_empty());

        let events = push(&mut s, "\n");
        assert_eq!(
            events,
            vec![
                StreamEvent::MarkdownParagraph {
                    text: "partial line".into()
                },
                StreamEvent::Blank,
            ]
        );
    }

    #[test]
    fn horizontal_rule_is_dropped() {
        let mut s = StreamState::default();
        let events = push(&mut s, "text\n---\n");
        assert_eq!(
            events,
            vec![StreamEvent::MarkdownParagraph {
                text: "text".into()
            }],
            "--- hr must be discarded (no Blank event)"
        );
    }
}
