use crate::render::render_md::{format_table, render_markdown_tui};
use crate::widgets::state::*;
use crate::widgets::tool_widget::ToolRenderOutput;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::ScrollbarState;

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

    /// Find the tool block (active or completed) containing a logical log line.
    /// Returns `(tool_idx, phys_idx, logical_start, row_count)`.
    pub(crate) fn find_tool_at_logical(
        &self,
        line_idx: usize,
    ) -> Option<(usize, usize, usize, usize)> {
        for (i, active) in self.tools.active.iter().enumerate() {
            let Some(si) = self.phys_to_logical_fast(active.phys_idx) else {
                continue;
            };
            let rows = active.output.visual_rows(false);
            if line_idx >= si && line_idx < si + rows {
                return Some((i, active.phys_idx, si, rows));
            }
        }
        let base = self.tools.active.len();
        for (i, block) in self.tools.blocks.iter().enumerate() {
            let Some(si) = self.phys_to_logical_fast(block.phys_idx) else {
                continue;
            };
            let rows = block.output.visual_rows(false);
            if line_idx >= si && line_idx < si + rows {
                return Some((base + i, block.phys_idx, si, rows));
            }
        }
        None
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
                let scroll_offset = total.saturating_sub(3);

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
                let (styled_lines, _) = render_markdown_tui(&raw_content, &self.theme);

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

    fn last_visible_phys_idx(&self) -> Option<usize> {
        (0..self.messages.len())
            .rev()
            .find(|&idx| self.is_message_visible(idx))
    }

    fn phys_idx_in_tool_block(&self, phys: usize) -> bool {
        self.tools.active.iter().any(|active| {
            phys >= active.phys_idx
                && phys <= active.phys_idx + active.output.message_placeholder_rows()
        }) || self.tools.blocks.iter().any(|block| {
            phys >= block.phys_idx
                && phys <= block.phys_idx + block.output.message_placeholder_rows()
        })
    }

    /// Blank line before assistant stream content when it follows a tool card.
    pub(crate) fn ensure_gap_after_tools(&mut self) {
        let Some(phys) = self.last_visible_phys_idx() else {
            return;
        };
        if !self.phys_idx_in_tool_block(phys) {
            return;
        }
        self.append_blank(RawMessageType::LLM);
    }

    /// Blank line before a tool block when it follows normal content.
    pub(crate) fn ensure_gap_before_tools(&mut self) {
        let Some(phys) = self.last_visible_phys_idx() else {
            return;
        };
        if self.phys_idx_in_tool_block(phys) {
            return;
        }
        if self
            .raw_messages
            .get(phys)
            .is_some_and(|line| line.is_empty())
        {
            return;
        }
        self.append_blank(RawMessageType::SysTool);
    }

    /// Flush residual content from the streaming buffer into the message list.
    pub(crate) fn flush_stream_pending(&mut self) {
        let will_flush_llm = !self.stream.table_buffer.is_empty()
            || self.stream.code_block
            || !self.stream.paragraph.is_empty()
            || !self.stream.buffer.is_empty();
        if will_flush_llm {
            self.ensure_gap_after_tools();
        }
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
                    let (styled, _) = render_markdown_tui(&code_text, &self.theme);
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
                let (lines, raw_lines) = render_markdown_tui(&code_text, &self.theme);
                self.extend_msgs(lines, raw_lines, RawMessageType::LLM);
            }
            self.stream.code_block = false;
            self.stream.code_block_line_count = 0;
        }
        // Flush accumulated paragraph (content not yet separated by blank lines, e.g. the last paragraph at stream end)
        if !self.stream.paragraph.is_empty() {
            let paragraph = std::mem::take(&mut self.stream.paragraph);
            let (lines, raw_lines) = render_markdown_tui(&paragraph, &self.theme);
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
        let (lines, raw_lines) = render_markdown_tui(&display, &self.theme);
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
                for block in &mut self.tools.blocks {
                    if block.phys_idx > idx {
                        block.phys_idx -= 1;
                    }
                }
                for active in &mut self.tools.active {
                    if active.phys_idx > idx {
                        active.phys_idx -= 1;
                    }
                }
                for block in &mut self.thinking.blocks {
                    if block.title_idx > idx {
                        block.title_idx -= 1;
                        block.end_idx -= 1;
                    }
                }
                if let Some(ref mut start) = self.thinking.active_start
                    && *start > idx
                {
                    *start -= 1;
                }
                if let Some(ref mut end) = self.thinking.active_end
                    && *end > idx
                {
                    *end -= 1;
                }
                if let Some(ref mut start) = self.stream.code_block_start_idx
                    && *start > idx
                {
                    *start -= 1;
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

    pub(crate) fn push_tool_placeholder_rows(&mut self, output: &ToolRenderOutput) -> usize {
        self.ensure_gap_before_tools();
        let phys_idx = self.messages.len();
        let rows = output.visual_rows(false);
        for _ in 0..rows {
            self.append_blank(RawMessageType::SysTool);
        }
        phys_idx
    }

    fn shift_phys_indices_from(&mut self, at: usize, delta: isize) {
        if delta == 0 {
            return;
        }
        let adjust = |idx: &mut usize| {
            if *idx >= at {
                *idx = (*idx as isize + delta).max(0) as usize;
            }
        };
        for block in &mut self.tools.blocks {
            adjust(&mut block.phys_idx);
        }
        for active in &mut self.tools.active {
            adjust(&mut active.phys_idx);
        }
        for block in &mut self.code_blocks {
            if block.start_idx >= at {
                block.start_idx = (block.start_idx as isize + delta).max(0) as usize;
                block.end_idx = (block.end_idx as isize + delta).max(0) as usize;
            }
        }
        for block in &mut self.thinking.blocks {
            if block.title_idx >= at {
                block.title_idx = (block.title_idx as isize + delta).max(0) as usize;
                block.end_idx = (block.end_idx as isize + delta).max(0) as usize;
            }
        }
        if let Some(ref mut start) = self.thinking.active_start
            && *start >= at
        {
            *start = (*start as isize + delta).max(0) as usize;
        }
        if let Some(ref mut end) = self.thinking.active_end
            && *end >= at
        {
            *end = (*end as isize + delta).max(0) as usize;
        }
        if let Some(ref mut idx) = self.loading_idx
            && *idx >= at
        {
            *idx = (*idx as isize + delta).max(0) as usize;
        }
        if let Some(ref mut start) = self.stream.code_block_start_idx
            && *start >= at
        {
            *start = (*start as isize + delta).max(0) as usize;
        }
    }

    fn remove_active_tool_rows(&mut self, active: ActiveToolBlock) {
        let rows = active.output.visual_rows(false);
        if rows == 0 {
            return;
        }
        let end = active.phys_idx.saturating_add(rows);
        if active.phys_idx < self.messages.len() && end <= self.messages.len() {
            self.drain_msgs(active.phys_idx..end);
            self.shift_phys_indices_from(end, -(rows as isize));
        }
    }

    /// Drop a running tool block and remove its placeholder rows from the log.
    pub(crate) fn cancel_active_tool(&mut self, tool_id: &str) {
        let Some(pos) = self.tools.active.iter().position(|a| a.tool_id == tool_id) else {
            return;
        };
        let active = self.tools.active.remove(pos);
        self.remove_active_tool_rows(active);
    }

    /// Drop all running tool blocks (e.g. when a new plan starts).
    pub(crate) fn cancel_all_active_tools(&mut self) {
        let mut actives = std::mem::take(&mut self.tools.active);
        actives.sort_by_key(|a| std::cmp::Reverse(a.phys_idx));
        for active in actives {
            self.remove_active_tool_rows(active);
        }
    }

    pub(crate) fn resize_tool_placeholder_rows(
        &mut self,
        phys_idx: usize,
        old_rows: usize,
        new_rows: usize,
    ) {
        if new_rows > old_rows {
            let insert_at = phys_idx + old_rows;
            for _ in 0..(new_rows - old_rows) {
                self.insert_msg(
                    insert_at,
                    Line::from(""),
                    String::new(),
                    RawMessageType::SysTool,
                );
            }
            self.shift_phys_indices_from(insert_at, (new_rows - old_rows) as isize);
        } else if new_rows < old_rows {
            self.drain_msgs(phys_idx + new_rows..phys_idx + old_rows);
            self.shift_phys_indices_from(phys_idx + new_rows, -((old_rows - new_rows) as isize));
        }
    }

    pub(crate) fn finalize_tool_block(&mut self, tool_id: &str, output: ToolRenderOutput) {
        if let Some(pos) = self.tools.active.iter().position(|a| a.tool_id == tool_id) {
            let active = self.tools.active.remove(pos);
            let old_rows = active.output.visual_rows(false);
            let new_rows = output.visual_rows(false);
            self.resize_tool_placeholder_rows(active.phys_idx, old_rows, new_rows);
            self.tools.blocks.push(ToolBlock {
                phys_idx: active.phys_idx,
                output,
            });
        } else {
            let phys_idx = self.push_tool_placeholder_rows(&output);
            self.tools.blocks.push(ToolBlock { phys_idx, output });
        }
        self.refresh_tool_log_scroll();
    }

    pub(crate) fn refresh_tool_log_scroll(&mut self) {
        self.log_scroll.state = ScrollbarState::new(self.total_log_lines().saturating_sub(1));
        if self.input_mode == InputMode::Insert || self.input_mode == InputMode::Normal {
            self.log_scroll.offset = u16::MAX;
        }
        if !self.search.term.is_empty() {
            self.update_search_matches();
        }
    }
}
