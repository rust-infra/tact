use crate::widgets::state::*;
use crate::widgets::tool_widget::ToolPhase;
use arboard::Clipboard;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use ratatui::layout::Rect;
use ratatui::text::Line;

impl App {
    /// Copy text via native clipboard → OSC 52 → internal buffer.
    pub(crate) fn copy_text(&mut self, text: &str) {
        let preview: String = text.chars().take(40).collect();

        if let Ok(mut clip) = Clipboard::new()
            && clip.set_text(text).is_ok()
        {
            let msgs = self.msgs();
            self.add_system_message(msgs.copied_tmpl.replace("{}", &preview));
            return;
        }

        let encoded = BASE64.encode(text);
        let osc52 = format!("\x1b]52;c;{}\x07", encoded);
        if std::io::Write::write_all(&mut std::io::stdout(), osc52.as_bytes()).is_ok() {
            let msgs = self.msgs();
            self.add_system_message(msgs.copied_terminal_tmpl.replace("{}", &preview));
            return;
        }

        self.clipboard_buffer = text.to_string();
        let msgs = self.msgs();
        self.add_system_message(msgs.copied_internal_tmpl.replace("{}", &preview));
    }

    /// True when thinking / tool-diff / code overlay popup is open.
    pub(crate) fn has_overlay_popup(&self) -> bool {
        self.thinking.popup.is_some() || self.tools.popup.is_some() || self.code_popup.is_some()
    }

    fn overlay_scroll_mut(&mut self) -> Option<&mut u16> {
        if let Some(p) = self.thinking.popup.as_mut() {
            Some(&mut p.scroll)
        } else if let Some(p) = self.tools.popup.as_mut() {
            Some(&mut p.scroll)
        } else if let Some(p) = self.code_popup.as_mut() {
            Some(&mut p.scroll)
        } else {
            None
        }
    }

    pub(crate) fn overlay_popup_scroll_up(&mut self) {
        if let Some(scroll) = self.overlay_scroll_mut() {
            *scroll = scroll.saturating_sub(1);
        }
    }

    pub(crate) fn overlay_popup_scroll_down(&mut self) {
        if let Some(scroll) = self.overlay_scroll_mut() {
            *scroll = scroll.saturating_add(1);
        }
    }

    pub(crate) fn close_overlay_popup(&mut self) {
        if self.thinking.popup.is_some() {
            self.close_thinking_popup();
        } else if self.tools.popup.is_some() {
            self.close_diff_popup();
        } else if self.code_popup.is_some() {
            self.close_code_popup();
        }
    }

    /// Close the active overlay if the click is outside its area.
    /// Returns `true` if an overlay was active (click is consumed).
    pub(crate) fn close_overlay_on_outside_click(&mut self, column: u16, row: u16) -> bool {
        let area = if self.thinking.popup.is_some() {
            Some(self.mouse.thinking_popup_area)
        } else if self.tools.popup.is_some() {
            Some(self.mouse.diff_popup_area)
        } else if self.code_popup.is_some() {
            Some(self.mouse.code_popup_area)
        } else {
            None
        };
        let Some(pa) = area else {
            return false;
        };
        if !point_in_rect(column, row, pa) {
            self.close_overlay_popup();
        }
        true
    }

    pub(crate) fn copy_overlay_popup(&mut self) {
        if self.thinking.popup.is_some() {
            self.copy_thinking_popup();
        } else if self.tools.popup.is_some() {
            self.copy_diff_popup();
        } else if self.code_popup.is_some() {
            self.copy_code_popup();
        }
    }

    // Add a blank line as separator to distinguish different input/output blocks in the log.
    pub(crate) fn add_new_line(&mut self) {
        self.append_blank(RawMessageType::LLM);
    }

    /// Append one log row, keeping `messages`, `raw_messages`, and `raw_message_types` in sync.
    pub(crate) fn append_msg(
        &mut self,
        line_msg: Line<'static>,
        raw_msg: String,
        msg_type: RawMessageType,
    ) {
        self.messages.push(line_msg);
        self.raw_messages.push(raw_msg);
        self.raw_message_types.push(msg_type);
    }

    pub(crate) fn append_blank(&mut self, msg_type: RawMessageType) {
        self.append_msg(Line::from(""), String::new(), msg_type);
    }

    pub(crate) fn extend_msgs(
        &mut self,
        lines: Vec<Line<'static>>,
        raw_lines: Vec<String>,
        msg_type: RawMessageType,
    ) {
        debug_assert_eq!(lines.len(), raw_lines.len());
        for (line, raw) in lines.into_iter().zip(raw_lines) {
            self.append_msg(line, raw, msg_type);
        }
    }

    pub(crate) fn insert_msg(
        &mut self,
        idx: usize,
        line_msg: Line<'static>,
        raw_msg: String,
        msg_type: RawMessageType,
    ) {
        self.messages.insert(idx, line_msg);
        self.raw_messages.insert(idx, raw_msg);
        self.raw_message_types.insert(idx, msg_type);
    }

    pub(crate) fn splice_msgs(
        &mut self,
        range: std::ops::Range<usize>,
        lines: Vec<Line<'static>>,
        raw: Vec<String>,
        msg_type: RawMessageType,
    ) {
        debug_assert_eq!(lines.len(), raw.len());
        let n = lines.len();
        self.messages.splice(range.clone(), lines);
        self.raw_messages.splice(range.clone(), raw);
        self.raw_message_types
            .splice(range, std::iter::repeat_n(msg_type, n));
    }

    pub(crate) fn drain_msgs(&mut self, range: std::ops::Range<usize>) {
        self.messages.drain(range.clone());
        self.raw_messages.drain(range.clone());
        self.raw_message_types.drain(range);
    }

    pub(crate) fn remove_msg(&mut self, idx: usize) {
        self.messages.remove(idx);
        self.raw_messages.remove(idx);
        self.raw_message_types.remove(idx);
    }

    /// Sentinel row — rendered as a full-width dashed rule at draw time.
    pub(crate) fn add_task_end_separator(&mut self) {
        self.append_msg(
            Line::default(),
            crate::render::cells::separator::TASK_END_SEPARATOR.to_string(),
            RawMessageType::LLM,
        );
    }

    /// Open the thinking popup, locating the block by its title line index.
    pub(crate) fn open_thinking_popup(&mut self, title_idx: usize) {
        if let Some((bi, block)) = self
            .thinking
            .blocks
            .iter()
            .enumerate()
            .find(|(_, b)| b.title_idx == title_idx)
        {
            let title = self.raw_messages[block.title_idx].clone();
            self.thinking.popup = Some(ThinkingPopup {
                block_idx: bi,
                title,
                scroll: 0,
            });
        }
    }

    /// Close the thinking popup.
    pub(crate) fn close_thinking_popup(&mut self) {
        self.thinking.popup = None;
    }

    /// Find the code block containing the given logical line number.
    /// Returns (logical_start, logical_end) including the opening and closing ``` markers.
    pub(crate) fn find_code_block_containing_logical(
        &self,
        target_logical: usize,
    ) -> Option<(usize, usize)> {
        let mut logical = 0;
        let mut block_start: Option<usize> = None;
        for phys_idx in 0..self.raw_messages.len() {
            if !self.is_message_visible(phys_idx) {
                continue;
            }
            let raw = &self.raw_messages[phys_idx];
            let trimmed = raw.trim();
            if trimmed.starts_with("```") {
                if block_start.is_none() {
                    block_start = Some(logical);
                } else if trimmed == "```" {
                    let start = block_start.unwrap();
                    let end = logical;
                    if target_logical >= start && target_logical <= end {
                        return Some((start, end));
                    }
                    block_start = None;
                }
            }
            logical += 1;
        }
        None
    }

    /// Extract the content of the last complete code block from raw_messages (without ``` markers).
    /// Returns None if no closed code block is found.
    pub(crate) fn extract_last_code_block(&self) -> Option<String> {
        let raw = &self.raw_messages;
        // Search backwards for a closing ```
        let mut end = raw.len();
        loop {
            if end == 0 {
                return None;
            }
            end -= 1;
            if raw[end].trim() == "```" {
                break;
            }
        }
        // Search backwards from the closing ``` for an opening ```lang
        let mut start = end;
        loop {
            if start == 0 {
                return None;
            }
            start -= 1;
            if raw[start].trim_start().starts_with("```") {
                // Extract content lines (excluding opening and closing ``` markers)
                let content: Vec<&str> = raw[start + 1..end].iter().map(|s| s.as_str()).collect();
                return if content.is_empty() {
                    None
                } else {
                    Some(content.join("\n"))
                };
            }
        }
    }

    /// Copy the full content of the current thinking popup to the clipboard.
    pub(crate) fn copy_thinking_popup(&mut self) {
        let popup = match &self.thinking.popup {
            Some(p) => p,
            None => return,
        };
        let block = &self.thinking.blocks[popup.block_idx];
        if block.cached_preview.is_empty() {
            return;
        }
        let text = block.cached_preview.join("\n");
        self.copy_text(&text);
    }

    /// Find tool render output whose block starts at `phys_idx`.
    fn tool_output_at(
        &self,
        phys_idx: usize,
    ) -> Option<&crate::widgets::tool_widget::ToolRenderOutput> {
        self.tools
            .active
            .iter()
            .find(|a| a.phys_idx == phys_idx)
            .map(|a| &a.output)
            .or_else(|| {
                self.tools
                    .blocks
                    .iter()
                    .find(|b| b.phys_idx == phys_idx)
                    .map(|b| &b.output)
            })
    }

    fn popup_from_tool_output(
        &self,
        output: &crate::widgets::tool_widget::ToolRenderOutput,
    ) -> Option<DiffPopup> {
        if !output.layout.has_detail_card {
            return None;
        }
        if output.phase == ToolPhase::Failed {
            let content = output.detail_full.clone()?;
            return Some(DiffPopup {
                title: output
                    .detail_title
                    .clone()
                    .unwrap_or_else(|| output.tool_name.clone()),
                file_path: None,
                git_diff_path: None,
                workspace_dir: None,
                inline_content: Some(content),
                lang: String::new(),
                use_diff_gutter: false,
                is_diff: false,
                scroll: 0,
                selection: None,
                cached_content: None,
                highlighted_lines: Vec::new(),
            });
        }
        match output.tool_name.as_str() {
            "write_file" | "read_file" => Some(DiffPopup {
                title: if output.arg_full.is_empty() {
                    output.arg_summary.clone()
                } else {
                    output.arg_full.clone()
                },
                file_path: Some(if output.arg_full.is_empty() {
                    output.arg_summary.clone()
                } else {
                    output.arg_full.clone()
                }),
                git_diff_path: None,
                workspace_dir: None,
                inline_content: output.detail_full.clone(),
                lang: crate::render::popups::diff_popup::popup_lang_for_path(
                    if output.arg_full.is_empty() {
                        &output.arg_summary
                    } else {
                        &output.arg_full
                    },
                ),
                use_diff_gutter: output.use_diff_gutter,
                is_diff: false,
                scroll: 0,
                selection: None,
                cached_content: None,
                highlighted_lines: Vec::new(),
            }),
            "edit_file" => {
                let path = if output.arg_full.is_empty() {
                    output.arg_summary.clone()
                } else {
                    output.arg_full.clone()
                };
                Some(DiffPopup {
                    title: path.clone(),
                    file_path: None,
                    git_diff_path: Some(path.clone()),
                    workspace_dir: Some(self.work_dir.to_string_lossy().to_string()),
                    inline_content: output.detail_full.clone(),
                    lang: crate::render::popups::diff_popup::popup_lang_for_path(&path),
                    use_diff_gutter: false,
                    is_diff: true,
                    scroll: 0,
                    selection: None,
                    cached_content: None,
                    highlighted_lines: Vec::new(),
                })
            }
            "bash" | "shell" | "run_command" => {
                let content = output.detail_full.clone()?;
                let full_arg = if output.arg_full.is_empty() {
                    output.arg_summary.clone()
                } else {
                    output.arg_full.clone()
                };
                Some(DiffPopup {
                    title: if full_arg.is_empty() {
                        output
                            .detail_title
                            .clone()
                            .unwrap_or_else(|| "Command output".to_string())
                    } else {
                        format!("bash ({full_arg})")
                    },
                    file_path: None,
                    git_diff_path: None,
                    workspace_dir: None,
                    inline_content: Some(if full_arg.is_empty() {
                        content
                    } else {
                        format!("$ {full_arg}\n\n{content}")
                    }),
                    lang: "bash".to_string(),
                    use_diff_gutter: false,
                    is_diff: false,
                    scroll: 0,
                    selection: None,
                    cached_content: None,
                    highlighted_lines: Vec::new(),
                })
            }
            _ => {
                let content = output.detail_full.clone()?;
                Some(DiffPopup {
                    title: output
                        .detail_title
                        .clone()
                        .unwrap_or_else(|| output.tool_name.clone()),
                    file_path: None,
                    git_diff_path: None,
                    workspace_dir: None,
                    inline_content: Some(content),
                    lang: String::new(),
                    use_diff_gutter: false,
                    is_diff: false,
                    scroll: 0,
                    selection: None,
                    cached_content: None,
                    highlighted_lines: Vec::new(),
                })
            }
        }
    }

    /// Open a tool detail popup (file content or command output).
    pub(crate) fn open_diff_popup(&mut self, phys_idx: usize) {
        let Some(output) = self.tool_output_at(phys_idx) else {
            return;
        };
        if let Some(popup) = self.popup_from_tool_output(output) {
            self.tools.popup = Some(popup);
        }
    }

    /// Open a tool detail popup only if the click was inside the detail card area.
    pub(crate) fn open_diff_popup_at_row(&mut self, phys_idx: usize, relative_row: usize) {
        let Some(output) = self.tool_output_at(phys_idx) else {
            return;
        };
        if !output.layout.has_detail_card {
            return;
        }
        let card_height = output.visual_rows(true);
        let total_height = output.visual_rows(false);
        let detail_card_start = total_height - card_height;
        if relative_row < detail_card_start || relative_row >= total_height {
            return;
        }
        self.open_diff_popup(phys_idx);
    }

    /// Close the file content popup.
    pub(crate) fn close_diff_popup(&mut self) {
        self.tools.popup = None;
    }

    /// Copy the popup content to the clipboard.
    pub(crate) fn copy_diff_popup(&mut self) {
        let popup = match &self.tools.popup {
            Some(p) => p,
            None => return,
        };
        let text = if popup.cached_content.is_some() {
            match popup.copy_content() {
                Some(content) => content,
                None => return,
            }
        } else if let Some(path) = &popup.file_path {
            match std::fs::read_to_string(path) {
                Ok(content) => popup.copy_content_from(&content),
                Err(e) => {
                    self.add_system_message(format!("⚠️ Could not read {}: {}", path, e));
                    return;
                }
            }
        } else {
            match popup.copy_content() {
                Some(content) => content,
                None => return,
            }
        };
        self.copy_text(&text);
    }

    // ========== Code Popup ==========

    /// Open the code block popup.
    pub(crate) fn open_code_popup(&mut self, block_idx: usize) {
        if block_idx < self.code_blocks.len() {
            let block = &self.code_blocks[block_idx];
            self.code_popup = Some(CodePopup {
                block_idx,
                lang: block.lang.clone(),
                scroll: 0,
            });
        }
    }

    /// Close the code block popup.
    pub(crate) fn close_code_popup(&mut self) {
        self.code_popup = None;
    }

    /// Copy the popup code content to the clipboard.
    pub(crate) fn copy_code_popup(&mut self) {
        let Some(popup) = &self.code_popup else {
            return;
        };
        let text = self.code_blocks[popup.block_idx].content.clone();
        self.copy_text(&text);
    }
}

fn point_in_rect(column: u16, row: u16, area: Rect) -> bool {
    column >= area.x && column < area.x + area.width && row >= area.y && row < area.y + area.height
}

#[cfg(test)]
mod tests {
    use crate::widgets::state::{DiffPopup, PopupTextSelection};

    fn inline_popup(content: &str) -> DiffPopup {
        DiffPopup {
            title: "test".into(),
            file_path: None,
            git_diff_path: None,
            workspace_dir: None,
            inline_content: Some(content.into()),
            lang: String::new(),
            use_diff_gutter: false,
            is_diff: false,
            scroll: 0,
            selection: None,
            cached_content: None,
            highlighted_lines: Vec::new(),
        }
    }

    #[test]
    fn popup_copy_content_prefers_non_empty_selection() {
        let mut popup = inline_popup("first\nsecond");
        popup.cached_content = Some("first\nsecond".into());
        popup.selection = Some(PopupTextSelection::new(6, 12));

        assert_eq!(popup.copy_content(), Some("second".into()));
    }

    #[test]
    fn popup_copy_content_uses_all_content_for_empty_selection() {
        let mut popup = inline_popup("first\nsecond");
        popup.cached_content = Some("first\nsecond".into());
        popup.selection = Some(PopupTextSelection::new(2, 2));

        assert_eq!(popup.copy_content(), Some("first\nsecond".into()));
    }

    #[test]
    fn popup_copy_content_returns_raw_content_without_presentation_prefixes() {
        let mut popup = inline_popup("first\nsecond");
        popup.selection = Some(PopupTextSelection::new(0, 5));

        assert_eq!(popup.copy_content(), Some("first".into()));
    }
}
