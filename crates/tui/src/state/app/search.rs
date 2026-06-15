use crate::state::*;
use crate::render::render_md::{format_table, is_horizontal_rule, render_markdown_tui};
use ratatui::widgets::{ListState, ScrollbarState};

impl App {
    pub(crate) fn update_search_matches(&mut self) {
        self.search.matches.clear();
        let mut logical_idx = 0;
        for (idx, msg) in self.raw_messages.iter().enumerate() {
            if !self.is_message_visible(idx) {
                continue;
            }
            if msg
                .to_lowercase()
                .contains(&self.search.term.to_lowercase())
            {
                self.search.matches.push(logical_idx);
            }
            logical_idx += 1;
        }
        if !self.stream.buffer.is_empty()
            && self
                .stream
                .buffer
                .to_lowercase()
                .contains(&self.search.term.to_lowercase())
        {
            self.search.matches.push(logical_idx);
        }
        if !self.search.matches.is_empty() {
            self.search.current_match = 0;
            if let Some(&match_idx) = self.search.matches.first() {
                self.log_scroll.offset =
                    (match_idx as u16).saturating_sub(self.log_scroll.height / 2);
            }
        }
    }

    pub(crate) fn jump_to_next_match(&mut self) {
        if self.search.matches.is_empty() {
            return;
        }
        self.search.current_match = (self.search.current_match + 1) % self.search.matches.len();
        let target_line = self.search.matches[self.search.current_match];
        self.log_scroll.offset = (target_line as u16).saturating_sub(self.log_scroll.height / 2);
    }

    pub(crate) fn jump_to_prev_match(&mut self) {
        if self.search.matches.is_empty() {
            return;
        }
        self.search.current_match = if self.search.current_match == 0 {
            self.search.matches.len() - 1
        } else {
            self.search.current_match - 1
        };
        let target_line = self.search.matches[self.search.current_match];
        self.log_scroll.offset = (target_line as u16).saturating_sub(self.log_scroll.height / 2);
    }
}
