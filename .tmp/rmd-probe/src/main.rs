use ratatui::style::Color;
use ratatui_markdown::markdown::MarkdownRenderer;
use ratatui_markdown::theme::{Generation, RichTextTheme};
use unicode_width::UnicodeWidthStr;

struct Theme;
impl RichTextTheme for Theme {
    fn generation(&self) -> Generation { Generation(1) }
    fn get_text_color(&self) -> Color { Color::White }
    fn get_muted_text_color(&self) -> Color { Color::Gray }
    fn get_primary_color(&self) -> Color { Color::Cyan }
    fn get_secondary_color(&self) -> Color { Color::Blue }
    fn get_info_color(&self) -> Color { Color::LightBlue }
    fn get_background_color(&self) -> Color { Color::Black }
    fn get_border_color(&self) -> Color { Color::DarkGray }
    fn get_focused_border_color(&self) -> Color { Color::White }
    fn get_popup_selected_background(&self) -> Color { Color::DarkGray }
    fn get_popup_selected_text_color(&self) -> Color { Color::White }
    fn get_json_key_color(&self) -> Color { Color::LightCyan }
    fn get_json_string_color(&self) -> Color { Color::Green }
    fn get_json_number_color(&self) -> Color { Color::Yellow }
    fn get_json_bool_color(&self) -> Color { Color::Magenta }
    fn get_json_null_color(&self) -> Color { Color::DarkGray }
    fn get_accent_yellow(&self) -> Color { Color::Yellow }
}

fn main() {
    let md = r#"| Skill | Description |
|-------|-------------|
| short | ok |
| superpowers:finishing-a-development-branch | Use when implementation is complete, all tests pass, and you need to decide how to integrate |
"#;
    let renderer = MarkdownRenderer::new(60);
    let blocks = renderer.parse(md);
    let lines = renderer.render(&blocks, &Theme);
    for line in &lines {
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        println!("{:3} | {}", UnicodeWidthStr::width(text.as_str()), text);
    }
}
