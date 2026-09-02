use arboard::Clipboard;
use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use ratatui::{layout::Rect, style::Color, text::Line};

use crate::widgets::{state::*, tool_widget::ToolPhase};

impl App {
    /// Copy text via native clipboard → OSC 52 → internal buffer.
    pub(crate) fn copy_text(&mut self, text: &str) {
        self.copy_text_inner(text, true);
    }

    /// Copy text without exposing its contents in the system message.
    pub(crate) fn copy_text_without_preview(&mut self, text: &str) {
        self.copy_text_inner(text, false);
    }

    fn copy_text_inner(&mut self, text: &str, include_preview: bool) {
        let preview: String = text.chars().take(40).collect();
        let copied = |template: &str| {
            if include_preview {
                template.replace("{}", &preview)
            } else {
                template.replace(": {}", "")
            }
        };

        if let Ok(mut clip) = Clipboard::new()
            && clip.set_text(text).is_ok()
        {
            let msgs = self.msgs();
            self.add_system_message(copied(msgs.copied_tmpl));
            return;
        }

        let encoded = BASE64.encode(text);
        let osc52 = format!("\x1b]52;c;{}\x07", encoded);
        if std::io::Write::write_all(&mut std::io::stdout(), osc52.as_bytes()).is_ok() {
            let msgs = self.msgs();
            self.add_system_message(copied(msgs.copied_terminal_tmpl));
            return;
        }

        self.clipboard_buffer = text.to_string();
        let msgs = self.msgs();
        self.add_system_message(copied(msgs.copied_internal_tmpl));
    }

    /// True when thinking / tool-diff / code overlay popup is open.
    pub(crate) fn has_overlay_popup(&self) -> bool {
        self.thinking().popup.is_some()
            || self.tools().popup.is_some()
            || self.code_popup.is_some()
            || self.mermaid_popup.is_some()
            || self.task_dag_popup.is_some()
            || self.system_prompt_popup.is_some()
            || self.has_subagent_popup()
    }

    fn overlay_scroll_mut(&mut self) -> Option<&mut u16> {
        // Each `self.X_mut()` borrows all of `self`; the check borrows
        // immutably (ends with the condition), the body borrows mutably.
        if self.thinking().popup.is_some() {
            return Some(&mut self.thinking_mut().popup.as_mut().unwrap().scroll);
        }
        if self.tools().popup.is_some() {
            return Some(&mut self.tools_mut().popup.as_mut().unwrap().scroll);
        }
        if let Some(p) = self.code_popup.as_mut() {
            return Some(&mut p.scroll);
        }
        if let Some(p) = self.mermaid_popup.as_mut() {
            return Some(&mut p.scroll);
        }
        if let Some(p) = self.task_dag_popup.as_mut() {
            return Some(&mut p.scroll);
        }
        if let Some(p) = self.system_prompt_popup.as_mut() {
            return Some(&mut p.scroll);
        }
        if let Some(id) = self.active_subagent_popup.as_ref()
            && let Some(p) = self.subagent_popups.get_mut(id)
        {
            return Some(&mut p.scroll);
        }
        None
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
        if self.thinking_mut().popup.is_some() {
            self.close_thinking_popup();
        } else if self.tools_mut().popup.is_some() {
            self.close_diff_popup();
        } else if self.code_popup.is_some() {
            self.close_code_popup();
        } else if self.mermaid_popup.is_some() {
            self.close_mermaid_popup();
        } else if self.task_dag_popup.is_some() {
            self.task_dag_popup = None;
        } else if self.system_prompt_popup.is_some() {
            self.system_prompt_popup = None;
        } else if self.has_subagent_popup() {
            self.close_subagent_popup();
        }
    }

    /// Close the active overlay if the click is outside its area.
    /// Returns `true` if an overlay was active (click is consumed).
    pub(crate) fn close_overlay_on_outside_click(&mut self, column: u16, row: u16) -> bool {
        let area = if self.thinking_mut().popup.is_some() {
            Some(self.mouse.thinking_popup_area)
        } else if self.tools_mut().popup.is_some() {
            Some(self.mouse.diff_popup_area)
        } else if self.code_popup.is_some() {
            Some(self.mouse.code_popup_area)
        } else if self.mermaid_popup.is_some() {
            Some(self.mouse.mermaid_popup_area)
        } else if self.task_dag_popup.is_some() {
            Some(self.mouse.task_dag_popup_area)
        } else if self.has_subagent_popup() {
            Some(self.mouse.subagent_popup_area)
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
        if self.thinking_mut().popup.is_some() {
            self.copy_thinking_popup();
        } else if self.tools_mut().popup.is_some() {
            self.copy_diff_popup();
        } else if self.code_popup.is_some() {
            self.copy_code_popup();
        } else if self.mermaid_popup.is_some() {
            self.copy_mermaid_popup();
        } else if self.has_subagent_popup() {
            self.copy_subagent_popup();
        } else if self.task_dag_popup.is_some() {
            let src = self
                .task_dag_popup
                .as_ref()
                .map(|p| p.mermaid_source.clone())
                .unwrap_or_default();
            self.copy_text(&src);
        }
    }

    pub(crate) fn open_task_dag_popup(&mut self) {
        let (mermaid_source, lines) = render_task_dag_lines(
            &self.task_panel().snapshot,
            &self.theme,
            DEFAULT_DAG_RENDER_WIDTH,
        );
        self.task_dag_popup = Some(TaskDagPopup {
            lines,
            scroll: 0,
            mermaid_source,
            render_width: DEFAULT_DAG_RENDER_WIDTH,
        });
    }

    /// Open a subagent live-output / markdown-summary popup for a tool card.
    ///
    /// Each subagent tool card owns an independent popup entry (keyed by tool
    /// id) so switching between concurrent subagents preserves each one's
    /// scroll / selection / cached layout. Re-opening a popup re-activates its
    /// existing entry rather than resetting it.
    pub(crate) fn open_subagent_popup(&mut self, phys_idx: usize) {
        let output = match self.tool_output_at(phys_idx) {
            Some(o) if matches!(o.visual_kind, tact_protocol::ToolVisualKind::Subagent) => {
                o.clone()
            }
            _ => return,
        };
        let tool_id = self
            .tools_mut()
            .active
            .iter()
            .find(|a| a.phys_idx == phys_idx)
            .map(|a| a.tool_id.clone())
            .or_else(|| {
                self.tools_mut()
                    .blocks
                    .iter()
                    .find(|b| b.phys_idx == phys_idx)
                    .map(|b| b.tool_id.clone())
            });
        let Some(tool_id) = tool_id else {
            return;
        };
        self.subagent_popups
            .entry(tool_id.clone())
            .or_insert_with(|| crate::widgets::state::SubagentPopup {
                title: output.title_raw.clone(),
                scroll: 0,
                tool_id: tool_id.clone(),
                cached_markdown: None,
                selection: None,
                layout_cache: None,
            });
        self.active_subagent_popup = Some(tool_id);
    }

    /// The currently-visible subagent popup, if any.
    pub(crate) fn subagent_popup(&self) -> Option<&crate::widgets::state::SubagentPopup> {
        self.active_subagent_popup
            .as_ref()
            .and_then(|id| self.subagent_popups.get(id))
    }

    /// Mutable access to the currently-visible subagent popup.
    pub(crate) fn subagent_popup_mut(
        &mut self,
    ) -> Option<&mut crate::widgets::state::SubagentPopup> {
        let id = self.active_subagent_popup.clone()?;
        self.subagent_popups.get_mut(&id)
    }

    /// True when a subagent popup is active (visible).
    pub(crate) fn has_subagent_popup(&self) -> bool {
        self.active_subagent_popup.is_some()
    }

    /// Deactivate the visible subagent popup (keep its entry so re-opening
    /// preserves scroll / selection).
    pub(crate) fn close_subagent_popup(&mut self) {
        self.active_subagent_popup = None;
        self.mouse.subagent_popup_area = Rect::default();
        self.mouse.popup_text_body_area = Rect::default();
        self.mouse.popup_text_hit_rows.clear();
        self.mouse.popup_text_drag_origin = None;
    }

    /// Copy the visible subagent popup content to clipboard.
    pub(crate) fn copy_subagent_popup(&mut self) {
        let Some(popup) = self.subagent_popup() else {
            return;
        };
        // Prefer the text the popup actually laid out (markdown-rendered when
        // completed) so mouse-selection byte offsets stay valid. Precompute
        // the tool-sourced fallback before the closure (a second `&mut self`
        // borrow cannot live inside it).
        let tool_id = popup.tool_id.clone();
        let (live_text, block_text) = {
            let tools = self.tools();
            (
                tools
                    .active
                    .iter()
                    .find(|a| a.tool_id == tool_id)
                    .map(|a| a.live_output.full_detail_text()),
                tools
                    .blocks
                    .iter()
                    .find(|b| b.tool_id == tool_id)
                    .and_then(|b| b.output.detail_full.clone()),
            )
        };
        let full_text = popup
            .layout_cache
            .as_ref()
            .map(|c| c.raw_text.clone())
            .or(live_text)
            .or(block_text)
            .unwrap_or_default();
        if full_text.is_empty() {
            return;
        }
        let text = popup
            .selection
            .and_then(|s| s.normalized_non_empty(&full_text))
            .map(|range| full_text[range].to_string())
            .unwrap_or(full_text);
        self.copy_text(&text);
    }

    // Add a blank line as separator to distinguish different input/output blocks in the log.
    pub(crate) fn add_new_line(&mut self) {
        self.append_blank(LogItemKind::AssistantMarkdown);
    }

    /// Append one log row, keeping all row metadata together in the coordinator.
    pub(crate) fn append_msg(&mut self, line: Line<'static>, raw: String, kind: LogItemKind) {
        self.log.append_msg(line, raw, kind);
    }

    /// Append a whole-Markdown notice as a single log item.
    pub(crate) fn append_markdown(&mut self, content: impl Into<String>) {
        self.append_markdown_with_kind(content, LogItemKind::AssistantMarkdown);
    }

    pub(crate) fn append_system_markdown(&mut self, content: impl Into<String>) {
        self.append_markdown_with_kind(content, LogItemKind::SystemMarkdown);
    }

    pub(crate) fn append_markdown_with_kind(
        &mut self,
        content: impl Into<String>,
        kind: LogItemKind,
    ) {
        self.log.append_markdown(content.into(), &self.theme, kind);
    }

    pub(crate) fn append_blank(&mut self, kind: LogItemKind) {
        self.log.append_blank(kind);
    }

    pub(crate) fn extend_msgs(
        &mut self,
        lines: Vec<Line<'static>>,
        raw_lines: Vec<String>,
        kind: LogItemKind,
    ) {
        self.log.extend_msgs(lines, raw_lines, kind);
    }

    pub(crate) fn insert_msg(
        &mut self,
        idx: usize,
        line: Line<'static>,
        raw: String,
        kind: LogItemKind,
    ) {
        self.log.insert_msg(idx, line, raw, kind);
    }

    pub(crate) fn splice_msgs(
        &mut self,
        range: std::ops::Range<usize>,
        lines: Vec<Line<'static>>,
        raw: Vec<String>,
        kind: LogItemKind,
    ) {
        self.log.splice_msgs(range, lines, raw, kind);
    }

    pub(crate) fn drain_msgs(&mut self, range: std::ops::Range<usize>) {
        self.log.drain_msgs(range);
    }

    pub(crate) fn remove_msg(&mut self, idx: usize) {
        self.log.remove_msg(idx);
    }

    /// Sentinel row — rendered as a full-width rule with frozen elapsed label.
    pub(crate) fn add_task_end_separator(&mut self) {
        let secs = if let Some(start) = self.task_start_time.take() {
            let s = chrono::Local::now()
                .signed_duration_since(start)
                .num_seconds()
                .max(0);
            self.last_prompt_elapsed_secs = Some(s);
            s
        } else {
            self.last_prompt_elapsed_secs.unwrap_or(0)
        };
        self.append_msg(
            Line::default(),
            crate::render::cells::separator::task_end_separator_raw(secs),
            LogItemKind::AssistantMarkdown,
        );
    }

    /// Open the thinking popup for active or completed content at `phys_idx`.
    pub(crate) fn open_thinking_popup(&mut self, phys_idx: usize) {
        let exists = self
            .thinking_mut()
            .active
            .as_ref()
            .is_some_and(|active| active.phys_idx == phys_idx)
            || self
                .thinking_mut()
                .blocks
                .iter()
                .any(|block| block.phys_idx == phys_idx);
        if exists {
            self.thinking_mut().popup = Some(ThinkingPopup {
                phys_idx,
                title: self.msgs().thinking_title.to_string(),
                scroll: 0,
                selection: None,
                selection_text: String::new(),
            });
        }
    }

    /// Close the thinking popup.
    pub(crate) fn close_thinking_popup(&mut self) {
        self.thinking_mut().popup = None;
        self.mouse.thinking_popup_area = Rect::default();
        self.mouse.popup_text_body_area = Rect::default();
        self.mouse.popup_text_hit_rows.clear();
        self.mouse.popup_text_drag_origin = None;
    }

    /// Find the code block containing the given logical line number.
    /// Returns (logical_start, logical_end) including the opening and closing ``` markers.
    pub(crate) fn find_code_block_containing_logical(
        &self,
        target_logical: usize,
    ) -> Option<(usize, usize)> {
        // Code blocks are detected by the code background on the styled
        // spans: the renderer consumes ``` fences at parse time, so the raw
        // rows no longer carry the markers. The hardcoded streamed-code
        // background (Rgb(30,35,50)) is matched too for code cards built by
        // `apply_stream_chunk`.
        let code_bg = self.theme.code_block_bg();
        let mut logical = 0;
        let mut block_start: Option<usize> = None;
        let mut block_end: Option<usize> = None;
        let mut result: Option<(usize, usize)> = None;
        for phys_idx in 0..self.log.items.len() {
            if !self.is_message_visible(phys_idx) {
                continue;
            }
            let is_code =
                self.log.items[phys_idx].line.spans.iter().any(|s| {
                    s.style.bg == Some(code_bg) || s.style.bg == Some(Color::Rgb(30, 35, 50))
                });
            if is_code {
                if block_start.is_none() {
                    block_start = Some(logical);
                }
                block_end = Some(logical);
            } else if let Some(start) = block_start {
                let end = block_end.unwrap_or(start);
                if target_logical >= start && target_logical <= end {
                    result = Some((start, end));
                    break;
                }
                block_start = None;
                block_end = None;
            }
            logical += 1;
        }
        if result.is_none()
            && let Some(start) = block_start
            && let Some(end) = block_end
            && target_logical >= start
            && target_logical <= end
        {
            result = Some((start, end));
        }
        result
    }

    /// Extract the content of the last complete code block from `LogItem::raw` values (without ``` markers).
    /// Returns None if no closed code block is found.
    pub(crate) fn extract_last_code_block(&self) -> Option<String> {
        let raw = self
            .log
            .items
            .iter()
            .map(|item| item.raw.as_str())
            .collect::<Vec<_>>();
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
                let content: Vec<&str> = raw[start + 1..end].to_vec();
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
        let Some(full_content) = self.thinking_popup_content() else {
            return;
        };
        let Some(popup) = self.thinking_mut().popup.as_ref() else {
            return;
        };
        let text = popup.copy_content(&full_content);
        self.copy_text(&text);
    }

    pub(crate) fn thinking_popup_content(&self) -> Option<String> {
        let phys_idx = self.thinking().popup.as_ref()?.phys_idx;
        self.thinking()
            .active
            .as_ref()
            .filter(|active| active.phys_idx == phys_idx)
            .map(|active| active.content.clone())
            .or_else(|| {
                self.thinking()
                    .blocks
                    .iter()
                    .find(|block| block.phys_idx == phys_idx)
                    .map(|block| block.content.clone())
            })
    }

    /// Find tool render output whose block starts at `phys_idx`.
    fn tool_output_at(
        &self,
        phys_idx: usize,
    ) -> Option<&crate::widgets::tool_widget::ToolRenderOutput> {
        self.tools()
            .active
            .iter()
            .find(|a| a.phys_idx == phys_idx)
            .map(|a| &a.output)
            .or_else(|| {
                self.tools()
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
        match output.visual_kind {
            tact_protocol::ToolVisualKind::FileWrite | tact_protocol::ToolVisualKind::FileRead => {
                Some(DiffPopup {
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
                })
            }
            tact_protocol::ToolVisualKind::FileEdit => {
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
            tact_protocol::ToolVisualKind::Command => {
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
                    inline_content: Some(content),
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
            self.tools_mut().popup = Some(popup);
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
        self.tools_mut().popup = None;
        self.mouse.diff_popup_area = Rect::default();
        self.mouse.popup_text_body_area = Rect::default();
        self.mouse.popup_text_hit_rows.clear();
        self.mouse.popup_text_drag_origin = None;
    }

    /// Copy the popup content to the clipboard.
    pub(crate) fn copy_diff_popup(&mut self) {
        // The popup is borrowed immutably for the whole extraction; the read
        // error is deferred so `self` is only mutated after the borrow ends.
        let (text, read_error) = {
            let popup = match &self.tools().popup {
                Some(p) => p,
                None => return,
            };
            if popup.cached_content.is_some() {
                match popup.copy_content() {
                    Some(content) => (content, None),
                    None => return,
                }
            } else if let Some(path) = &popup.file_path {
                match std::fs::read_to_string(path) {
                    Ok(content) => (popup.copy_content_from(&content), None),
                    Err(e) => (
                        String::new(),
                        Some(format!("⚠️ Could not read {}: {}", path, e)),
                    ),
                }
            } else {
                match popup.copy_content() {
                    Some(content) => (content, None),
                    None => return,
                }
            }
        };
        if let Some(msg) = read_error {
            self.add_system_message(msg);
            return;
        }
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

    // ========== Mermaid Popup ==========

    /// Open the Mermaid source popup for a rendered diagram block.
    pub(crate) fn open_mermaid_popup(&mut self, block_idx: usize) {
        if block_idx < self.mermaid_blocks.len()
            && !self.mermaid_blocks[block_idx].source.is_empty()
        {
            self.mermaid_popup = Some(MermaidPopup {
                block_idx,
                scroll: 0,
            });
        }
    }

    /// Close the Mermaid source popup.
    pub(crate) fn close_mermaid_popup(&mut self) {
        self.mermaid_popup = None;
        self.mouse.mermaid_popup_area = Rect::default();
    }

    /// Copy the Mermaid fence body to the clipboard.
    pub(crate) fn copy_mermaid_popup(&mut self) {
        let Some(popup) = &self.mermaid_popup else {
            return;
        };
        if popup.block_idx >= self.mermaid_blocks.len() {
            return;
        }
        let text = self.mermaid_blocks[popup.block_idx].source.clone();
        self.copy_text(&text);
    }
}

fn point_in_rect(column: u16, row: u16, area: Rect) -> bool {
    column >= area.x && column < area.x + area.width && row >= area.y && row < area.y + area.height
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use ratatui::layout::Rect;
    use tact_protocol::{
        AgentUpdate, PlanStep, StepResult, StepStatus, ToolDetailKind, ToolPopupKind,
        ToolPresentationInfo, ToolVisualKind,
    };

    use crate::{
        render::test_harness::make_app,
        widgets::{
            state::{
                App, DiffPopup, PopupHitRow, PopupTextHit, PopupTextSelection, SubagentPopup,
                ThinkingPopup,
            },
            tool_widget::{ToolPhase, ToolWidget},
        },
    };

    fn subagent_presentation() -> ToolPresentationInfo {
        ToolPresentationInfo {
            visual_kind: ToolVisualKind::Subagent,
            display_name: "spawn_subagent".into(),
            keep_full_live_output: true,
            detail: ToolDetailKind::Result,
            popup: ToolPopupKind::SubagentTranscript,
            compact_result_to_meta: false,
            keep_live: false,
        }
    }

    /// Push a finished `spawn_subagent` tool card and return its `phys_idx`.
    fn push_subagent_card(app: &mut App, tool_id: &str) -> usize {
        app.handle_agent_update(AgentUpdate::StepAdded(PlanStep::new(
            "run",
            "spawn_subagent",
            tool_id,
            HashMap::from([("prompt".to_string(), "do it".to_string())]),
        )));
        app.handle_agent_update(AgentUpdate::StepStarted {
            idx: 0,
            tool_id: tool_id.into(),
            tool_name: "spawn_subagent".into(),
            arg_summary: "do it".into(),
            arg_full: "do it".into(),
            presentation: subagent_presentation(),
        });
        app.handle_agent_update(AgentUpdate::StepFinished {
            idx: 0,
            tool_id: tool_id.into(),
            result: StepResult {
                tool: "spawn_subagent".into(),
                arg_summary: "do it".into(),
                arg_full: Some("do it".into()),
                status: StepStatus::Success,
                message: "ok".into(),
                detail: Some(format!("summary for {tool_id}")),
                duration_us: Some(1),
                permission_label: None,
                presentation: subagent_presentation(),
            },
        });
        app.tools_mut().blocks.last().unwrap().phys_idx
    }

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
    fn close_diff_popup_clears_mouse_state_before_reopen() {
        let mut app = make_app();
        app.tools_mut().popup = Some(inline_popup("old"));
        app.mouse.diff_popup_area = Rect::new(5, 5, 20, 10);
        app.mouse.popup_text_body_area = Rect::new(6, 6, 18, 7);
        app.mouse.popup_text_hit_rows = vec![PopupHitRow {
            screen_y: 6,
            text_x: 10,
            line_start: 0,
            line_end: 3,
            cells: vec![PopupTextHit::new(0, 1)],
        }];
        app.mouse.popup_text_drag_origin = Some(PopupTextHit::new(0, 1));

        app.close_diff_popup();
        app.tools_mut().popup = Some(inline_popup("new"));

        assert_eq!(app.mouse.diff_popup_area, Rect::default());
        assert_eq!(app.mouse.popup_text_body_area, Rect::default());
        assert!(app.mouse.popup_text_hit_rows.is_empty());
        assert!(app.mouse.popup_text_drag_origin.is_none());
        assert!(app.tools_mut().popup.as_ref().unwrap().selection.is_none());
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

    fn thinking_popup(selection: Option<PopupTextSelection>) -> ThinkingPopup {
        ThinkingPopup {
            phys_idx: 0,
            title: "thinking".into(),
            scroll: 0,
            selection,
            selection_text: "first\nsecond".into(),
        }
    }

    #[test]
    fn thinking_popup_copy_content_prefers_non_empty_selection() {
        let popup = thinking_popup(Some(PopupTextSelection::new(6, 12)));

        assert_eq!(popup.copy_content("raw **reasoning**"), "second");
    }

    #[test]
    fn thinking_popup_copy_content_uses_full_reasoning_for_empty_selection() {
        let popup = thinking_popup(Some(PopupTextSelection::new(2, 2)));

        assert_eq!(popup.copy_content("raw **reasoning**"), "raw **reasoning**");
    }

    #[test]
    fn close_thinking_popup_clears_selectable_mouse_state() {
        let mut app = make_app();
        app.thinking_mut().popup = Some(thinking_popup(Some(PopupTextSelection::new(0, 5))));
        app.mouse.thinking_popup_area = Rect::new(5, 5, 20, 10);
        app.mouse.popup_text_body_area = Rect::new(6, 6, 18, 7);
        app.mouse.popup_text_hit_rows = vec![PopupHitRow {
            screen_y: 6,
            text_x: 6,
            line_start: 0,
            line_end: 5,
            cells: vec![PopupTextHit::new(0, 1)],
        }];
        app.mouse.popup_text_drag_origin = Some(PopupTextHit::new(0, 1));

        app.close_thinking_popup();

        assert!(app.thinking_mut().popup.is_none());
        assert_eq!(app.mouse.thinking_popup_area, Rect::default());
        assert_eq!(app.mouse.popup_text_body_area, Rect::default());
        assert!(app.mouse.popup_text_hit_rows.is_empty());
        assert!(app.mouse.popup_text_drag_origin.is_none());
    }

    #[test]
    fn bash_popup_and_card_share_the_same_total_line_count() {
        let app = make_app();
        let result = StepResult {
            tool: "bash".into(),
            arg_summary: "echo start".into(),
            arg_full: Some("echo start\necho done".into()),
            status: StepStatus::Success,
            message: "ok".into(),
            detail: Some("output one\noutput two".into()),
            duration_us: Some(1),
            permission_label: None,
            presentation: ToolPresentationInfo::generic("bash"),
        };
        let msgs = app.msgs();
        let output = ToolWidget::from_step_result(&result, &app.theme, &msgs)
            .with_phase(ToolPhase::Success)
            .build();
        let popup = app.popup_from_tool_output(&output).expect("bash popup");

        assert_eq!(
            output.detail_total_lines,
            popup.inline_content.unwrap().lines().count()
        );
    }

    #[test]
    fn subagent_popups_keep_independent_scroll_per_tool_id() {
        let mut app = make_app();
        let phys_a = push_subagent_card(&mut app, "sa-1");
        let phys_b = push_subagent_card(&mut app, "sa-2");

        // Open A, scroll it.
        app.open_subagent_popup(phys_a);
        assert_eq!(app.active_subagent_popup.as_deref(), Some("sa-1"));
        app.subagent_popup_mut().unwrap().scroll = 7;

        // Open B — the map now has two entries and B is active.
        app.open_subagent_popup(phys_b);
        assert_eq!(app.subagent_popups.len(), 2);
        assert_eq!(app.active_subagent_popup.as_deref(), Some("sa-2"));
        assert_eq!(app.subagent_popup().unwrap().tool_id, "sa-2");

        // Re-open A — its scroll is preserved (not reset), because the map
        // keeps one popup entry per tool id.
        app.open_subagent_popup(phys_a);
        assert_eq!(app.active_subagent_popup.as_deref(), Some("sa-1"));
        assert_eq!(app.subagent_popup().unwrap().scroll, 7);

        // Closing deactivates but keeps the entry so re-open still preserves.
        app.close_subagent_popup();
        assert!(app.active_subagent_popup.is_none());
        assert!(!app.has_subagent_popup());
        assert_eq!(app.subagent_popups.len(), 2);
        app.open_subagent_popup(phys_a);
        assert_eq!(app.subagent_popup().unwrap().scroll, 7);
    }

    #[test]
    fn subagent_popup_is_not_in_overlay_set_when_none_active() {
        let mut app = make_app();
        assert!(!app.has_subagent_popup());
        // A stale map entry alone must not report an open overlay.
        app.subagent_popups.insert(
            "sa-1".into(),
            SubagentPopup {
                title: "A".into(),
                scroll: 0,
                tool_id: "sa-1".into(),
                cached_markdown: None,
                selection: None,
                layout_cache: None,
            },
        );
        assert!(!app.has_subagent_popup());
        assert!(!app.has_overlay_popup());
    }
}
