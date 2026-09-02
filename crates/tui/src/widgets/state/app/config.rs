use crate::{
    i18n::Messages,
    theme::{Theme, ThemeName},
    widgets::state::*,
};
impl App {
    /// Palette commands visible for the current provider configuration,
    /// including dynamic skill commands.
    pub(crate) fn palette_commands(&self) -> Vec<(String, String)> {
        let account_enabled = self.account_rx.is_some();
        let mut cmds: Vec<(String, String)> = PALETTE_COMMANDS
            .iter()
            .filter(move |(cmd, _)| account_enabled || *cmd != "balance")
            .map(|&(cmd, _desc)| {
                let desc = self.localize_cmd_desc(cmd);
                (cmd.to_string(), desc)
            })
            .collect();
        // Skills as slash targets (Claude Code style `/skill-name`).
        // Skip names that collide with built-ins — builtins always win on Enter.
        let builtin_names: std::collections::HashSet<&str> =
            PALETTE_COMMANDS.iter().map(|(n, _)| *n).collect();
        for skill in &self.skills_data {
            if builtin_names.contains(skill.name.as_str()) {
                continue;
            }
            let desc = if skill.description.is_empty() {
                skill.name.clone()
            } else {
                skill.description.clone()
            };
            cmds.push((skill.name.clone(), desc));
        }
        cmds
    }

    pub(crate) fn save_history(&self, entry: &str) {
        let _ = self
            .history_save_tx
            .send((self.session_id.clone(), entry.to_string()));
    }

    pub(crate) fn toggle_theme(&mut self) {
        let next_name = self.theme.name.next();
        let msgs = self.msgs();
        let label = match next_name {
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
        };
        self.add_system_message(msgs.theme_changed_tmpl.replace("{}", label));
        self.theme = Theme::from(next_name);
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
        }
    }

    pub(crate) fn localize_cmd_desc(&self, cmd: &str) -> String {
        let msgs = self.msgs();
        match cmd {
            "theme" => msgs.cmd_theme.to_string(),
            "model" => msgs.cmd_model.to_string(),
            "model-subagent" => msgs.cmd_model_subagent.to_string(),
            "save" => msgs.cmd_save.to_string(),
            "cancel" => msgs.cmd_cancel.to_string(),
            "subagent_cancel" => msgs.cmd_subagent_cancel.to_string(),
            "quit" => msgs.cmd_quit.to_string(),
            "help" => msgs.cmd_help.to_string(),
            "history" => msgs.cmd_history.to_string(),
            "balance" => msgs.cmd_balance.to_string(),
            "lang" => msgs.cmd_lang.to_string(),
            "skills" => msgs.cmd_skills.to_string(),
            "skill-reload" => msgs.cmd_skill_reload.to_string(),
            "plugin" => msgs.cmd_plugin.to_string(),
            "tasks-dag" => msgs.cmd_tasks_dag.to_string(),
            "background" => msgs.cmd_background.to_string(),
            _ => cmd.to_string(),
        }
    }

    pub(crate) fn toggle_language(&mut self) {
        let next = self.language.next();
        let label = next.label();
        let old_msgs = self.msgs();
        self.language = next;
        self.add_system_message(old_msgs.lang_changed_tmpl.replace("{}", label));
    }
}

#[cfg(test)]
mod tests {

    use crate::{render::test_harness::make_app, theme::ThemeName};

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
    fn toggle_language_switches_en_and_zh() {
        let mut app = make_app();
        let before = app.language;

        app.toggle_language();

        assert_ne!(app.language, before);
    }
}
