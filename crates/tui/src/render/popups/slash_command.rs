use crate::widgets::state::App;
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, List, ListItem},
};

pub(crate) fn render_slash_command_popup(frame: &mut Frame, area: Rect, app: &App) {
    let slash = &app.slash_command;
    if !slash.active {
        return;
    }

    let msgs = app.msgs();
    let cmds = app.palette_commands();
    let commands: Vec<(&str, &str)> = cmds.iter().map(|(c, d)| (c.as_str(), d.as_str())).collect();
    let skill_names: std::collections::HashSet<&str> =
        app.skills_data.iter().map(|s| s.name.as_str()).collect();
    let filtered = slash.matched_commands(&app.input, app.input_cursor, &commands, &skill_names);
    let n = filtered.len();
    if n == 0 {
        let hint_area = super::centered_list_popup_area(area, 30, 5);
        let hint_block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(Color::DarkGray));
        frame.render_widget(Clear, hint_area);
        frame.render_widget(&hint_block, hint_area);
        let inner = hint_block.inner(hint_area);
        frame.buffer_mut().set_line(
            inner.x,
            inner.y + 1,
            &Line::from(Span::styled(
                "No matching command",
                Style::default().fg(Color::Gray),
            )),
            inner.width,
        );
        return;
    }

    let selected = slash.selected.min(n.saturating_sub(1));
    let max_visible = 10usize;

    // Build rows with section headers (headers are not selectable).
    let mut rows: Vec<SlashRow<'_>> = Vec::new();
    let mut last_section: Option<Section> = None;
    for (i, &(_idx, (cmd, desc), _score)) in filtered.iter().enumerate() {
        let section = if skill_names.contains(cmd) {
            Section::Skills
        } else {
            Section::Commands
        };
        if last_section != Some(section) {
            rows.push(SlashRow::Header(section));
            last_section = Some(section);
        }
        rows.push(SlashRow::Item {
            global_idx: i,
            cmd,
            desc,
        });
    }

    // Map selected command index → row index for scroll anchoring.
    let selected_row = rows
        .iter()
        .position(|r| matches!(r, SlashRow::Item { global_idx, .. } if *global_idx == selected))
        .unwrap_or(0);

    let list_height = rows.len().min(max_visible + 2) as u16;
    let popup_width: u16 = 56;
    let popup_area = super::centered_list_popup_area(area, popup_width, list_height + 2);

    let offset = if rows.len() > max_visible + 2 && selected_row >= max_visible {
        selected_row - max_visible + 1
    } else {
        0
    };

    let accent = Color::Cyan;
    let visible_end = (offset + max_visible + 2).min(rows.len());
    let items: Vec<ListItem> = rows[offset..visible_end]
        .iter()
        .map(|row| match row {
            SlashRow::Header(section) => {
                let label = match section {
                    Section::Commands => msgs.slash_section_commands,
                    Section::Skills => msgs.slash_section_skills,
                };
                ListItem::new(Line::from(Span::styled(
                    format!(" {label}"),
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::DIM | Modifier::BOLD),
                )))
            }
            SlashRow::Item {
                global_idx,
                cmd,
                desc,
            } => {
                let is_sel = *global_idx == selected;
                let prefix = if is_sel { "▶ " } else { "  " };
                let desc_short = truncate_chars(desc, 28);
                ListItem::new(Line::from(vec![
                    Span::styled(
                        format!("{prefix}/{cmd}"),
                        if is_sel {
                            Style::default().fg(accent).add_modifier(Modifier::BOLD)
                        } else {
                            Style::default().fg(Color::White)
                        },
                    ),
                    Span::raw("  "),
                    Span::styled(desc_short, Style::default().fg(Color::DarkGray)),
                ]))
            }
        })
        .collect();

    let title = match last_section {
        Some(Section::Skills) if filtered.iter().all(|(_, (c, _), _)| skill_names.contains(*c)) => {
            msgs.slash_section_skills
        }
        _ => "Commands / Skills",
    };

    let block = Block::default()
        .title(Span::styled(
            title,
            Style::default().fg(accent).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(accent));

    let list = List::new(items).block(block);

    frame.render_widget(Clear, popup_area);
    frame.render_widget(list, popup_area);
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Section {
    Commands,
    Skills,
}

enum SlashRow<'a> {
    Header(Section),
    Item {
        global_idx: usize,
        cmd: &'a str,
        desc: &'a str,
    },
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
