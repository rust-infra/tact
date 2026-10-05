use chrono::Local;
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};
use tact_llm::content::{ContentBlock, Message, MessageContent, Role};

use tact::hook::{hook_context_body, is_hook_context_text};

use agent_tui_kit::widgets::button::{Button, ButtonTheme, ButtonVariant};

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
                                self.append_hook_context_markdown(hook_context_body(content));
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

    /// Append hook-injected context: a labelled header row, then the body
    /// behind a left bar.
    ///
    /// The body renders through the same Markdown pipeline as
    /// [`Self::append_system_markdown`] — the caller has already stripped the
    /// `<hook-context>` framing — which is exactly why the block needs a shape
    /// of its own: without it a hook's stdout and a markdown notice Tact wrote
    /// itself are the same rows. The header names the source, the bar holds the
    /// whole block together (blank rows included), and the line count tells the
    /// reader how much they are scrolling past.
    pub(crate) fn append_hook_context_markdown(&mut self, body: &str) {
        let label = self.msgs().hook_context_label;
        let bar = HOOK_CONTEXT_GUTTER;
        let lines = body.lines().count();
        let header = format!("{bar}{HOOK_CONTEXT_MARK} {label} · {lines} lines");
        self.append_msg(
            Line::from(vec![
                Span::styled(bar, Style::default().fg(self.theme.accent)),
                Span::styled(
                    format!("{HOOK_CONTEXT_MARK} {label}"),
                    Style::default()
                        .fg(self.theme.accent)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!(" · {lines} lines"),
                    Style::default().fg(self.theme.muted_fg()),
                ),
            ]),
            header,
            LogItemKind::SystemPlain(SystemMsgStyle::Default),
        );
        self.log.append_markdown_with_gutter(
            body.to_string(),
            &self.theme,
            LogItemKind::SystemMarkdown,
            Gutter {
                glyph: bar,
                color: self.theme.accent,
            },
        );
    }

    /// Append a task-completion stats block right after the task-end separator.
    ///
    /// Reads the already-frozen `last_prompt_elapsed_secs` and the status-bar
    /// token/model snapshots; deliberately adds no new state (YAGNI — the data
    /// is already collected by `add_task_end_separator` / `TokenUsage` /
    /// `ModelInfo` updates). The leading copy affordance (`⎘`, an icon so it
    /// needs no translation) copies this turn's log text — from the previous
    /// stats row, or session start, up to but not including this stats row.
    pub(crate) fn add_task_stats_block(&mut self) {
        let secs = self.last_prompt_elapsed_secs.unwrap_or(0).max(0);
        let mm_ss = format!("{:02}:{:02}", secs / 60, secs % 60);

        let mut parts = vec![format!("⏱ {mm_ss}")];
        if !self.status_bar_mut().model_name.is_empty() {
            parts.push(self.status_bar_mut().model_name.clone());
        }
        let tokens = self.status_bar_mut().token_total;
        if tokens > 0 {
            let mut detail = format!("{tokens} tokens");
            let sub: Vec<String> = [
                ("prompt", self.status_bar_mut().token_prompt),
                ("completion", self.status_bar_mut().token_completion),
                ("cache", self.status_bar_mut().token_cache_hit),
                ("reasoning", self.status_bar_mut().token_reasoning),
            ]
            .into_iter()
            .filter(|(_, v)| *v > 0)
            .map(|(k, v)| format!("{k} {v}"))
            .collect();
            if !sub.is_empty() {
                detail.push_str(&format!(" ({})", sub.join(" · ")));
            }
            parts.push(detail);
        }
        let msgs = self.msgs();
        let body = format!("{}{}", msgs.task_stats_prefix, parts.join(" · "));
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
        self.append_msg(line, raw, LogItemKind::SystemPlain(SystemMsgStyle::Default));
        if self.input_mode == InputMode::Insert || self.input_mode == InputMode::Normal {
            self.scroll_log_to_bottom();
        }
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
                format!(
                    "{}\nfrom disk\n{}",
                    HOOK_CONTEXT_OPEN_TAG, HOOK_CONTEXT_CLOSE_TAG
                ),
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
        app.append_hook_context_markdown("first paragraph\n\nsecond paragraph");

        let text = render_log_panel_text(&mut app, 80, 24);
        assert!(
            text.contains(app.msgs().hook_context_label),
            "the header names the block: {text}"
        );
        assert!(
            text.contains("3 lines"),
            "the header counts the body it heads: {text}"
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
