use crate::i18n::Messages;
use crate::theme::{Theme, ThemeName};
use crate::widgets::state::*;
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
        // Add each skill as a palette command (Claude Code style)
        for (name, _body) in &self.skills_data {
            let desc = format!("🎯 {}", name);
            cmds.push((name.clone(), desc));
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
        };
        self.add_system_message(msgs.theme_changed_tmpl.replace("{}", label));
        self.theme = Theme::from(next_name);
    }

    pub(crate) fn msgs(&self) -> Messages {
        Messages::by_language(self.language)
    }

    pub(crate) fn localize_cmd_desc(&self, cmd: &str) -> String {
        let msgs = self.msgs();
        match cmd {
            "theme" => msgs.cmd_theme.to_string(),
            "model" => msgs.cmd_model.to_string(),
            "save" => msgs.cmd_save.to_string(),
            "cancel" => msgs.cmd_cancel.to_string(),
            "quit" => msgs.cmd_quit.to_string(),
            "help" => msgs.cmd_help.to_string(),
            "history" => msgs.cmd_history.to_string(),
            "balance" => msgs.cmd_balance.to_string(),
            "lang" => msgs.cmd_lang.to_string(),
            "skills" => msgs.cmd_skills.to_string(),
            "skill-reload" => msgs.cmd_skill_reload.to_string(),
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

    use crate::render::test_harness::make_app;
    use crate::theme::ThemeName;

    #[test]
    fn toggle_theme_cycles_from_retro() {
        let mut app = make_app();
        assert_eq!(app.theme.name, ThemeName::Retro);

        app.toggle_theme();
        assert_ne!(app.theme.name, ThemeName::Retro);
        assert!(
            app.raw_messages
                .iter()
                .any(|m| m.contains("theme") || m.contains("Theme")),
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
