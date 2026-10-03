use crate::{
    i18n::Messages,
    theme::{Theme, ThemeName},
    widgets::state::*,
};
impl App {
    /// The built-in commands the palette lists, in [`SlashCommand::ALL`] order.
    ///
    /// Skills used to be appended here as `/{name}` entries, one per installed
    /// skill. A marketplace with dozens of them buried `/mcp`, `/compact` and
    /// the rest under a wall of skill names, and every new skill shifted the
    /// first-level list — so they are no longer first-level entries. They live
    /// under `/skill`: the popup offers them there (`/skill `, `/skill co`),
    /// `slash_candidates` is what knows about them, and `/{name}` still runs
    /// one directly for anyone who types it.
    pub(crate) fn palette_commands(&self) -> Vec<(String, String)> {
        let account_enabled = self.account_rx.is_some();
        let msgs = self.msgs();
        SlashCommand::ALL
            .iter()
            .filter(|cmd| account_enabled || !cmd.needs_account())
            .map(|cmd| (cmd.name().to_string(), cmd.desc(msgs).to_string()))
            .collect()
    }

    pub(crate) fn save_history(&self, entry: &str) {
        let _ = self
            .history_save_tx
            .send((self.session_id.clone(), entry.to_string()));
    }

    /// Cycle to the next built-in theme (`Ctrl+T`, and the old `/theme`).
    pub(crate) fn toggle_theme(&mut self) {
        self.set_theme(self.theme.name.next());
    }

    /// Switch to one theme and say so — `Ctrl+T`, which has no persist step.
    pub(crate) fn set_theme(&mut self, name: ThemeName) {
        let msgs = self.msgs();
        self.apply_theme(name);
        let label = theme_label(&msgs, name);
        self.add_system_message(msgs.theme_changed_tmpl.replace("{}", label));
    }

    /// Switch the theme silently.
    ///
    /// The `/theme` picker uses this and lets its persist step do the talking;
    /// two messages for one action ("Theme: nord" then "saved"/"session only")
    /// would say the same thing twice.
    pub(crate) fn apply_theme(&mut self, name: ThemeName) {
        self.theme = Theme::from(name);
    }

    pub(crate) fn msgs(&self) -> Messages {
        Messages::by_language(self.language)
    }

    /// Build the per-frame [`RenderCtx`] from disjoint `&self` borrows.
    ///
    /// The kit's pure render functions take `&RenderCtx` instead of `&App`;
    /// this is the single construction site (design doc §2.3). The only
    /// mutation path from render code is `RenderCommand`s, drained by the
    /// shell after the frame.
    pub(crate) fn render_ctx(&self) -> agent_tui_kit::render::ctx::RenderCtx<'_> {
        use agent_tui_kit::render::ctx::RenderCtx;
        RenderCtx {
            theme: &self.theme,
            messages: self.msgs(),
            log_scroll: &self.log_scroll,
            log: &self.log,
            code_blocks: &self.code_blocks,
            mermaid_blocks: &self.mermaid_blocks,
            tools: self.tools().state(),
            thinking: self.thinking().state(),
            stream: self.stream().state(),
            mouse: &self.mouse,
            skills_data: &self.skills_data,
            loading_idx: self.loading_idx,
            spinner_frame: self.spinner_frame,
            status_bar: self.status_bar().state(),
            status: &self.status,
            input_mode: self.input_mode,
            focused_panel: self.focused_panel,
            language: self.language,
            workspace_dir: self.workspace_dir.as_str(),
            model_context_window: self.model_context_window,
            process_start_time: &self.process_start_time,
            task_start_time: self.task_start_time.as_ref(),
            flash_msg: self.flash_msg.as_ref().map(|(m, _)| m.as_str()),
            copy_flash: self.copy_flash_at.is_some(),
            account: self.account_rx.as_ref().map(|_| &self.account),
            plan: self.plan().state(),
            input: &self.input,
            input_cursor: self.input_cursor,
            input_scroll: self.input_scroll,
            cmd_line: &self.cmd_line,
            pending_messages: &self.pending_messages,
            input_voice_title: self.voice_title(),
            code_popup: self.code_popup.as_ref(),
            mermaid_popup: self.mermaid_popup.as_ref(),
            system_prompt_popup: self.system_prompt_popup.as_ref(),
            subagent_popup: self.subagent_popup(),
            task_history: &self.task_history,
            select: &self.select,
            task_panel: self.task_panel().state(),
            subagent_panel: self.subagent_panel().state(),
            background_panel: self.background_panel().state(),
        }
    }

    /// Flip the UI language and say so — `Ctrl+L`, which has no persist step.
    pub(crate) fn toggle_language(&mut self) {
        let next = self.language.next();
        let old_msgs = self.msgs();
        self.apply_language(next);
        self.add_system_message(old_msgs.lang_changed_tmpl.replace("{}", next.label()));
    }

    /// Switch the language silently, handing the new [`Messages`] to every
    /// component that owns one.
    ///
    /// `/lang` uses this and lets its persist step do the talking; two messages
    /// for one action ("Language: 中文" then "saved"/"session only") would say
    /// the same thing twice.
    ///
    /// `self.language` is what the *render* path reads, so flipping it alone
    /// would leave the components building their log text (event messages, card
    /// chrome) in the old locale while the rows they anchor are drawn in the new
    /// one. Components take a snapshot at construction, so the snapshot is
    /// refreshed here — the one place the language changes.
    pub(crate) fn apply_language(&mut self, language: Language) {
        self.language = language;
        let msgs = self.msgs();
        self.thinking_mut().set_messages(msgs);
        self.stream_mut().set_messages(msgs);
        self.tools_mut().set_messages(msgs);
    }
}

/// The localized name of one theme.
///
/// Single source for the `/theme` picker's rows and the "theme changed" line:
/// two spellings of the same theme would be a bug the moment one is renamed.
pub(crate) fn theme_label(msgs: &Messages, name: ThemeName) -> &'static str {
    match name {
        ThemeName::Dark => msgs.theme_dark,
        ThemeName::Light => msgs.theme_light,
        ThemeName::SolarizedDark => msgs.theme_solarized_dark,
        ThemeName::SolarizedLight => msgs.theme_solarized_light,
        ThemeName::GruvboxDark => msgs.theme_gruvbox_dark,
        ThemeName::Nord => msgs.theme_nord,
        ThemeName::Retro => msgs.theme_retro,
        ThemeName::Kawaii => msgs.theme_kawaii,
        ThemeName::Japanese => msgs.theme_japanese,
        ThemeName::Brutal => msgs.theme_brutal,
        ThemeName::Ink => msgs.theme_ink,
        ThemeName::InkLight => msgs.theme_ink_light,
    }
}

#[cfg(test)]
mod tests {

    use super::theme_label;
    use crate::{
        i18n::Language,
        render::test_harness::make_app,
        theme::{Theme, ThemeName},
    };

    #[test]
    fn toggle_theme_cycles_from_ink() {
        let mut app = make_app();
        assert_eq!(app.theme.name, ThemeName::Ink);

        app.toggle_theme();
        assert_ne!(app.theme.name, ThemeName::Ink);
        assert!(
            app.log
                .items
                .iter()
                .any(|item| item.raw.contains("theme") || item.raw.contains("Theme")),
            "toggle should append theme changed message"
        );
    }

    #[test]
    fn set_theme_switches_to_the_named_theme() {
        let mut app = make_app();
        assert_eq!(app.theme.name, ThemeName::Ink);

        app.set_theme(ThemeName::SolarizedLight);

        assert_eq!(app.theme.name, ThemeName::SolarizedLight);
        assert_eq!(app.theme.fg, Theme::from(ThemeName::SolarizedLight).fg);
        assert_eq!(app.theme.bg, Theme::from(ThemeName::SolarizedLight).bg);
        assert!(
            app.log.items.iter().any(|item| item
                .raw
                .contains(theme_label(&app.msgs(), ThemeName::SolarizedLight))),
            "the switch must name the theme it moved to: {:?}",
            app.log.items
        );
    }

    #[test]
    fn toggle_language_switches_en_and_zh() {
        let mut app = make_app();
        let before = app.language;

        app.toggle_language();

        assert_ne!(app.language, before);
    }

    /// The silent half must move `language` too, or `/lang` would apply the
    /// locale to the picker's answer and not to the app it belongs to.
    #[test]
    fn apply_language_switches_without_announcing() {
        let mut app = make_app();
        app.language = Language::English;
        let before = app.log.items.len();

        app.apply_language(Language::Chinese);

        assert_eq!(app.language, Language::Chinese);
        assert_eq!(
            app.log.items.len(),
            before,
            "the persist step speaks for /lang; apply_language must stay silent"
        );
    }

    /// The language and its canonical config name are separate on purpose: the
    /// label is drawn for the user, the name goes into a file.
    #[test]
    fn language_names_round_trip_through_the_config_spelling() {
        for language in Language::all() {
            assert_eq!(
                Language::parse(language.as_str()),
                Some(*language),
                "{} must parse back from what it writes",
                language.as_str()
            );
        }
        assert_eq!(Language::parse("EN"), Some(Language::English));
        assert_eq!(Language::parse(" chinese "), Some(Language::Chinese));
        assert_eq!(Language::parse("jp"), None, "a typo is not a fallback");
    }
}
