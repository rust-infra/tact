use crate::render::render_md::{format_table, render_markdown_tui};
use crate::widgets::state::*;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{ListState, ScrollbarState};
use tact_core::{AgentUpdate, StepStatus};

const CODE_BG: Color = Color::Rgb(30, 35, 50);
const STREAMING_INDICATOR: &str = " ▌";

impl App {
    pub(crate) fn is_message_visible(&self, idx: usize) -> bool {
        // messages[] 布局（一个已完成 thinking block 的物理索引范围）:

        // [blank_idx]   ""                              ← 隔离空行
        // [title_idx]   "🧠 Thinking (8 lines)…"        ← title_idx
        // [title_idx+1] "│ Let me analyze…"              ← 思考内容行 1
        //   ...
        // [end_idx]     "│ Solution: use B…"             ← end_idx ← 最后一行
        // [end_idx+1]   ""                              ← 隔离空行（close 时插入）
        for block in &self.thinking.blocks {
            if idx > block.title_idx && idx <= block.end_idx {
                let total = block.end_idx.saturating_sub(block.title_idx);
                let visible_start = block.scroll_offset.min(total.saturating_sub(1));
                let visible_end = (block.scroll_offset + 3).min(total);
                let relative = idx.saturating_sub(block.title_idx + 1);
                return relative >= visible_start && relative < visible_end;
            }
        }
        true
    }

    /// Left indent columns for nested log content at this physical row.
    pub(crate) fn nested_log_indent(&self, phys: usize) -> u16 {
        self.raw_message_types
            .get(phys)
            .copied()
            .unwrap_or(RawMessageType::LLM)
            .log_indent()
    }

    /// Map a logical line number to the physical index in messages.
    /// Returns None if the logical line number exceeds the fixed message range
    /// (meaning it's an incomplete streaming line).
    pub(crate) fn visible_message_index(&self, logical_idx: usize) -> Option<usize> {
        let mut visible_count = 0;
        for idx in 0..self.messages.len() {
            if self.is_message_visible(idx) {
                if visible_count == logical_idx {
                    return Some(idx);
                }
                visible_count += 1;
            }
        }
        None
    }

    /// Find the word boundary at the given mouse column in the raw text of a
    /// specific logical line. Returns (word_start_byte, word_end_byte).
    /// Words consist of letters, digits, underscores, and hyphens; other
    /// characters are separators.
    pub(crate) fn find_word_bounds(
        &self,
        logical_idx: usize,
        col: usize,
    ) -> Option<(usize, usize)> {
        let phys_idx = self.visible_message_index(logical_idx)?;
        let text = self.raw_messages.get(phys_idx)?;
        let bytes = text.as_bytes();
        let mut byte_pos = 0;
        let mut char_count = 0;
        // Convert column position to byte offset
        while byte_pos < bytes.len() && char_count < col {
            let c = text[byte_pos..].chars().next()?;
            byte_pos += c.len_utf8();
            char_count += 1;
        }
        if byte_pos >= bytes.len() || bytes.is_empty() {
            return None;
        }
        // Expand from click position to find word boundaries
        let classify = |b: u8| -> bool { b.is_ascii_alphanumeric() || b == b'_' || b == b'-' };
        let mut start = byte_pos;
        let mut end = byte_pos;
        // Expand left
        while start > 0 {
            if classify(bytes[start - 1]) {
                start -= 1;
            } else {
                break;
            }
        }
        // Expand right
        while end < bytes.len() {
            if classify(bytes[end]) {
                end += 1;
            } else {
                break;
            }
        }
        if start < end {
            Some((start, end))
        } else {
            None
        }
    }

    /// O(1) version: uses the cache mapping built by render_log_panel.
    /// Returns None if the physical index is not visible or cache hasn't been built yet.
    pub(crate) fn phys_to_logical_fast(&self, phys_idx: usize) -> Option<usize> {
        self.log_scroll
            .phys_to_logical_cache
            .get(phys_idx)
            .copied()
            .flatten()
    }

    /// Map a visual line number (mouse click row) back to a logical line number.
    /// Depends on the log_scroll.visual_start prefix array updated each frame
    /// by render_log_panel.
    pub(crate) fn logical_from_visual(&self, visual_row: usize) -> usize {
        if self.log_scroll.visual_start.is_empty() {
            return visual_row;
        }
        match self.log_scroll.visual_start.binary_search(&visual_row) {
            Ok(i) => i,
            Err(i) => i.saturating_sub(1),
        }
    }

    /// Total logical line count of the current Log area (fixed messages + incomplete streaming lines).
    pub(crate) fn total_log_lines(&self) -> usize {
        let visible_count = (0..self.messages.len())
            .filter(|&idx| self.is_message_visible(idx))
            .count();
        visible_count + if self.stream.buffer.is_empty() { 0 } else { 1 }
    }

    /// Close the currently active thinking block, adding it to thinking_blocks
    /// and showing only the last 3 lines by default.
    pub(crate) fn close_active_thinking_block(&mut self) {
        if let Some(blank_idx) = self.thinking.active_start.take() {
            let end = self.thinking.active_end.unwrap_or(blank_idx);
            self.thinking.active_end = None;
            self.thinking.title_added = false;
            // blank_idx is the isolation blank line above the title (inserted in ThinkingChunk)
            // title at blank_idx+1, thinking content lines at blank_idx+2..=end
            if end > blank_idx {
                // Insert a blank line at the end as visual separator (isolation line above already inserted during streaming)
                self.insert_msg(
                    end + 1,
                    Line::from(""),
                    String::new(),
                    RawMessageType::LLMThinking,
                );

                let title_idx = blank_idx + 1;
                let end_idx = end; // Not affected by insert since insert happens after end
                let total = end_idx.saturating_sub(title_idx);
                let scroll_offset = if total > 3 { total - 3 } else { 0 };

                // Pre-render Markdown and cache preview text, avoiding per-frame re-rendering for popups/cards
                let mut preview_lines = Vec::with_capacity(total);
                let mut raw_content = String::new();
                for i in 1..=total {
                    let phys_idx = title_idx + i;
                    if phys_idx < self.raw_messages.len() {
                        let line = &self.raw_messages[phys_idx];
                        let stripped = line.strip_prefix("│ ").unwrap_or(line);
                        preview_lines.push(stripped.to_string());
                        raw_content.push_str(stripped);
                        raw_content.push('\n');
                    }
                }
                let (styled_lines, _) = render_markdown_tui(&raw_content);

                let elapsed = self
                    .thinking
                    .thinking_start
                    .take()
                    .map(|start| start.elapsed())
                    .unwrap_or_default();

                self.thinking.blocks.push(ThinkingBlock {
                    title_idx,
                    end_idx,
                    scroll_offset,
                    cached_preview: preview_lines,
                    cached_markdown: styled_lines,
                    elapsed,
                });
            }
        }
        // log_scroll clamping is deferred to render_log_panel,
        // avoiding scroll offset mismatches between the update phase and the current screen render.
        // See the clamp logic at the start of render_log_panel in render.rs.
    }

    /// Flush leftover lines in the thinking buffer and close the currently active thinking block.
    /// Does nothing if there is no active thinking block.
    pub(crate) fn flush_and_close_thinking(&mut self) {
        if self.thinking.active_start.is_some() {
            if !self.thinking.buffer.is_empty() {
                let style = Style::default()
                    .fg(Color::Gray)
                    .add_modifier(Modifier::ITALIC)
                    .bg(Color::Rgb(35, 35, 45));
                let flush_text = if self.thinking.buffer.trim().is_empty() {
                    String::new()
                } else {
                    format!("│ {}", self.thinking.buffer)
                };
                if !flush_text.is_empty() {
                    self.append_msg(
                        Line::from(Span::styled(flush_text.clone(), style)),
                        flush_text,
                        RawMessageType::LLMThinking,
                    );
                }
                self.thinking.buffer.clear();
                self.thinking.active_end = Some(self.messages.len() - 1);
            }
            self.close_active_thinking_block();
        }
    }

    /// Flush residual content from the streaming buffer into the message list.
    pub(crate) fn flush_stream_pending(&mut self) {
        // Flush accumulated table
        if !self.stream.table_buffer.is_empty() {
            let (lines, raw_lines) = format_table(&self.stream.table_buffer, &self.theme);
            self.extend_msgs(lines, raw_lines, RawMessageType::LLM);
            self.stream.table_buffer.clear();
        }
        // Flush incomplete code block (interrupted stream)
        if self.stream.code_block {
            const MAX_CODE_PREVIEW: usize = 30;
            let lang = std::mem::take(&mut self.stream.code_block_lang);
            let code_lines = std::mem::take(&mut self.stream.code_block_buffer);

            if let Some(start_idx) = self.stream.code_block_start_idx.take() {
                let stream_end = start_idx + self.stream.code_block_line_count;
                if !code_lines.is_empty() {
                    let code_text = format!("```{}\n{}\n```", lang, code_lines.join("\n"));
                    let (styled, _) = render_markdown_tui(&code_text);
                    let placeholder_count = styled.len().min(MAX_CODE_PREVIEW) + 2;
                    let placeholders: Vec<Line<'static>> =
                        (0..placeholder_count).map(|_| Line::from("")).collect();
                    let raw_placeholders: Vec<String> =
                        (0..placeholder_count).map(|_| String::new()).collect();
                    self.splice_msgs(
                        start_idx..stream_end,
                        placeholders,
                        raw_placeholders,
                        RawMessageType::LLM,
                    );
                    self.code_blocks.push(CodeBlock {
                        start_idx,
                        end_idx: start_idx + placeholder_count,
                        lang,
                        content: code_lines.join("\n"),
                        styled,
                    });
                } else {
                    self.drain_msgs(start_idx..stream_end);
                }
            } else if !code_lines.is_empty() {
                let code_text = format!("```{}\n{}\n```", lang, code_lines.join("\n"));
                let (lines, raw_lines) = render_markdown_tui(&code_text);
                self.extend_msgs(lines, raw_lines, RawMessageType::LLM);
            }
            self.stream.code_block = false;
            self.stream.code_block_line_count = 0;
        }
        // Flush accumulated paragraph (content not yet separated by blank lines, e.g. the last paragraph at stream end)
        if !self.stream.paragraph.is_empty() {
            let paragraph = std::mem::take(&mut self.stream.paragraph);
            let (lines, raw_lines) = render_markdown_tui(&paragraph);
            self.extend_msgs(lines, raw_lines, RawMessageType::LLM);
        }
        // Flush leftover thinking lines and close thinking block
        if !self.thinking.buffer.is_empty() {
            let style = Style::default()
                .fg(Color::Gray)
                .add_modifier(Modifier::ITALIC)
                .bg(Color::Rgb(35, 35, 45));
            let text = if self.thinking.buffer.trim().is_empty() {
                String::new()
            } else {
                format!("│ {}", self.thinking.buffer)
            };
            if !text.is_empty() {
                self.append_msg(
                    Line::from(Span::styled(text.clone(), style)),
                    text,
                    RawMessageType::LLMThinking,
                );
            }
            self.thinking.buffer.clear();
            self.thinking.active_end = Some(self.messages.len() - 1);
        }
        self.close_active_thinking_block();
        if self.stream.buffer.is_empty() {
            return;
        }
        let display = self.stream.buffer.clone();
        let (lines, raw_lines) = render_markdown_tui(&display);
        self.extend_msgs(lines, raw_lines, RawMessageType::LLM);
        self.stream.buffer.clear();
    }

    /// Remove the loading placeholder line if it exists.
    /// Returns true if a loading placeholder was removed.
    pub(crate) fn remove_loading_placeholder(&mut self) -> bool {
        if let Some(idx) = self.loading_idx.take() {
            if idx < self.messages.len() {
                self.remove_msg(idx);
                // Adjust any code_blocks / tool_blocks / thinking blocks that reference
                // indices after the removed line
                for block in &mut self.code_blocks {
                    if block.start_idx > idx {
                        block.start_idx -= 1;
                        block.end_idx -= 1;
                    }
                }
                for block in &mut self.tool_blocks {
                    if block.phys_idx > idx {
                        block.phys_idx -= 1;
                    }
                }
                for block in &mut self.thinking.blocks {
                    if block.title_idx > idx {
                        block.title_idx -= 1;
                        block.end_idx -= 1;
                    }
                }
                if let Some(ref mut start) = self.thinking.active_start {
                    if *start > idx {
                        *start -= 1;
                    }
                }
                if let Some(ref mut end) = self.thinking.active_end {
                    if *end > idx {
                        *end -= 1;
                    }
                }
                if let Some(ref mut start) = self.stream.code_block_start_idx {
                    if *start > idx {
                        *start -= 1;
                    }
                }
            }
            true
        } else {
            false
        }
    }

    /// Ensure the status is `Status::Executing`. If it already is, preserve
    /// current_step and total unchanged. Otherwise transition to Executing
    /// with a fallback total based on the current plan length.
    pub(crate) fn ensure_executing_status(&mut self, _step_idx: usize) {
        if !matches!(self.status, Status::Executing { .. }) {
            self.status = Status::Executing {
                current_step: 0,
                total: self.plan.steps.len(),
            };
        }
    }
}
