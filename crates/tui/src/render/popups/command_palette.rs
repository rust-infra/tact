use ratatui::{Frame, layout::Rect, style::Style, text::Span};

use agent_tui_kit::widgets::list_popup::{ListPopup, ListRow, SelectionStyle};

use crate::widgets::state::{App, InputMode, SurfaceId};

/// Map command name to emoji icon for palette display.
fn cmd_emoji(cmd: &str, is_skill: bool) -> &'static str {
    if is_skill {
        return "🎯";
    }
    match cmd {
        "theme" => "🎨",
        "save" => "💾",
        "cancel" => "⏹",
        "subagent_cancel" => "⏹",
        "quit" => "✕",
        "help" => "❓",
        "history" => "📜",
        "balance" => "💰",
        "lang" => "🌐",
        "model" => "🧠",
        "skill" => "📋",
        "plugin" => "🧩",
        "background" => "🖥",
        _ => "⚡",
    }
}

/// Group commands into categories for visual separation.
fn cmd_category(cmd: &str, is_skill: bool) -> &'static str {
    if is_skill {
        return "  Skills";
    }
    match cmd {
        "save" | "cancel" | "subagent_cancel" | "quit" => "  Actions",
        "help" | "history" | "skill" | "plugin" | "background" => "  Tools",
        "theme" | "lang" | "balance" | "model" => "  Settings",
        _ => "",
    }
}

fn is_skill_cmd(app: &App, cmd: &str) -> bool {
    app.skills_data.iter().any(|s| s.name == cmd)
}

pub(crate) fn render_command_palette(frame: &mut Frame, area: Rect, app: &mut App) {
    if app.input_mode != InputMode::Palette {
        // Called every frame; the hit area recorded while active must not
        // outlive the popup (see `render_select_popup`).
        app.mouse.clear_area(SurfaceId::PalettePopup);
        return;
    }

    let commands = app.palette_commands();
    let filtered: Vec<&(String, String)> = app
        .palette_filtered()
        .into_iter()
        .map(|i| &commands[i])
        .collect();

    let msgs = app.msgs();
    let count = filtered.len().max(1) as u16;

    // Dynamic width: 60% of terminal, clamped to [60, 120]
    let popup_width = ((area.width as f32 * 0.60) as u16).clamp(60, 120);
    // Inner width after block borders.
    let inner_width = popup_width.saturating_sub(2) as usize;

    let popup_height = (count + 6).min(area.height.saturating_sub(4)); // cap to not exceed terminal

    // Rows are built item by item with a category header in front of the first
    // command of each group. Headers are ordinary rows as far as the scroll
    // window is concerned, so the focused item's *row* index is tracked
    // alongside its item index.
    let selected = app.palette_selected.min(filtered.len().saturating_sub(1));
    let mut rows: Vec<ListRow<'_>> = Vec::new();
    let mut selected_row = 0;
    let mut last_cat = "";
    for (i, (cmd, desc)) in filtered.iter().enumerate() {
        let skill = is_skill_cmd(app, cmd);
        let cat = cmd_category(cmd, skill);
        if !cat.is_empty() && cat != last_cat {
            if !rows.is_empty() || skill {
                rows.push(ListRow::header(vec![Span::styled(
                    cat,
                    Style::default()
                        .fg(app.theme.muted)
                        .add_modifier(ratatui::style::Modifier::DIM),
                )]));
            }
            last_cat = cat;
        }

        if i == selected {
            selected_row = rows.len();
        }

        // Row format: "  {emoji}  {cmd:<14} {desc}"
        // Overhead: "  " (2) + emoji (~2) + "  " (2) + cmd_pad + " " (1)
        let cmd_width = cmd.chars().count().max(14);
        let reserved = 2 + 2 + 2 + cmd_width + 1; // spaces + emoji + spaces + cmd + space
        let max_desc = inner_width.saturating_sub(reserved).max(5);
        let desc_short = truncate_chars(desc, max_desc);
        let emoji = cmd_emoji(cmd, skill);
        rows.push(ListRow::item(vec![Span::styled(
            format!("  {emoji}  {cmd:<14} {desc_short}"),
            Style::default().fg(app.theme.fg),
        )]));
    }

    let title = msgs.palette_title.replace("{}", &app.cmd_line);
    let popup = ListPopup::new(&app.theme, &rows, popup_width, popup_height)
        .title(title)
        .selected_row(selected_row)
        .empty_text(msgs.palette_empty)
        .selection(SelectionStyle::Highlight)
        .bg(Some(app.theme.bottom_bar_bg));

    app.mouse
        .set_area(SurfaceId::PalettePopup, popup.layout(area).popup_area);
    frame.render_widget(popup, area);
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
