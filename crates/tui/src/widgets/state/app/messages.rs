use chrono::Local;
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};
use tact_llm::content::{ContentBlock, Message, MessageContent, Role};

use tact::hook::{hook_context_body, hook_context_source, is_hook_context_text};

use agent_tui_kit::widgets::button::{Button, ButtonTheme, ButtonVariant};
use agent_tui_kit::widgets::tool_widget::collapsed_action_text;

use agent_tui_kit::render::cells::markdown::Gutter;

use crate::{
    i18n::{Language, Messages},
    render::cells::separator::is_task_end_separator,
    widgets::state::*,
};

/// The bar down the left edge of a hook-injected block: the glyph plus the
/// space that separates it from the content it marks.
const HOOK_CONTEXT_GUTTER: &str = "▎ ";

/// Prefixes a hook block's header row. A provenance marker, not a status glyph.
const HOOK_CONTEXT_MARK: &str = "⌁";

/// Source lines of a hook block kept inline before the rest moves to the popup.
const HOOK_CONTEXT_INLINE_LINES: usize = 8;

/// Cut a hook body to `max_lines`, closing any fence the cut ran through.
///
/// Returns the head and the number of source lines dropped. Cutting inside a
/// fenced block would leave the fence open, and the Markdown renderer would
/// then swallow every row after it into the code block — so an odd number of
/// fence markers in the head gets a closing one appended.
fn truncate_hook_body(body: &str, max_lines: usize) -> (String, usize) {
    let lines: Vec<&str> = body.lines().collect();
    if lines.len() <= max_lines {
        return (body.to_string(), 0);
    }
    let head = &lines[..max_lines];
    let fences = head
        .iter()
        .filter(|line| line.trim_start().starts_with("```"))
        .count();
    let mut kept = head.join("\n");
    if fences % 2 == 1 {
        kept.push_str("\n```");
    }
    (kept, lines.len() - max_lines)
}

/// How long a hook took, at the precision a reader can use: milliseconds under
/// a second, one decimal above it.
fn format_hook_elapsed(ms: u64) -> String {
    if ms < 1000 {
        format!("{ms}ms")
    } else {
        format!("{:.1}s", ms as f64 / 1000.0)
    }
}

/// True for a cell carrying hook-injected context (`<hook-context>`) instead
/// of a user turn.
///
/// Accepts both signals: the in-memory `MessageKind::HookContext` set at
/// injection time, and the marker text — which is all that survives a reload
/// from disk, since the kind is never serialized.
fn is_hook_context_cell(message: &Message) -> bool {
    message.is_hook_context()
        || matches!(&message.content, MessageContent::Text { content } if is_hook_context_text(content))
}

/// Copy affordances written by older versions (`[copy]` / `[复制]`). Rows they
/// persisted are re-rendered from `raw`, so they must stay both recognizable
/// ([`is_task_stats_line`]) and clickable ([`find_task_stats_copy_button`]).
const LEGACY_TASK_STATS_COPY_BTNS: [&str; 2] = ["[copy]", "[复制]"];

/// Every copy affordance a task-stats row can carry: the current icon, then the
/// legacy labels. The icon is locale-independent, hence `Language::English` —
/// one entry, not one per language.
fn task_stats_copy_buttons() -> impl Iterator<Item = &'static str> {
    std::iter::once(Messages::by_language(Language::English).task_stats_copy_btn)
        .chain(LEGACY_TASK_STATS_COPY_BTNS)
}

/// Returns true for a per-turn stats row in any supported language, including
/// the legacy `📊 任务统计：` rows persisted before the icon was removed (old
/// sessions still need the copy affordance to keep working).
pub(crate) fn is_task_stats_line(raw: &str) -> bool {
    // The copy affordance renders before the stats body; strip it (any
    // version) before matching the prefix. Rows without a leading button
    // (the legacy format) are matched as-is.
    let body = task_stats_copy_buttons()
        .find_map(|btn| raw.strip_prefix(btn).map(str::trim_start))
        .unwrap_or(raw);
    Language::all()
        .iter()
        .any(|lang| body.starts_with(Messages::by_language(*lang).task_stats_prefix))
        || body.starts_with("📊 任务统计：")
}

/// Byte range `(start, end)` of the clickable copy affordance in a task-stats
/// raw row, or `None` when no button glyph is present.
///
/// The earliest match wins: the affordance leads the row today, while rows
/// persisted before the icon carried their label at the end.
pub(crate) fn find_task_stats_copy_button(raw: &str) -> Option<(usize, usize)> {
    task_stats_copy_buttons()
        .filter_map(|btn| raw.find(btn).map(|start| (start, start + btn.len())))
        .min_by_key(|(start, _)| *start)
}

impl App {
    /// The startup banner: the logo block, then the welcome and mode hints,
    /// closed by the blank rows that keep them apart from everything else.
    ///
    /// The log is one scrolling column, so separation has to be written as
    /// rows. With a single trailing row the first thing to land after startup
    /// sat flush against "Current mode: …" — and that first thing is routinely
    /// another system line (a `/theme` or `/model` write, a plugin's
    /// `SessionStart` briefing, the restored session history), which then read
    /// as one more line of the banner.
    pub(crate) fn add_startup_banner(&mut self) {
        self.add_startup_logo();
        let msgs = self.msgs();
        self.add_system_message(msgs.startup_welcome.to_string());
        self.add_system_message(msgs.startup_mode_hint.to_string());
        self.add_new_line();
        self.add_new_line();
    }

    pub(crate) fn add_startup_logo(&mut self) {
        let logo = [
            "  ████████╗ ",
            "  ╚══██╔══╝ ",
            "     ██║    ",
            "     ██║    ",
            "     ██║    ",
            "     ╚═╝    ",
        ];

        // Gradient: use accent color and increase brightness for each line
        let accent = self.theme.accent;
        let line_colors = match accent {
            Color::Rgb(r, g, b) => {
                let step = 15u8;
                [
                    Color::Rgb(r.saturating_sub(step * 2), g.saturating_sub(step * 2), b),
                    Color::Rgb(r.saturating_sub(step), g.saturating_sub(step), b),
                    Color::Rgb(r, g, b),
                    Color::Rgb(r.saturating_add(step / 2), g.saturating_add(step / 2), b),
                    Color::Rgb(
                        r.saturating_add(step),
                        g.saturating_add(step),
                        b.saturating_add(step / 2),
                    ),
                    Color::Rgb(
                        r.saturating_add(step * 2),
                        g.saturating_add(step * 2),
                        b.saturating_add(step),
                    ),
                ]
            }
            _ => [
                Color::Green,
                Color::LightGreen,
                Color::Green,
                Color::LightGreen,
                Color::Green,
                Color::LightGreen,
            ],
        };

        self.add_new_line();
        for (i, line) in logo.iter().enumerate() {
            let color = line_colors[i.min(line_colors.len() - 1)];
            self.append_msg(
                Line::from(Span::styled(
                    (*line).to_string(),
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                )),
                (*line).to_string(),
                LogItemKind::SystemPlain(SystemMsgStyle::Default),
            );
        }

        let title = "  Tact Agent".to_string();
        self.append_msg(
            Line::from(Span::styled(
                title.clone(),
                Style::default()
                    .fg(self.theme.accent)
                    .add_modifier(Modifier::BOLD),
            )),
            title,
            LogItemKind::SystemPlain(SystemMsgStyle::Default),
        );

        // Random startup quote
        let quotes = self.msgs().startup_quotes;
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let idx = (seed as usize) % quotes.len();
        let tagline = quotes[idx];
        self.append_msg(
            Line::from(Span::styled(
                tagline.to_string(),
                Style::default()
                    .fg(self.theme.muted_fg())
                    .add_modifier(Modifier::ITALIC),
            )),
            tagline.to_string(),
            LogItemKind::SystemPlain(SystemMsgStyle::Default),
        );
        // Two rows, so the banner reads as its own block rather than as the
        // first three lines of the welcome text.
        self.add_new_line();
        self.add_new_line();
    }

    /// Load persisted session messages into the Log area.
    /// Converts stored `Message` objects into display cells or lines.
    /// Only `Text` blocks are rendered; `Thinking`, `ToolUse`, `ToolResult`,
    /// and `Image` blocks are skipped.
    pub(crate) fn load_history(&mut self, messages: Vec<Message>) {
        for msg in messages {
            let blocks: Vec<&ContentBlock> = match &msg.content {
                MessageContent::Blocks { content } => content.iter().collect(),
                MessageContent::Text { content } => {
                    if content.trim().is_empty() {
                        continue;
                    }
                    match msg.role {
                        Role::User => {
                            if is_hook_context_cell(&msg) {
                                self.append_hook_context_markdown(
                                    hook_context_source(content),
                                    hook_context_body(content),
                                );
                                continue;
                            }
                            // Seed the session turn counter: persisted user
                            // messages are the only durable record of past
                            // turns (no dedicated store query needed).
                            self.status_bar_mut().turn_user += 1;
                            self.add_user_message(content.clone());
                        }
                        Role::Assistant => self.append_markdown(content.clone()),
                    }
                    continue;
                }
            };

            match msg.role {
                Role::User => {
                    let texts: Vec<&str> = blocks
                        .iter()
                        .filter_map(|b| match b {
                            ContentBlock::Text { text } => Some(text.as_str()),
                            _ => None,
                        })
                        .collect();
                    if texts.is_empty() {
                        continue;
                    }
                    // Seed the session turn counter (see the Text branch above).
                    self.status_bar_mut().turn_user += 1;
                    self.add_user_message(texts.join("\n"));
                }
                Role::Assistant => {
                    let has_text = blocks
                        .iter()
                        .any(|b| matches!(b, ContentBlock::Text { .. }));
                    if !has_text {
                        continue;
                    }
                    self.add_new_line();
                    for block in &blocks {
                        if let ContentBlock::Text { text } = block {
                            self.append_markdown(text.clone());
                        }
                    }
                }
            }
        }
    }

    /// Save current input state to undo stack and clear redo stack. Max 100 snapshots retained.
    pub(crate) fn save_undo(&mut self) {
        self.redo_stack.clear();
        self.undo_stack
            .push((self.input.clone(), self.input_cursor));
        if self.undo_stack.len() > 100 {
            self.undo_stack.remove(0);
        }
    }

    /// Append a plain system message with explicit provenance.
    ///
    /// System-ness comes from this API's caller, never from indentation or
    /// arbitrary content. Explicit marker prefixes only choose the visual
    /// system color for each line.
    pub(crate) fn add_system_message(&mut self, content: String) {
        let theme = self.theme;
        for line in content.split('\n') {
            let style = SystemMsgStyle::from_marker(line).unwrap_or(SystemMsgStyle::Default);
            self.append_msg(
                Line::from(Span::styled(
                    line.to_string(),
                    Style::default().fg(style.color(&theme)),
                )),
                line.to_string(),
                LogItemKind::SystemPlain(style),
            );
        }
        self.scroll_after_message();
    }

    fn scroll_after_message(&mut self) {
        if self.input_mode == InputMode::Insert || self.input_mode == InputMode::Normal {
            // usize::MAX is correctly clipped by render_log_panel based on visual line count
            self.scroll_log_to_bottom();
        }
    }

    /// Show, or finish, a plugin hook's progress line.
    ///
    /// The row is keyed by the id the agent handed out and **rewritten in
    /// place** on completion: removing it would shift every physical index the
    /// selection, the cards and the scroll anchors are keyed on. The plugin's
    /// own wording survives both states — what changes is the styling (accent
    /// while it runs, muted once it is a record) and the measured time, so the
    /// line stops claiming to be loading without the reader losing which hook
    /// it was.
    pub(crate) fn apply_hook_status(
        &mut self,
        id: u64,
        source: Option<&str>,
        message: &str,
        elapsed_ms: Option<u64>,
    ) {
        // Gated here rather than in the renderer: a row that is never appended
        // cannot take a physical index, and the log's indices are the key for
        // the selection, the cards and the scroll anchors. Returning before
        // both halves of the pair is what keeps a *completion* from appending
        // the row its opening deliberately skipped.
        if !self.hook_output {
            return;
        }
        let (line, raw) = self.hook_status_line(source, message, elapsed_ms);
        let kind = LogItemKind::HookStatus(id);
        let gutter = self.hook_gutter(elapsed_ms.is_none());
        match self.log.items.iter().position(|item| item.kind == kind) {
            Some(idx) => {
                self.log.items[idx] = LogItem::new(line, raw, kind).with_gutter(gutter);
                // The row count did not change, and the cache's validity token
                // is the count — so it has to be moved off `items.len()` by
                // hand or the old text would be reused verbatim.
                self.log_scroll.visual_cache_ver = usize::MAX;
            }
            None => self.log.append_bared_msg(line, raw, kind, gutter),
        }
    }

    /// The bar a hook row wears, in the colour its state calls for.
    ///
    /// Running wears the accent the context block will wear; finished drops to
    /// muted, which is what tells the reader the wait is over.
    fn hook_gutter(&self, running: bool) -> Gutter {
        Gutter {
            glyph: HOOK_CONTEXT_GUTTER,
            color: if running {
                self.theme.accent
            } else {
                self.theme.muted_fg()
            },
        }
    }

    /// One progress line: `▎ ⌁ hook · <source> · <message> · <elapsed>`.
    fn hook_status_line(
        &self,
        source: Option<&str>,
        message: &str,
        elapsed_ms: Option<u64>,
    ) -> (Line<'static>, String) {
        let label = self.msgs().hook_status_label;
        // The bar is the row's gutter (see `LogItem::gutter`), so it is not
        // part of this text: a `▎` typed in here would mark only the first
        // visual row of a wrapped line.
        let (mark, text) = match elapsed_ms {
            None => (self.theme.accent, self.theme.fg),
            Some(_) => (self.theme.muted_fg(), self.theme.muted_fg()),
        };
        let mut head = label.to_string();
        if let Some(source) = source {
            head.push_str(&format!(" · {source}"));
        }
        let mut spans = vec![
            Span::styled(
                format!("{HOOK_CONTEXT_MARK} {head}"),
                Style::default().fg(mark),
            ),
            Span::styled(format!(" · {message}"), Style::default().fg(text)),
        ];
        let mut raw = format!("{HOOK_CONTEXT_MARK} {head} · {message}");
        if let Some(ms) = elapsed_ms {
            let elapsed = format_hook_elapsed(ms);
            spans.push(Span::styled(
                format!(" · {elapsed}"),
                Style::default().fg(self.theme.muted_fg()),
            ));
            raw.push_str(&format!(" · {elapsed}"));
        }
        (Line::from(spans), raw)
    }

    /// Append hook-injected context: a labelled header row, then the head of
    /// the body behind a left bar, with the whole text available from Open.
    ///
    /// The body renders through the same Markdown pipeline as
    /// [`Self::append_system_markdown`] — the caller has already stripped the
    /// `<hook-context>` framing — which is exactly why the block needs a shape
    /// of its own: without it a hook's stdout and a markdown notice Tact wrote
    /// itself are the same rows. The header names the block and says how to
    /// open it, the bar holds what is shown together (blank rows included), and
    /// the tail is cut at a paragraph-safe boundary so a briefing does not push
    /// the conversation off the log — the reader's full copy stays in the popup.
    ///
    /// The header carries **no** line count. It sits at the tool block's indent
    /// (`LOG_TOOL_BLOCK_INDENT`), so the four fields it could name — block,
    /// source, size, gesture — have to fit in what is left of a narrow panel,
    /// and the size is the one that is already stated elsewhere: the tail row
    /// gives the count that was *hidden* (`… 9 more lines`), and a block that
    /// hid nothing is entirely on screen. Keeping it cost eleven columns and
    /// wrapped the header at 60 columns wide.
    pub(crate) fn append_hook_context_markdown(&mut self, source: Option<&str>, body: &str) {
        // Display-only switch, applied at the point the rows would enter the
        // log: the agent has already injected this text into the conversation,
        // and a row that is never appended cannot take an index. See
        // `App::hook_output`.
        if !self.hook_output {
            return;
        }
        let label = self.msgs().hook_context_label;
        let bar = HOOK_CONTEXT_GUTTER;
        // The bar is the row's gutter, not the first span of its text: a `▎`
        // typed into the text marks only the first visual row, so a header that
        // wraps would leave its continuation outside the rail. See
        // `LogItem::gutter`.
        let gutter = Gutter {
            glyph: bar,
            color: self.theme.accent,
        };
        let mut spans = vec![Span::styled(
            format!("{HOOK_CONTEXT_MARK} {label}"),
            Style::default()
                .fg(self.theme.accent)
                .add_modifier(Modifier::BOLD),
        )];
        let mut header = format!("{HOOK_CONTEXT_MARK} {label}");
        if let Some(source) = source {
            spans.push(Span::styled(
                format!(" · {source}"),
                Style::default().fg(self.theme.muted_fg()),
            ));
            header.push_str(&format!(" · {source}"));
        }
        // No expand hint on the header: the tail row below carries the one
        // affordance, spelled the way the log spells it everywhere else
        // (`[Open]`, the same glyphs a collapsed tool block's meta row draws).
        // Two spellings of one gesture on one block read as two gestures.

        self.log.append_bared_msg_with_popup(
            Line::from(spans),
            header,
            LogItemKind::HookContext,
            body.to_string(),
            gutter,
        );

        let (head, dropped) = truncate_hook_body(body, HOOK_CONTEXT_INLINE_LINES);
        self.log
            .append_markdown_with_gutter(head, &self.theme, LogItemKind::HookContext, gutter);
        if dropped > 0 {
            // `collapsed_action_text` is the shared definition of the bracketed
            // action — the tool block's meta row draws the same string through
            // `ButtonChrome::Brackets`, so the two `[Open]`s cannot drift.
            let more = self
                .msgs()
                .hook_context_more_tmpl
                .replacen("{}", &dropped.to_string(), 1)
                .replacen("{}", &collapsed_action_text(&self.msgs()), 1);
            self.log.append_bared_msg_with_popup(
                Line::from(Span::styled(
                    more.clone(),
                    Style::default().fg(self.theme.muted_fg()),
                )),
                more,
                LogItemKind::HookContext,
                body.to_string(),
                gutter,
            );
        }
    }

    /// Append a task-completion stats block right after the task-end separator.
    ///
    /// Reads the already-frozen `last_prompt_elapsed_secs` and the status-bar
    /// token/model snapshots; deliberately adds no new state (YAGNI — the data
    /// is already collected by `add_task_end_separator` / `TokenUsage` /
    /// `ModelInfo` updates). The body comes from
    /// `agent_tui_kit::render::stats_line::task_stats_body` — the same builder
    /// the kit draws live on the Log's last row while the task runs, so the row
    /// the reader watched and the row frozen here cannot drift.
    ///
    /// The leading copy affordance (`⎘`, an icon so it needs no translation)
    /// copies this turn's log text — from the previous stats row, or session
    /// start, up to but not including this stats row.
    pub(crate) fn add_task_stats_block(&mut self) {
        let secs = self.last_prompt_elapsed_secs.unwrap_or(0).max(0);
        let msgs = self.msgs();
        let body = agent_tui_kit::render::stats_line::task_stats_body(
            &msgs,
            secs,
            self.status_bar().state(),
        );
        // The copy affordance is drawn by the shared button component; keep
        // `raw` glyph-for-glyph identical to the line so the byte-based
        // selection and click mapping stay aligned.
        let button = Button::new(
            msgs.task_stats_copy_btn,
            ButtonTheme::from_theme(&self.theme),
        )
        .variant(ButtonVariant::Primary)
        .horizontal_padding(0)
        .modifier(Modifier::BOLD | Modifier::UNDERLINED);
        let raw = format!("{}  {body}", button.label());
        let mut spans = button.line().spans;
        spans.push(Span::raw("  "));
        spans.push(Span::styled(body, Style::default().fg(self.theme.accent)));
        let line = Line::from(spans);
        // `append_msg` applies follow-the-tail: this row lands where the live
        // band was, so a reader at the bottom stays put and one who scrolled
        // away keeps their place.
        self.append_msg(line, raw, LogItemKind::SystemPlain(SystemMsgStyle::Default));
    }

    /// Copy the turn that ends at the given task-stats physical row.
    ///
    /// Range: after the previous stats line (or session start) .. `stats_phys`
    /// (exclusive). Skips blank rows, task-end separators, and other stats rows.
    pub(crate) fn copy_turn_ending_at_stats(&mut self, stats_phys: usize) {
        let Some(item) = self.log.items.get(stats_phys) else {
            return;
        };
        if !is_task_stats_line(&item.raw) {
            return;
        }
        let start = (0..stats_phys)
            .rev()
            .find(|&i| is_task_stats_line(&self.log.items[i].raw))
            .map(|i| i + 1)
            .unwrap_or(0);
        let mut parts: Vec<&str> = Vec::new();
        for item in &self.log.items[start..stats_phys] {
            let line = item.raw.as_str();
            if line.is_empty() || is_task_end_separator(line) || is_task_stats_line(line) {
                continue;
            }
            parts.push(line);
        }
        let text = parts.join("\n");
        if text.is_empty() {
            return;
        }
        self.copy_text_without_preview(&text);
    }

    /// Add a user input message and record it in task history.
    pub(crate) fn add_user_message(&mut self, content: String) {
        // The user's own words are the one append that always follows: they
        // just pressed Enter, so the echo belongs on screen. This is a
        // *deliberate* exception to follow-the-tail (`append_msg`), not an
        // accident of the old unconditional scroll — which is why it lives
        // here, at the user action, instead of in the append primitive.
        self.scroll_log_to_bottom();
        // Insert a blank line as separator first
        self.add_new_line();
        let msgs = self.msgs();
        // Style offline first so we don't hold `&self.skills_data` across `append_msg`.
        let theme = self.theme;
        let skill_names = crate::render::slash_style::skill_name_set(&self.skills_data);
        let pending: Vec<(Line<'static>, String)> = content
            .split('\n')
            .enumerate()
            .map(|(i, line)| {
                let text = if i == 0 {
                    msgs.user_msg_prefix.replace("{}", line)
                } else {
                    msgs.user_msg_cont.replace("{}", line)
                };
                let styled = crate::render::slash_style::style_user_skill_line(
                    &text,
                    &skill_names,
                    &theme,
                    msgs.user_msg_prefix,
                    msgs.user_msg_cont,
                )
                .unwrap_or_else(|| {
                    Line::from(Span::styled(
                        text.clone(),
                        Style::default().fg(theme.success),
                    ))
                });
                (styled, text)
            })
            .collect();
        for (styled, text) in pending {
            self.append_msg(styled, text, LogItemKind::User);
        }
        let timestamp = Local::now().format("%H:%M:%S").to_string();
        self.task_history.push(HistoryEntry {
            task: content,
            timestamp,
            summary: String::new(),
        });
        if self.task_history.len() > 20 {
            self.task_history.remove(0);
        }
        self.refresh_tool_log_scroll();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::test_harness::make_app;
    use agent_tui_kit::render::util::{LOG_TOOL_BLOCK_INDENT, LOG_TOOL_INDENT};
    use tact::hook::{HOOK_CONTEXT_CLOSE_TAG, HOOK_CONTEXT_OPEN_TAG};

    #[test]
    fn load_history_seeds_session_turn_counter() {
        let mut app = make_app();
        app.load_history(vec![
            tact_llm::Message::new_text(tact_llm::Role::User, "first".to_string()),
            tact_llm::Message::new_text(tact_llm::Role::Assistant, "answer".to_string()),
            tact_llm::Message::new_text(tact_llm::Role::User, "second".to_string()),
        ]);

        assert_eq!(
            app.status_bar_mut().turn_user,
            2,
            "each persisted user message should seed one turn"
        );
        assert_eq!(
            app.status_bar_mut().turn_llm,
            0,
            "resume must not invent in-flight LLM turns"
        );
    }

    #[test]
    fn load_history_skips_blank_user_text_when_seeding_turns() {
        let mut app = make_app();
        app.load_history(vec![
            tact_llm::Message::new_text(tact_llm::Role::User, "   ".to_string()),
            tact_llm::Message::new_text(tact_llm::Role::User, "real".to_string()),
        ]);

        assert_eq!(
            app.status_bar_mut().turn_user,
            1,
            "blank user text is skipped for display and must not count as a turn"
        );
    }

    /// Hook-injected context is shown as a system notice, not as something the
    /// user typed, and does not count as a turn.
    ///
    /// It also carries a provenance label: the `<hook-context>` framing is
    /// stripped, so without the label the reader cannot tell a plugin's briefing
    /// from a markdown notice Tact wrote itself — both are `SystemMarkdown`.
    #[test]
    fn load_history_renders_hook_context_as_a_system_notice() {
        let mut app = make_app();
        // As injected, and as a reload from disk would deliver it: same
        // markers, no in-memory kind.
        app.load_history(vec![
            tact_llm::Message::new_text(
                tact_llm::Role::User,
                format!(
                    "{}\nbrief body\n{}",
                    HOOK_CONTEXT_OPEN_TAG, HOOK_CONTEXT_CLOSE_TAG
                ),
            )
            .with_kind(tact_llm::MessageKind::HookContext),
            tact_llm::Message::new_text(
                tact_llm::Role::User,
                tact::hook::frame_hook_context(Some("plugin codex"), "from disk"),
            ),
            tact_llm::Message::new_text(tact_llm::Role::User, "real".to_string()),
        ]);

        assert_eq!(
            app.status_bar_mut().turn_user,
            1,
            "only the real turn counts"
        );
        let raws: Vec<&str> = app.log.items.iter().map(|item| item.raw.as_str()).collect();
        assert!(
            raws.iter().any(|raw| raw.contains("brief body")),
            "the briefing is kept: {raws:?}"
        );
        assert!(
            raws.iter().any(|raw| raw.contains("from disk")),
            "a reloaded session is recognized by its markers: {raws:?}"
        );
        assert!(
            raws.iter().any(|raw| raw.contains("plugin codex")),
            "a reloaded session still knows which hook spoke: {raws:?}"
        );
        assert!(
            !raws.iter().any(|raw| raw.contains(HOOK_CONTEXT_OPEN_TAG)),
            "the framing is not shown to the reader: {raws:?}"
        );

        let label = app.msgs().hook_context_label;
        assert_eq!(
            raws.iter().filter(|raw| raw.contains(label)).count(),
            2,
            "every hook cell is labelled, live and reloaded alike: {raws:?}"
        );
        let label_at = raws
            .iter()
            .position(|raw| raw.contains(label))
            .expect("a label row");
        let body_at = raws
            .iter()
            .position(|raw| raw.contains("brief body"))
            .expect("a body row");
        assert!(
            label_at < body_at,
            "the label leads the body it names: {raws:?}"
        );
    }

    /// The hook block is a container: a labelled header row, then every body row
    /// behind the bar — and the row tails beside the bar still carry the theme
    /// background (a gutter that leaves unpainted cells behind is how residue
    /// starts).
    #[test]
    fn hook_context_renders_a_labelled_barred_block() {
        use crate::render::test_harness::{
            buffer_first_char_x, render_log_panel_terminal, render_log_panel_text,
        };

        let mut app = make_app();
        app.append_hook_context_markdown(
            Some("plugin demo"),
            "first paragraph\n\nsecond paragraph",
        );

        let text = render_log_panel_text(&mut app, 80, 24);
        assert!(
            text.contains(app.msgs().hook_context_label),
            "the header names the block: {text}"
        );
        assert!(
            text.contains("plugin demo"),
            "the header names the hook that spoke: {text}"
        );
        // The header carries no expand hint and no size. The hint is the tail
        // row's job (spelled `[Open]`, the log's one word for "there is a
        // payload behind this row"), and a block too short to be truncated has
        // no tail — so a hint here would be either a duplicate or a second
        // spelling of the same gesture. The size went with it: at the tool
        // block's indent the header has to hold the block's name and its
        // source, and the count is the field the reader can already work out.
        let header_row = text
            .lines()
            .find(|row| row.contains(app.msgs().hook_context_label))
            .expect("the header row");
        assert!(
            header_row.contains("plugin demo"),
            "the header names the hook that spoke: {header_row}"
        );
        assert!(
            !header_row.contains(&collapsed_action_text(&app.msgs())),
            "the header must not spell the gesture the tail row owns: {header_row}"
        );
        assert!(
            !header_row.contains("3 lines"),
            "the header must not spend columns on the count: {header_row}"
        );
        // Both facts on one row, and it has to stay that way in a narrow panel.
        let narrow = render_log_panel_text(&mut app, 60, 24);
        assert!(
            narrow
                .lines()
                .any(|row| row.contains(app.msgs().hook_context_label)
                    && row.contains("plugin demo")),
            "the header fits one row at 60 columns: {narrow}"
        );
        assert!(
            text.contains("first paragraph") && text.contains("second paragraph"),
            "the body is rendered, not swallowed: {text}"
        );

        let terminal = render_log_panel_terminal(&mut app, 80, 24);
        let buf = terminal.backend().buffer();
        let bar_x = buffer_first_char_x(buf, '▎').expect("a bar on screen");
        let mut barred_rows = 0;
        for y in 0..buf.area.height {
            if buf[(bar_x, y)].symbol() != "▎" {
                continue;
            }
            barred_rows += 1;
            assert_eq!(
                buf[(buf.area.width - 1, y)].style().bg,
                Some(app.theme.bg),
                "row {y} tail must carry the theme background"
            );
        }
        assert!(
            barred_rows >= 3,
            "header plus every body row wears the bar, got {barred_rows}: {text}"
        );

        // The block sits in the *tool block* column — the one `ToolCell`
        // insets its whole area to, which is where a tool block's title, its
        // meta row and its detail card's border all start. `LOG_TOOL_INDENT`
        // is **not** that column: it belongs to the blank placeholder rows a
        // tool block overwrites, so aligning to it leaves the bar four columns
        // left of every tool row a reader can see.
        let terminal = render_log_panel_terminal(&mut app, 80, 24);
        let buf = terminal.backend().buffer();
        let bar_x = buffer_first_char_x(buf, '▎').expect("the hook bar is drawn");
        // Header and body share the one column: a bar anywhere else would mean
        // the block's rows disagree about where the block starts.
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                if buf[(x, y)].symbol() == "▎" {
                    assert_eq!(x, bar_x, "row {y} has a bar out of column");
                }
            }
        }
        // `render_log_panel` draws its left border in column 0, so the panel's
        // first content column is 1.
        let inner_x = 1;
        assert_eq!(
            bar_x,
            inner_x + LOG_TOOL_BLOCK_INDENT,
            "the hook bar must sit in the tool block column, not the placeholder one"
        );
        assert_ne!(
            bar_x,
            inner_x + LOG_TOOL_INDENT,
            "`LOG_TOOL_INDENT` is the placeholder column; nothing visible is drawn there"
        );
    }

    /// The block's left rail is unbroken, wrapped rows included.
    ///
    /// The header and the "… N more lines" tail are *text* rows, so a `▎` typed
    /// into their text marks only the first visual row and leaves the
    /// continuation bare — a hole in the rail, appearing exactly when the panel
    /// is narrow enough to force the wrap. Both rows therefore carry their bar
    /// as a row gutter (`LogItem::gutter`), and the wrap pass paints it on every
    /// row it produces.
    #[test]
    fn hook_context_rail_has_no_hole_when_rows_wrap() {
        use crate::render::test_harness::{buffer_first_char_x, render_log_panel_terminal};

        let mut app = make_app();
        let body = (1..=20)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        app.append_hook_context_markdown(Some("plugin codex"), &body);

        // Narrow enough that the header *and* the truncated tail both wrap.
        let terminal = render_log_panel_terminal(&mut app, 40, 24);
        let buf = terminal.backend().buffer();
        let bar_x = buffer_first_char_x(buf, '▎').expect("a bar on screen");
        let barred: Vec<u16> = (0..buf.area.height)
            .filter(|&y| buf[(bar_x, y)].symbol() == "▎")
            .collect();
        assert!(
            barred.len() >= 5,
            "header, body and tail rows all wear the bar: {barred:?}"
        );
        assert_eq!(
            barred,
            (barred[0]..=*barred.last().unwrap()).collect::<Vec<u16>>(),
            "a wrapped row dropped its bar, leaving a hole in the rail"
        );
    }

    /// The switch is display-only, and it is applied where the rows would enter
    /// the log: with it off the block leaves no rows at all — not a blank row,
    /// not a collapsed stub — and the *completion* half of a progress line
    /// cannot append the row its opening deliberately skipped.
    #[test]
    fn hook_output_off_keeps_hook_rows_out_of_the_log() {
        let mut app = make_app();
        app.set_hook_output(false);

        app.append_hook_context_markdown(Some("plugin codex"), "brief body");
        app.apply_hook_status(7, Some("plugin codex"), "Loading context", None);
        app.apply_hook_status(7, Some("plugin codex"), "Loading context", Some(3_400));

        assert!(
            app.log.items.is_empty(),
            "a hidden hook leaves no row behind: {:?}",
            app.log.items
        );

        // And the switch is a switch, not a one-way latch.
        app.set_hook_output(true);
        app.append_hook_context_markdown(Some("plugin codex"), "brief body");
        assert!(
            app.log
                .items
                .iter()
                .any(|item| item.raw.contains(app.msgs().hook_context_label)),
            "turning it back on restores the block: {:?}",
            app.log.items
        );
    }

    /// A briefing longer than the inline budget keeps its head in the log and
    /// puts the rest behind the popup — and a cut inside a fence closes it, so
    /// the rows after the block are not swallowed into the code block.
    #[test]
    fn hook_context_truncates_a_long_body_and_keeps_the_rest_reachable() {
        let mut app = make_app();
        let body = format!(
            "head line\n```text\n{}\n```\ntail line",
            (1..=20)
                .map(|i| format!("fenced {i}"))
                .collect::<Vec<_>>()
                .join("\n")
        );
        app.append_hook_context_markdown(Some("plugin demo"), &body);

        let header = &app.log.items[0];
        assert_eq!(
            header.popup_source.as_deref(),
            Some(body.as_str()),
            "the header stands for the whole body"
        );
        let more = app
            .log
            .items
            .iter()
            .find(|item| item.raw.contains("more lines"))
            .expect("a row that says how much was left out");
        assert_eq!(
            more.popup_source.as_deref(),
            Some(body.as_str()),
            "the tail row opens the same full body"
        );
        // The tail spells the gesture the way the log spells it everywhere
        // else: the bracketed action a collapsed tool block's meta row draws,
        // from the same definition, so the two `[Open]`s cannot drift.
        assert!(
            more.raw.ends_with(&collapsed_action_text(&app.msgs())),
            "the tail wears the shared bracketed action: {}",
            more.raw
        );

        let shown = app
            .log
            .items
            .iter()
            .find(|item| item.markdown_cell.is_some())
            .expect("the head is rendered");
        assert_eq!(
            shown.raw.matches("```").count() % 2,
            0,
            "a cut fence is closed: {}",
            shown.raw
        );
        assert!(shown.raw.contains("head line"), "{}", shown.raw);
        assert!(
            !shown.raw.contains("tail line"),
            "the body was actually cut: {}",
            shown.raw
        );
    }

    /// A hook's progress line has a lifetime: it appears while the subprocess
    /// runs, and the completion rewrites **that same row** — not a second one,
    /// and not a stale "Loading…" left behind. The row keeps its index because
    /// removing it would shift every physical index the TUI keys on.
    #[test]
    fn hook_status_is_rewritten_in_place_when_the_hook_returns() {
        let mut app = make_app();
        let label = app.msgs().hook_status_label;
        app.apply_hook_status(
            7,
            Some("plugin codex"),
            "Loading Basic Memory context",
            None,
        );

        let running = app.log.items.last().expect("a progress row");
        assert_eq!(running.kind, LogItemKind::HookStatus(7));
        assert_eq!(
            running.raw,
            format!("⌁ {label} · plugin codex · Loading Basic Memory context"),
            "the running line names the hook and quotes the plugin"
        );
        assert_eq!(
            running.line.spans[0].style.fg,
            Some(app.theme.accent),
            "running wears the accent the block will wear"
        );
        // The bar is the row's gutter, not a span in its text, so it survives
        // a wrap — and it takes the same colour as the row it marks.
        assert_eq!(
            running.gutter,
            Some(Gutter {
                glyph: HOOK_CONTEXT_GUTTER,
                color: app.theme.accent,
            }),
            "the running row wears the accent bar"
        );
        let idx = app.log.items.len() - 1;

        app.apply_hook_status(
            7,
            Some("plugin codex"),
            "Loading Basic Memory context",
            Some(8200),
        );

        assert_eq!(
            app.log.items.len(),
            idx + 1,
            "the completion rewrites the row instead of adding one"
        );
        let done = &app.log.items[idx];
        assert_eq!(done.kind, LogItemKind::HookStatus(7));
        assert_eq!(
            done.raw,
            format!("⌁ {label} · plugin codex · Loading Basic Memory context · 8.2s")
        );
        assert_eq!(
            done.line.spans[0].style.fg,
            Some(app.theme.muted_fg()),
            "a finished line reads as a record, not as an activity"
        );
        assert_eq!(
            done.gutter,
            Some(Gutter {
                glyph: HOOK_CONTEXT_GUTTER,
                color: app.theme.muted_fg(),
            }),
            "and its bar drops to muted with it"
        );
        assert_ne!(
            app.log_scroll.visual_cache_ver,
            app.log.items.len(),
            "the row count did not change, so the cache token has to be moved by hand"
        );
    }

    #[test]
    fn hook_elapsed_prefers_milliseconds_under_a_second() {
        assert_eq!(format_hook_elapsed(340), "340ms");
        assert_eq!(format_hook_elapsed(1000), "1.0s");
        assert_eq!(format_hook_elapsed(100_400), "100.4s");
    }

    #[test]
    fn add_system_message_applies_semantic_colors() {
        let mut app = make_app();

        app.add_system_message("❌ Error: boom".into());
        assert_eq!(
            app.log.items.last().unwrap().line.spans[0].style.fg,
            Some(app.theme.error)
        );

        app.add_system_message("✓ Selected: x".into());
        assert_eq!(
            app.log.items.last().unwrap().line.spans[0].style.fg,
            Some(app.theme.success)
        );

        app.add_system_message("  ✓ still success".into());
        assert_eq!(
            app.log.items.last().unwrap().line.spans[0].style.fg,
            Some(app.theme.success)
        );

        app.add_system_message("📋 Copied: x".into());
        assert_eq!(
            app.log.items.last().unwrap().line.spans[0].style.fg,
            Some(app.theme.accent)
        );
    }

    #[test]
    fn system_markdown_keeps_explicit_kind_and_source() {
        let mut app = make_app();
        app.append_system_markdown("  **not bold**");

        assert!(
            app.log
                .items
                .last()
                .is_some_and(|item| item.markdown_cell.is_some())
        );
        assert_eq!(
            app.log.items.last().map(|item| item.kind),
            Some(LogItemKind::SystemMarkdown)
        );
        assert_eq!(
            app.log.items.last().map(|item| item.raw.as_str()),
            Some("  **not bold**")
        );
    }

    #[test]
    fn system_tool_rows_use_explicit_provenance() {
        let mut app = make_app();
        app.append_msg(
            Line::from(Span::styled(
                "  1. inspect files",
                Style::default().fg(app.theme.accent),
            )),
            "  1. inspect files".into(),
            LogItemKind::SystemTool,
        );

        let line = app.log.items.last().expect("rendered system tool row");
        assert_eq!(line.line.spans.len(), 1);
        assert_eq!(line.line.spans[0].style.fg, Some(app.theme.accent));
        assert_eq!(
            app.log.items.last().map(|item| item.kind),
            Some(LogItemKind::SystemTool)
        );
        assert_eq!(
            app.log.items.last().map(|item| item.raw.as_str()),
            Some("  1. inspect files")
        );
    }
}
