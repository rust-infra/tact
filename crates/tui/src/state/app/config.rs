use crate::i18n::Messages;
use crate::state::*;
use crate::theme::{Theme, ThemeName};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use std::path::Path;

impl App {
    pub(crate) fn load_history(work_dir: &Path) -> Vec<String> {
        let path = work_dir.join(".tact").join("history.txt");
        std::fs::read_to_string(&path)
            .map(|s| s.lines().map(|l| l.to_string()).collect())
            .unwrap_or_default()
    }

    pub(crate) fn save_history(&self) {
        let dir = self.work_dir.join(".tact");
        if !dir.exists() {
            let _ = std::fs::create_dir_all(&dir);
        }
        let path = dir.join("history.txt");
        let data = self.input_history.entries.join("\n");
        let _ = std::fs::write(&path, data);
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
                self.messages.push(Line::from(Span::styled(
                    line.to_string(),
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                )));
                self.raw_messages.push(line.to_string());
            }
            self.add_new_line();
        } else {
            self.add_new_line();
            self.messages.push(Line::from(Span::styled(
                msgs.party_exit,
                Style::default()
                    .fg(Color::Rgb(180, 180, 180))
                    .add_modifier(Modifier::ITALIC),
            )));
            self.raw_messages.push(msgs.party_exit.to_string());
            self.add_new_line();
        }
    }
}
