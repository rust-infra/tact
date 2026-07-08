use crate::i18n::Messages;
use crate::theme::{Theme, ThemeName};
use crate::widgets::state::*;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
impl App {
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
        self.theme = Theme::by_name(next_name);
    }

    pub(crate) fn msgs(&self) -> Messages {
        Messages::by_language(self.language)
    }

    pub(crate) fn localize_cmd_desc(&self, cmd: &str) -> String {
        let msgs = self.msgs();
        match cmd {
            "theme" => msgs.cmd_theme.to_string(),
            "save" => msgs.cmd_save.to_string(),
            "cancel" => msgs.cmd_cancel.to_string(),
            "quit" => msgs.cmd_quit.to_string(),
            "help" => msgs.cmd_help.to_string(),
            "history" => msgs.cmd_history.to_string(),
            "search" => msgs.cmd_search.to_string(),
            "balance" => msgs.cmd_balance.to_string(),
            "lang" => msgs.cmd_lang.to_string(),
            "party" => msgs.cmd_party.to_string(),
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

    pub(crate) fn toggle_party_mode(&mut self) {
        self.party_mode = !self.party_mode;
        let msgs = self.msgs();
        if self.party_mode {
            let colors = [
                Color::Rgb(255, 105, 180),
                Color::Rgb(255, 165, 0),
                Color::Rgb(255, 215, 0),
                Color::Rgb(50, 205, 50),
                Color::Rgb(0, 191, 255),
                Color::Rgb(138, 43, 226),
                Color::Rgb(255, 0, 255),
            ];

            let cat_art = [
                "  ╱|、",
                " (˚ˎ 。7  ",
                "  |、˜\\\\",
                " じしˍ,)ノ",
                "",
                msgs.party_msg_1,
                msgs.party_msg_2,
                msgs.party_msg_3,
                "",
                msgs.party_hint,
            ];

            self.add_new_line();
            for (line_num, &line) in cat_art.iter().enumerate() {
                let color = colors[line_num % colors.len()];
                self.append_msg(
                    Line::from(Span::styled(
                        line.to_string(),
                        Style::default().fg(color).add_modifier(Modifier::BOLD),
                    )),
                    line.to_string(),
                    RawMessageType::LLM,
                );
            }
            self.add_new_line();
        } else {
            self.add_new_line();
            self.append_msg(
                Line::from(Span::styled(
                    msgs.party_exit,
                    Style::default()
                        .fg(Color::Rgb(180, 180, 180))
                        .add_modifier(Modifier::ITALIC),
                )),
                msgs.party_exit.to_string(),
                RawMessageType::LLM,
            );
            self.add_new_line();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
