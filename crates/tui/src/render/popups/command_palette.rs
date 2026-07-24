use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, List, ListItem},
};

use crate::widgets::state::App;

/// Map command name to emoji icon for palette display.
fn cmd_emoji(cmd: &str, is_skill: bool) -> &'static str {
    if is_skill {
        return "🎯";
    }
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
        "skills" => "📋",
        "skill-reload" => "🔄",
        "plugin" => "🧩",
        _ => "⚡",
    }
}

/// Group commands into categories for visual separation.
fn cmd_category(cmd: &str, is_skill: bool) -> &'static str {
    if is_skill {
        return "  Skills";
    }
    match cmd {
        "save" | "cancel" | "quit" => "  Actions",
        "help" | "history" | "skills" | "skill-reload" | "plugin" => "  Tools",
        "theme" | "lang" | "balance" | "model" => "  Settings",
        _ => "",
    }
}

fn is_skill_cmd(app: &App, cmd: &str) -> bool {
    app.skills_data.iter().any(|s| s.name == cmd)
}

pub(crate) fn render_command_palette(frame: &mut Frame, area: Rect, app: &App) {
    let filter = app.cmd_line.to_lowercase();
    let commands = app.palette_commands();
    let filtered: Vec<(usize, &(String, String))> = commands
        .iter()
        .enumerate()
        .filter(|(_, (cmd, desc))| {
            filter.is_empty()
                || cmd.to_lowercase().contains(&filter)
                || desc.to_lowercase().contains(&filter)
        })
        .collect();

    let msgs = app.msgs();
    let count = filtered.len().max(1) as u16;

    // Dynamic width: 60% of terminal, clamped to [60, 120]
    let popup_width = ((area.width as f32 * 0.60) as u16).clamp(60, 120);
    // Inner width after block borders (returned by render_list_popup_chrome)
    let inner_width = popup_width.saturating_sub(2) as usize;

    let popup_height = (count + 6).min(area.height.saturating_sub(4)); // cap to not exceed terminal
    let popup_area = super::centered_list_popup_area(area, popup_width, popup_height);

    let inner = super::render_list_popup_chrome(
        frame,
        popup_area,
        msgs.palette_title.replace("{}", &app.cmd_line),
        app.theme.block_border_type(),
        app.theme.bottom_bar_bg,
    );

    let items: Vec<ListItem> = if filtered.is_empty() {
        vec![ListItem::new(Span::styled(
            msgs.palette_empty,
            Style::default().fg(Color::Gray),
        ))]
    } else {
        let selected = app.palette_selected.min(filtered.len().saturating_sub(1));
        let mut results: Vec<ListItem> = Vec::new();
        let mut last_cat = "";
        for (i, (_orig_idx, (cmd, desc))) in filtered.iter().enumerate() {
            let skill = is_skill_cmd(app, cmd);
            let cat = cmd_category(cmd, skill);
            if !cat.is_empty() && cat != last_cat {
                if !results.is_empty() || skill {
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
            let emoji = cmd_emoji(cmd, skill);
            let style = if is_selected {
                Style::default().bg(app.theme.highlight).fg(Color::White)
            } else {
                Style::default().fg(app.theme.fg)
            };
            // Calculate available width for description
            // Row format: "  {emoji}  {cmd:<14} {desc}"
            // Overhead: "  " (2) + emoji (~2) + "  " (2) + cmd_pad + " " (1)
            let cmd_width = cmd.chars().count().max(14);
            let reserved = 2 + 2 + 2 + cmd_width + 1; // spaces + emoji + spaces + cmd + space
            let max_desc = inner_width.saturating_sub(reserved).max(5);
            let desc_short = truncate_chars(desc, max_desc);
            let text = format!("  {emoji}  {cmd:<14} {desc_short}");
            results.push(ListItem::new(Span::styled(text, style)));
        }
        results
    };

    let list = List::new(items).block(Block::default());
    frame.render_widget(list, inner);
}

fn truncate_chars(s: &str, max: usize) -> String {
    let count = s.chars().count();
    if count <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}
