use crate::state::*;
use arboard::Clipboard;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use chrono::Local;
use crate::render::render_md::render_markdown_tui;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{ListState, ScrollbarState};
use tact_core::UserCommand;

const CODE_BG: Color = Color::Rgb(30, 35, 50);
const STREAMING_INDICATOR: &str = " ▌";

impl App {
    pub(crate) fn retry_task(&mut self, task: String) {
        self.add_user_message(task.clone());
        self.plan.steps.clear();
        self.plan.collapsed.clear();
        self.plan.selected = 0;
        self.plan.list_state = ListState::default();
        self.plan.scroll_state = ScrollbarState::new(0);
        self.status = Status::Planning;
        let _ = self.user_cmd_tx.send(UserCommand::SubmitTask(task));
        self.show_history = false;
    }

    // Add a blank line as separator to distinguish different input/output blocks in the log.
    pub(crate) fn add_new_line(&mut self) {
        self.messages.push(Line::from(""));
        self.raw_messages.push(String::new());
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

    /// Scroll up within the popup.
    pub(crate) fn thinking_popup_scroll_up(&mut self) {
        if let Some(ref mut popup) = self.thinking.popup {
            popup.scroll = popup.scroll.saturating_sub(1);
        }
    }

    /// Scroll down within the popup (upper bound clamped by actual line count during render).
    pub(crate) fn thinking_popup_scroll_down(&mut self) {
        if let Some(ref mut popup) = self.thinking.popup {
            popup.scroll = popup.scroll.saturating_add(1);
        }
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
        let preview = if text.chars().count() > 40 {
            format!("{}…", text.chars().take(40).collect::<String>())
        } else {
            text.clone()
        };

        // 1. Try native clipboard
        if let Ok(mut clip) = Clipboard::new()
            && clip.set_text(&text).is_ok()
        {
            self.add_system_message(format!("📋 Copied: {}", preview));
            return;
        }

        // 2. Fallback: OSC 52 terminal clipboard
        let encoded = BASE64.encode(&text);
        let osc52 = format!("\x1b]52;c;{}\x07", encoded);
        if std::io::Write::write_all(&mut std::io::stdout(), osc52.as_bytes()).is_ok() {
            self.add_system_message(format!("📋 Copied to terminal clipboard: {}", preview));
            return;
        }

        // 3. Last resort: save to internal buffer
        self.clipboard_buffer = text;
        self.add_system_message(format!(
            "📋 Copied to internal buffer (clipboard unavailable): {}",
            preview
        ));
        self.thinking.popup = None;
    }

    /// Open a file content popup, accepting the diff block's starting line index.
    pub(crate) fn open_diff_popup(&mut self, start_idx: usize) {
        if let Some((_, block)) = self
            .diff_blocks
            .iter()
            .enumerate()
            .find(|(_, b)| b.start_idx == start_idx)
        {
            self.diff_popup = Some(DiffPopup {
                file_path: block.file_path.clone(),
                scroll: 0,
                cached_content: None,
            });
        }
    }

    /// Close the file content popup.
    pub(crate) fn close_diff_popup(&mut self) {
        self.diff_popup = None;
    }

    /// Scroll up within the popup.
    pub(crate) fn diff_popup_scroll_up(&mut self) {
        if let Some(ref mut popup) = self.diff_popup {
            popup.scroll = popup.scroll.saturating_sub(1);
        }
    }

    /// Scroll down within the popup (upper bound clamped by actual line count during render).
    pub(crate) fn diff_popup_scroll_down(&mut self) {
        if let Some(ref mut popup) = self.diff_popup {
            popup.scroll = popup.scroll.saturating_add(1);
        }
    }

    /// Copy the popup file content to the clipboard.
    pub(crate) fn copy_diff_popup(&mut self) {
        let popup = match &self.diff_popup {
            Some(p) => p,
            None => return,
        };
        let path = &popup.file_path;
        let text = match std::fs::read_to_string(path) {
            Ok(content) => content,
            Err(e) => {
                self.add_system_message(format!("⚠️ Could not read {}: {}", path, e));
                return;
            }
        };
        let preview = if text.chars().count() > 40 {
            format!("{}…", text.chars().take(40).collect::<String>())
        } else {
            text.clone()
        };

        if let Ok(mut clip) = Clipboard::new()
            && clip.set_text(&text).is_ok()
        {
            self.add_system_message(format!("📋 Copied: {}", preview));
            return;
        }
        let encoded = BASE64.encode(&text);
        let osc52 = format!("\x1b]52;c;{}\x07", encoded);
        if std::io::Write::write_all(&mut std::io::stdout(), osc52.as_bytes()).is_ok() {
            self.add_system_message(format!("📋 Copied to terminal clipboard: {}", preview));
            return;
        }
        self.clipboard_buffer = text;
        self.add_system_message(format!(
            "📋 Copied to internal buffer (clipboard unavailable): {}",
            preview
        ));
        self.diff_popup = None;
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

    /// Scroll up within the popup.
    pub(crate) fn code_popup_scroll_up(&mut self) {
        if let Some(ref mut popup) = self.code_popup {
            popup.scroll = popup.scroll.saturating_sub(1);
        }
    }

    /// Scroll down within the popup (upper bound clamped by actual line count during render).
    pub(crate) fn code_popup_scroll_down(&mut self) {
        if let Some(ref mut popup) = self.code_popup {
            popup.scroll = popup.scroll.saturating_add(1);
        }
    }

    /// Copy the popup code content to the clipboard.
    pub(crate) fn copy_code_popup(&mut self) {
        let popup = match &self.code_popup {
            Some(p) => p,
            None => return,
        };
        let block = &self.code_blocks[popup.block_idx];
        let text = &block.content;
        let preview = if text.chars().count() > 40 {
            format!("{}…", text.chars().take(40).collect::<String>())
        } else {
            text.clone()
        };

        if let Ok(mut clip) = Clipboard::new()
            && clip.set_text(text).is_ok()
        {
            self.add_system_message(format!("📋 Copied: {}", preview));
            return;
        }
        let encoded = BASE64.encode(text);
        let osc52 = format!("\x1b]52;c;{}\x07", encoded);
        if std::io::Write::write_all(&mut std::io::stdout(), osc52.as_bytes()).is_ok() {
            self.add_system_message(format!("📋 Copied to terminal clipboard: {}", preview));
            return;
        }
        self.clipboard_buffer = text.clone();
        self.add_system_message(format!(
            "📋 Copied to internal buffer (clipboard unavailable): {}",
            preview
        ));
        self.code_popup = None;
    }
}
