use agent_tui_kit::state::{clamp_step, contains_ignore_case};

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
    /// Adopt the shared slot the host fills with the runtime's catalog.
    ///
    /// The slot, not the catalog: the view is up before the agent is built, so
    /// it has to hold the place the catalog will appear in. Reading it happens
    /// in `palette_commands`, which is why there is no "is it ready" flag to
    /// keep in step.
    pub(crate) fn set_command_directory(&mut self, slot: crate::ports::SharedCommandDirectory) {
        self.commands = slot;
    }

    pub(crate) fn palette_commands(&self) -> Vec<(String, String)> {
        let account_enabled = self.account_rx.is_some();
        let msgs = self.msgs();
        // Local (view-scoped) commands first: they exist whether or not a
        // runtime catalog was installed, and a picker that lost `/theme`
        // because a plugin failed to load would be a regression.
        let mut commands: Vec<(String, String)> = SlashCommand::ALL
            .iter()
            .filter(|cmd| account_enabled || !cmd.needs_account())
            .map(|cmd| (cmd.name().to_string(), cmd.desc(msgs).to_string()))
            .collect();

        // Then the catalog's commands, which are the ones a plugin contributed.
        // Without this a command the runtime can route is unreachable from the
        // palette, which is the half of "commands are distributed" a user sees.
        let directory = self
            .commands
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone();
        if let Some(directory) = directory {
            for entry in directory.commands() {
                // A local command owns its name: the catalog must not shadow
                // `/theme` with a contributed command of the same name.
                if commands.iter().any(|(name, _)| name == &entry.name) {
                    continue;
                }
                commands.push((entry.name, entry.summary));
            }
        }
        commands
    }

    /// Indices into [`Self::palette_commands`] that match the current
    /// `cmd_line` filter.
    ///
    /// The one definition of "what the palette is showing": the renderer, the
    /// Enter handler and the cursor step all read it, so the filtered list and
    /// the highlighted row can never disagree.
    pub(crate) fn palette_filtered(&self) -> Vec<usize> {
        let commands = self.palette_commands();
        let filter = self.cmd_line.as_str();
        commands
            .iter()
            .enumerate()
            .filter(|(_, (cmd, desc))| {
                contains_ignore_case(cmd, filter) || contains_ignore_case(desc, filter)
            })
            .map(|(i, _)| i)
            .collect()
    }

    /// Move the palette cursor by `delta`, clamped to the filtered list.
    pub(crate) fn step_palette_selection(&mut self, delta: i32) {
        let len = self.palette_filtered().len();
        self.palette_selected = clamp_step(len, self.palette_selected, delta);
    }

    pub(crate) fn save_history(&self, entry: &str) {
        let _ = self
            .history_save_tx
            .send((self.session_id.clone(), entry.to_string()));
    }

    /// Cycle to the next built-in theme (`Ctrl+T`).
    ///
    /// Unlike the `/theme` picker this path does not ask: it is the "just give
    /// me the next one" shortcut, and a question on every press would defeat
    /// it. The choice is written straight to `[ui] theme`, so the shortcut no
    /// longer loses the theme on the next launch.
    pub(crate) fn toggle_theme(&mut self) {
        let name = self.theme.name.next();
        self.apply_theme(name);
        self.persist_theme_choice(name);
    }

    /// Flip whether hook-injected content is drawn, and say so — `/hook-output`.
    ///
    /// Shaped like [`Self::toggle_theme`] rather than like the `/theme` picker:
    /// a boolean has no list to open, so there is no picker step for the
    /// "save it?" question to hang on, and the one message names both halves —
    /// the new state and whether it outlives the session. The rows already in
    /// the log are left alone: this gates what gets appended, so the switch is
    /// visible in what follows rather than by a rewind.
    pub(crate) fn toggle_hook_output(&mut self) {
        self.hook_output = !self.hook_output;
        self.persist_hook_output_choice(self.hook_output);
    }

    /// Report the new state and the write — the same contract as
    /// [`Self::persist_theme_choice`].
    fn persist_hook_output_choice(&mut self, enabled: bool) {
        let msgs = self.msgs();
        let outcome = if !self.ui_config_available() {
            msgs.hook_output_session_only
        } else {
            let result = match self.settings_writer.clone() {
                Some(writer) => writer.persist_ui(crate::ports::UiSetting::HookOutput(enabled)),
                None => Ok(()),
            };
            match result {
                Ok(()) => msgs.hook_output_persisted,
                Err(error) => {
                    let message = msgs
                        .hook_output_persist_failed_tmpl
                        .replace("{}", &error.to_string());
                    self.add_system_message(message);
                    return;
                }
            }
        };
        let template = if enabled {
            msgs.hook_output_shown_tmpl
        } else {
            msgs.hook_output_hidden_tmpl
        };
        self.add_system_message(template.replace("{}", outcome));
    }

    /// Adopt the configured `[ui] hook_output` at startup.
    pub(crate) fn set_hook_output(&mut self, enabled: bool) {
        self.hook_output = enabled;
    }

    /// Switch the theme silently, then report the change and the write in one
    /// message.
    ///
    /// The picker applies through [`Self::apply_theme`] and lets its own
    /// persist step do the talking; this path has no step, so the message has
    /// to carry both halves — which is why it never goes through a separate
    /// "announce" call.
    fn persist_theme_choice(&mut self, name: ThemeName) {
        let msgs = self.msgs();
        if !self.ui_config_available() {
            let label = theme_label(&msgs, name);
            self.add_system_message(msgs.theme_session_only_tmpl.replace("{}", label));
            return;
        }
        let result = match self.settings_writer.clone() {
            Some(writer) => {
                writer.persist_ui(crate::ports::UiSetting::Theme(name.as_str().to_string()))
            }
            None => Ok(()),
        };
        match result {
            Ok(()) => {
                self.add_system_message(msgs.theme_persisted_tmpl.replace("{}", name.as_str()))
            }
            Err(error) => self.add_system_message(
                msgs.theme_persist_failed_tmpl
                    .replace("{}", &error.to_string()),
            ),
        }
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
            log_following: self.log_scroll.follow,
            log_unseen: self.log_scroll.unseen,
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
            task_dag_popup: self.task_dag_popup.as_ref(),
            task_history: &self.task_history,
            select: &self.select,
            goal_panel: self.goal_panel().state(),
            task_panel: self.task_panel().state(),
            subagent_panel: self.subagent_panel().state(),
            background_panel: self.background_panel().state(),
        }
    }

    /// Flip the UI language and say so — `Ctrl+L`, which has no persist step.
    pub(crate) fn toggle_language(&mut self) {
        let next = self.language.next();
        self.apply_language(next);
        self.persist_language_choice(next);
    }

    /// Cycle the language silently, then report the change and the write in one
    /// message — the same contract as [`Self::persist_theme_choice`].
    ///
    /// The message is spoken in the language just switched *to*, matching what
    /// the `/lang` picker's persist step already does.
    fn persist_language_choice(&mut self, language: Language) {
        let msgs = self.msgs();
        if !self.ui_config_available() {
            let label = language.label();
            self.add_system_message(msgs.lang_session_only_tmpl.replace("{}", label));
            return;
        }
        let result = match self.settings_writer.clone() {
            Some(writer) => writer.persist_ui(crate::ports::UiSetting::Language(
                language.as_str().to_string(),
            )),
            None => Ok(()),
        };
        match result {
            Ok(()) => {
                self.add_system_message(msgs.lang_persisted_tmpl.replace("{}", language.as_str()))
            }
            Err(error) => self.add_system_message(
                msgs.lang_persist_failed_tmpl
                    .replace("{}", &error.to_string()),
            ),
        }
    }

    /// Record where a `[ui]` preference can be written back to.
    ///
    /// Called once at startup, after `App::new`, like
    /// [`Self::set_configured_language`]. Left `None` (the test default) every
    /// toggle reports "this session only" and writes nothing — which is also
    /// what keeps the theme/lang tests from touching a file.
    pub(crate) fn set_ui_config_path(&mut self, path: Option<std::path::PathBuf>) {
        self.ui_config_path = path;
    }

    /// Record the resolved configuration this view was started with.
    ///
    /// Called once at startup, after `App::new`, from the same place that seeds
    /// the other configured values. A view that is never given one keeps every
    /// reader on its built-in default — the behaviour a test and a config-less
    /// run both need.
    pub(crate) fn set_config_snapshot(&mut self, config: Option<crate::ports::ViewSettings>) {
        self.config_snapshot = config;
    }

    /// Re-read the configuration after this view changed it.
    ///
    /// The `/model` writes go through `tact::config`'s update helpers, which
    /// also adjust derived values (a budget that no longer fits raises
    /// `max_tokens`). The snapshot has to follow, or the status bar would show
    /// the previous ceiling. This is the one place the view refreshes it, so the
    /// reason is stated once instead of at each call site.
    pub(crate) fn refresh_config_snapshot(&mut self) {
        // Through the port, immediately after a write went through it: the write
        // may have recomputed derived values (a budget that no longer fits raises
        // `max_tokens`), and the port owns the projection.
        self.config_snapshot = self
            .settings_writer
            .as_ref()
            .and_then(|writer| writer.current_settings());
    }

    /// The resolved configuration, if the host supplied one.
    ///
    /// Cloned rather than borrowed because the callers consume parts of it
    /// (`model_profiles.get(..).cloned()`), and a `ResolvedConfig` is a few
    /// small structs rather than a large tree.
    pub(crate) fn config_snapshot(&self) -> Option<crate::ports::ViewSettings> {
        self.config_snapshot.clone()
    }

    /// Whether there is a config file to write a `[ui]` preference into.
    ///
    /// Shared by `/theme`, `/lang`, `/hook-output`, `Ctrl+T` and `Ctrl+L`: all
    /// of them persist through the same `[ui]` table, so all of them must agree
    /// on whether that table has a file to live in.
    pub(crate) fn ui_config_available(&self) -> bool {
        self.ui_config_path.is_some()
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
        i18n::Language, ports::CommandDirectory, ports::CommandEntry,
        render::test_harness::make_app, theme::ThemeName,
    };

    /// A directory holding exactly the entries it was built with.
    struct FixedDirectory(Vec<CommandEntry>);

    impl CommandDirectory for FixedDirectory {
        fn commands(&self) -> Vec<CommandEntry> {
            self.0.clone()
        }

        fn resolve(&self, name: &str) -> Option<CommandEntry> {
            self.0.iter().find(|entry| entry.name == name).cloned()
        }
    }

    /// A slot already holding `directory`, which is what the host hands the view.
    fn filled(
        directory: std::sync::Arc<dyn CommandDirectory>,
    ) -> crate::ports::SharedCommandDirectory {
        let slot = crate::ports::shared_command_directory();
        *slot.lock().unwrap() = Some(directory);
        slot
    }

    fn entry(name: &str, summary: &str) -> CommandEntry {
        CommandEntry {
            name: name.to_string(),
            summary: summary.to_string(),
            needs_args: false,
        }
    }

    /// A command the runtime contributes appears in the palette.
    ///
    /// Without this a plugin's command is reachable only by someone who already
    /// knows its name — the palette is the only discovery surface there is.
    #[test]
    fn catalog_commands_join_the_palette() {
        let mut app = make_app();
        // A name no local command owns, so this measures the wiring rather than
        // a shadowing rule.
        assert!(
            !app.palette_commands()
                .iter()
                .any(|(name, _)| name == "contributed"),
            "the fixture starts without a catalog"
        );

        app.set_command_directory(filled(std::sync::Arc::new(FixedDirectory(vec![entry(
            "contributed",
            "A command a plugin contributed",
        )]))));

        let listed = app.palette_commands();
        let found = listed
            .iter()
            .find(|(name, _)| name == "contributed")
            .expect("the contributed command is offered");
        assert_eq!(found.1, "A command a plugin contributed");
    }

    /// A local command owns its name: the catalog cannot shadow it.
    ///
    /// The view's own commands must keep working whether or not a plugin was
    /// loaded, and a contributed `/theme` would otherwise replace a local one
    /// that the keybinding path still uses.
    #[test]
    fn a_local_command_is_not_shadowed_by_the_catalog() {
        let mut app = make_app();
        let local = app
            .palette_commands()
            .into_iter()
            .find(|(name, _)| name == "theme")
            .expect("theme is a local command");

        app.set_command_directory(filled(std::sync::Arc::new(FixedDirectory(vec![entry(
            "theme",
            "A contributed theme command",
        )]))));

        let listed = app.palette_commands();
        let themes: Vec<_> = listed.iter().filter(|(name, _)| name == "theme").collect();
        assert_eq!(themes.len(), 1, "the name appears once, not twice");
        assert_eq!(
            themes[0].1, local.1,
            "the local description wins over the contributed one"
        );
    }

    /// A command that moved to the catalog is offered by the catalog.
    ///
    /// `/stats` and `/background` used to be local variants. Removing them must
    /// not remove them from the palette: they are runtime commands now, and the
    /// catalog is what knows their names and descriptions.
    #[test]
    fn a_moved_command_is_still_offered_by_the_catalog() {
        let mut app = make_app();
        assert!(
            !app.palette_commands()
                .iter()
                .any(|(name, _)| name == "stats" || name == "background"),
            "they are no longer local commands"
        );

        app.set_command_directory(filled(std::sync::Arc::new(FixedDirectory(vec![
            entry("stats", "Show session statistics"),
            entry("background", "List background tasks"),
        ]))));

        let listed = app.palette_commands();
        for name in ["stats", "background"] {
            assert!(
                listed.iter().any(|(listed_name, _)| listed_name == name),
                "/{name} is still offered: {listed:?}"
            );
        }
    }

    /// Without a catalog the palette is exactly the local commands.
    #[test]
    fn no_catalog_means_local_commands_only() {
        let app = make_app();
        assert!(
            app.commands.lock().unwrap().is_none(),
            "the fixture starts without a catalog"
        );
        let listed = app.palette_commands();
        assert!(!listed.is_empty());
        assert!(listed.iter().all(|(name, _)| !name.is_empty()));
    }

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
    fn toggle_theme_without_a_config_file_says_session_only() {
        let mut app = make_app();
        assert!(!app.ui_config_available(), "the test default is no file");

        app.toggle_theme();

        assert_ne!(app.theme.name, ThemeName::Ink);
        let label = theme_label(&app.msgs(), app.theme.name);
        assert!(
            app.log
                .items
                .iter()
                .any(|item| item.raw.contains(label) && item.raw.contains("session")),
            "the shortcut must name the theme it moved to and admit it did not \
             save: {:?}",
            app.log.items
        );
    }

    /// `Ctrl+T` used to apply and announce only, so the theme was back to the
    /// configured one on the next launch. It now writes `[ui] theme` — the
    /// write itself is covered in `handlers::select::tests`, where the config
    /// fixture and the lock that serializes the global settings already live.
    #[test]
    fn a_config_file_means_a_real_write_attempt() {
        let mut app = make_app();
        app.set_ui_config_path(Some(std::path::PathBuf::from("/nonexistent/config.toml")));

        app.toggle_theme();

        // Whether the write lands or fails, the line must not be the
        // session-only one: "no file" and "could not write" are different
        // states and the user has to be able to tell them apart.
        let label = theme_label(&app.msgs(), app.theme.name);
        let session_only = app.msgs().theme_session_only_tmpl.replace("{}", label);
        assert!(
            !app.log.items.iter().any(|item| item.raw == session_only),
            "a config file means a write is attempted: {:?}",
            app.log.items
        );
    }

    /// `/hook-output` is the `Ctrl+T` shape, not the `/theme` one: a boolean
    /// has no list to pick from, so there is no picker step to hang the "save
    /// it?" question on — the one message carries both halves instead.
    #[test]
    fn toggle_hook_output_flips_and_says_session_only_without_a_file() {
        let mut app = make_app();
        assert!(app.hook_output, "hook output is drawn by default");
        assert!(!app.ui_config_available(), "the test default is no file");

        app.toggle_hook_output();

        assert!(!app.hook_output);
        let expected = app
            .msgs()
            .hook_output_hidden_tmpl
            .replace("{}", app.msgs().hook_output_session_only);
        assert!(
            app.log.items.iter().any(|item| item.raw == expected),
            "the toggle must say what it is now and that it did not save: {:?}",
            app.log.items
        );

        app.toggle_hook_output();

        assert!(app.hook_output, "and it is a toggle, not a one-way latch");
        let expected = app
            .msgs()
            .hook_output_shown_tmpl
            .replace("{}", app.msgs().hook_output_session_only);
        assert!(
            app.log.items.iter().any(|item| item.raw == expected),
            "{:?}",
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
