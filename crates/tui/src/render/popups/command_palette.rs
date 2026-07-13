use crate::widgets::state::App;
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, List, ListItem},
};

/// Map command name to emoji icon for palette display.
fn cmd_emoji(cmd: &str) -> &'static str {
    match cmd {
        "theme" => "🎨",
        "save" => "💾",
        "cancel" => "⏹",
        "quit" => "✕",
        "help" => "❓",
        "history" => "📜",
        "balance" => "💰",
        "lang" => "🌐",
        "model" => "🧠",
        _ => "⚡",
    }
}

/// Group commands into categories for visual separation.
fn cmd_category(cmd: &str) -> &'static str {
    match cmd {
        "save" | "cancel" | "quit" => "  Actions",
        "help" | "history" => "  Tools",
        "theme" | "lang" | "balance" | "model" => "  Settings",
        _ => "",
    }
}

pub(crate) fn render_command_palette(frame: &mut Frame, area: Rect, app: &App) {
    let filter = app.cmd_line.to_lowercase();
    let commands: Vec<_> = app.palette_commands().copied().collect();
    let filtered: Vec<(usize, &(&str, &str))> = commands
        .iter()
        .enumerate()
        .filter(|(_, (cmd, desc))| {
            filter.is_empty()
                || cmd.to_lowercase().contains(&filter)
                || desc.to_lowercase().contains(&filter)
        })
        .collect();

    let count = filtered.len().max(1) as u16;
    let popup_width = 48u16;
    let popup_height = count + 4 + 3; // extra space for category headers
    let popup_area = super::centered_list_popup_area(area, popup_width, popup_height);

    let inner = super::render_list_popup_chrome(
        frame,
        popup_area,
        app.msgs().palette_title.replace("{}", &app.cmd_line),
        app.theme.block_border_type(),
        app.theme.bottom_bar_bg,
    );

    let items: Vec<ListItem> = if filtered.is_empty() {
        vec![ListItem::new(Span::styled(
            app.msgs().palette_empty,
            Style::default().fg(Color::Gray),
        ))]
    } else {
        let selected = app.palette_selected.min(filtered.len().saturating_sub(1));
        let mut results: Vec<ListItem> = Vec::new();
        let mut last_cat = "";
        for (i, (_orig_idx, (cmd, _desc))) in filtered.iter().enumerate() {
            let cat = cmd_category(cmd);
            // Insert category separator
            if !cat.is_empty() && cat != last_cat {
                // Change the last_cat BEFORE using it
                // Use dimmed header for category
                if !results.is_empty() {
                    results.push(ListItem::new(Line::from(Span::styled(
                        cat,
                        Style::default()
                            .fg(Color::Rgb(100, 100, 120))
                            .add_modifier(ratatui::style::Modifier::DIM),
                    ))));
                }
                last_cat = cat;
            }

            let is_selected = i == selected;
            let emoji = cmd_emoji(cmd);
            let style = if is_selected {
                Style::default().bg(app.theme.highlight).fg(Color::White)
            } else {
                Style::default().fg(app.theme.fg)
            };
            let text = format!("  {}  {:<10} {}", emoji, cmd, app.localize_cmd_desc(cmd));
            results.push(ListItem::new(Span::styled(text, style)));
        }
        results
    };

    let list = List::new(items).block(Block::default());
    frame.render_widget(list, inner);
}
