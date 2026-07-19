use crate::render::render_md::{format_table, render_markdown_tui};
use crate::render::util::visual_pos_to_byte_offset;
use crate::widgets::state::*;
use crate::widgets::tool_widget::ToolRenderOutput;
use ratatui::text::Line;
use ratatui::widgets::ScrollbarState;

impl App {
    /// Whether the rendered log viewport currently sits at its visual bottom.
    ///
    /// `u16::MAX` is only a pre-render bottom sentinel: `render_log_panel`
    /// clamps it to a real logical offset. Tool progress therefore needs to
    /// recognize both representations before it grows placeholder rows.
    pub(crate) fn is_log_pinned_to_bottom(&self) -> bool {
        if self.log_scroll.offset == u16::MAX {
            return true;
        }
        if self.log_scroll.visible_indices_ver != self.messages.len()
            || self.log_scroll.visual_cache_ver != self.messages.len()
            || self.log_scroll.visual_start_cache.is_empty()
        {
            return false;
        }
        let max_offset = crate::render::effective_max_logical_scroll(
            &self.log_scroll.visual_start_cache,
            self.log_scroll.height as usize,
        );
        self.log_scroll.offset as usize >= max_offset
    }

    pub(crate) fn is_message_visible(&self, idx: usize) -> bool {
        idx < self.messages.len()
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
        // Prefer the per-frame cache built by render_log_panel (O(1)).
        if self.log_scroll.visible_indices_ver == self.messages.len() {
            return self.log_scroll.visible_indices.get(logical_idx).copied();
        }
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

    /// Find the word boundary at the given byte offset in the raw text of a
    /// specific logical line. Returns (word_start_byte, word_end_byte).
    /// Words consist of letters, digits, underscores, and hyphens; other
    /// characters are separators.
    pub(crate) fn find_word_bounds(
        &self,
        logical_idx: usize,
        byte_offset: usize,
    ) -> Option<(usize, usize)> {
        let phys_idx = self.visible_message_index(logical_idx)?;
        let text = self.raw_messages.get(phys_idx)?;
        let bytes = text.as_bytes();
        if bytes.is_empty() {
            return None;
        }
        let byte_pos = text.floor_char_boundary(byte_offset.min(bytes.len()));
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

    /// Compute the byte offset in raw_messages for a given mouse position in the Log panel.
    /// Returns (phys_idx, byte_offset) or None if the position is not inside a physical message.
    pub(crate) fn byte_offset_from_log_position(
        &self,
        logical_idx: usize,
        visual_row: usize,
        col: usize,
    ) -> Option<(usize, usize)> {
        let phys_idx = self.visible_message_index(logical_idx)?;
        let raw_text = self.raw_messages.get(phys_idx)?;
        let wrap_width = self.mouse.log_area.width.saturating_sub(2) as usize;
        let vis_start = self.log_scroll.visual_start.get(logical_idx).copied()?;
        let visual_line_in_row = visual_row.saturating_sub(vis_start);
        let indent = self.nested_log_indent(phys_idx) as usize;
        let text_col = col.saturating_sub(indent);
        let byte_offset =
            visual_pos_to_byte_offset(raw_text, wrap_width, visual_line_in_row, text_col);
        Some((phys_idx, byte_offset))
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

    /// Find an active or completed thinking card containing a logical log row.
    pub(crate) fn find_thinking_at_logical(
        &self,
        line_idx: usize,
    ) -> Option<(usize, usize, usize)> {
        let find = |phys_idx: usize, rows: usize| {
            let logical_start = self.phys_to_logical_fast(phys_idx)?;
            (line_idx >= logical_start && line_idx < logical_start + rows).then_some((
                phys_idx,
                logical_start,
                rows,
            ))
        };
        if let Some(active) = self.thinking.active.as_ref()
            && let Some(found) = find(
                active.phys_idx,
                crate::render::cells::thinking::thinking_visual_rows(active.body_line_count()),
            )
        {
            return Some(found);
        }
        self.thinking.blocks.iter().find_map(|block| {
            find(
                block.phys_idx,
                crate::render::cells::thinking::thinking_visual_rows(1),
            )
        })
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
        let visible_count = if self.log_scroll.visible_indices_ver == self.messages.len() {
            self.log_scroll.visible_indices.len()
        } else {
            (0..self.messages.len())
                .filter(|&idx| self.is_message_visible(idx))
                .count()
        };
        visible_count + if self.stream.buffer.is_empty() { 0 } else { 1 }
    }

    /// Extract the raw text covered by a character-level selection.
    /// Skips collapsed/hidden physical rows so yank matches what the user sees.
    pub(crate) fn extract_selected_text(&self, start: TextPosition, end: TextPosition) -> String {
        if start.phys_idx == end.phys_idx {
            if let Some(text) = self.raw_messages.get(start.phys_idx) {
                return text[start.byte_offset.min(text.len())..end.byte_offset.min(text.len())]
                    .to_string();
            }
            return String::new();
        }
        if start.phys_idx >= end.phys_idx {
            return String::new();
        }
        let mut parts: Vec<&str> = Vec::new();
        if self.is_message_visible(start.phys_idx)
            && let Some(text) = self.raw_messages.get(start.phys_idx)
        {
            parts.push(&text[start.byte_offset.min(text.len())..]);
        }
        for phys in (start.phys_idx + 1)..end.phys_idx {
            if self.is_message_visible(phys)
                && let Some(text) = self.raw_messages.get(phys)
            {
                parts.push(text);
            }
        }
        if self.is_message_visible(end.phys_idx)
            && let Some(text) = self.raw_messages.get(end.phys_idx)
        {
            parts.push(&text[..end.byte_offset.min(text.len())]);
        }
        parts.join("\n")
    }

    /// Finalize the active thinking card at its existing placeholder row.
    pub(crate) fn close_active_thinking_block(&mut self) {
        let Some(active) = self.thinking.active.take() else {
            return;
        };
        let old_rows =
            crate::render::cells::thinking::thinking_visual_rows(active.body_line_count());
        if active.is_blank() {
            let end = active.phys_idx.saturating_add(old_rows);
            if active.phys_idx < self.messages.len() && end <= self.messages.len() {
                self.drain_msgs(active.phys_idx..end);
                self.shift_phys_indices_from(end, -(old_rows as isize));
            }
            return;
        }

        let summary = active
            .content
            .lines()
            .rev()
            .find(|line| !line.trim().is_empty())
            .unwrap_or_default()
            .to_string();
        let (cached_markdown, _) = render_markdown_tui(&active.content, &self.theme);
        let new_rows = crate::render::cells::thinking::thinking_visual_rows(1);
        self.resize_thinking_placeholder_rows(active.phys_idx, old_rows, new_rows);
        self.thinking.blocks.push(ThinkingBlock {
            phys_idx: active.phys_idx,
            content: active.content,
            summary,
            cached_markdown,
            elapsed: active.started_at.elapsed(),
        });
    }

    /// Open a new thinking card at one shared-log placeholder row.
    pub(crate) fn begin_thinking_block(&mut self) {
        if self.thinking.active.is_some() {
            return;
        }
        let phys_idx = self.push_thinking_placeholder_rows(1);
        self.thinking.active = Some(ActiveThinkingBlock::new(
            phys_idx,
            std::time::Instant::now(),
        ));
    }

    /// Append a thinking delta without creating source rows in the shared log.
    pub(crate) fn append_thinking_delta(&mut self, text: &str) {
        let resize = if let Some(active) = self.thinking.active.as_mut() {
            let old_rows =
                crate::render::cells::thinking::thinking_visual_rows(active.body_line_count());
            active.push_delta(text);
            Some((
                active.phys_idx,
                old_rows,
                crate::render::cells::thinking::thinking_visual_rows(active.body_line_count()),
            ))
        } else {
            None
        };
        if let Some((phys_idx, old_rows, new_rows)) = resize {
            self.resize_thinking_placeholder_rows(phys_idx, old_rows, new_rows);
            self.refresh_thinking_log_scroll();
        }
    }

    /// Close active thinking on an explicit finish or compatibility fallback.
    pub(crate) fn flush_and_close_thinking(&mut self) {
        if self.thinking.active.is_some() {
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
                    if block.phys_idx > idx {
                        block.phys_idx -= 1;
                    }
                }
                if let Some(active) = self.thinking.active.as_mut()
                    && active.phys_idx > idx
                {
                    active.phys_idx -= 1;
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

    pub(crate) fn push_thinking_placeholder_rows(&mut self, body_lines: usize) -> usize {
        let phys_idx = self.messages.len();
        for _ in 0..crate::render::cells::thinking::thinking_visual_rows(body_lines) {
            self.append_blank(RawMessageType::LLMThinking);
        }
        phys_idx
    }

    pub(crate) fn resize_thinking_placeholder_rows(
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
                    RawMessageType::LLMThinking,
                );
            }
            self.shift_phys_indices_from(insert_at, (new_rows - old_rows) as isize);
        } else if new_rows < old_rows {
            self.drain_msgs(phys_idx + new_rows..phys_idx + old_rows);
            self.shift_phys_indices_from(phys_idx + new_rows, -((old_rows - new_rows) as isize));
        }
    }

    pub(crate) fn refresh_thinking_log_scroll(&mut self) {
        self.log_scroll.state = ScrollbarState::new(self.total_log_lines().saturating_sub(1));
        if self.input_mode == InputMode::Insert || self.input_mode == InputMode::Normal {
            self.log_scroll.offset = u16::MAX;
        }
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
            if block.phys_idx >= at {
                block.phys_idx = (block.phys_idx as isize + delta).max(0) as usize;
            }
        }
        if let Some(active) = self.thinking.active.as_mut()
            && active.phys_idx >= at
        {
            active.phys_idx = (active.phys_idx as isize + delta).max(0) as usize;
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
    }
}
