use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::Span,
    widgets::BorderType,
};

use agent_tui_kit::widgets::list_popup::{ListPopup, ListRow, SelectionStyle};

use crate::widgets::state::{App, Candidate, SurfaceId};

/// Reserved overhead per item row: prefix("▶ "|"  ") 2 + "/" 1 + "  " separator 2.
const ROW_OVERHEAD: usize = 5;

pub(crate) fn render_slash_command_popup(frame: &mut Frame, area: Rect, app: &mut App) {
    if !app.slash_command.active {
        app.mouse.clear_area(SurfaceId::SlashPopup);
        return;
    }

    // ----- dynamic sizing based on terminal dimensions -----
    // Width: 60% of terminal width, clamped to [60, 120]
    let popup_width = ((area.width as f32 * 0.60) as u16).clamp(60, 120);
    // Visible rows: ~45% of terminal height, clamped to [6, 22]
    let max_visible = ((area.height as f32 * 0.45) as usize).clamp(6, 22);
    // Inner width after block borders (left + right = 2)
    let inner_width = popup_width.saturating_sub(2) as usize;
    // -----

    let msgs = app.msgs();
    // Theme, not literals: this popup used to draw cyan/white/dark-gray, which
    // on a light theme is white items on the white popup background — the whole
    // list, invisible.
    let theme = app.theme;
    let candidates = app.slash_candidates();
    if candidates.is_empty() {
        app.mouse.clear_area(SurfaceId::SlashPopup);
        let hint = ListPopup::new(&theme, &[], 40, 5)
            .title(format!(
                "{}{}",
                msgs.slash_title_mixed, msgs.popup_close_hint
            ))
            .empty_text(msgs.palette_empty)
            .bg(Some(theme.bottom_bar_bg));
        frame.render_widget(hint, area);
        return;
    }

    let selected = app.slash_command.selected.min(candidates.len() - 1);

    // Build rows with section headers (headers are not selectable). A header
    // occupies a window row, so the focused item's *row* index is tracked
    // alongside its item index.
    let mut rows: Vec<ListRow<'static>> = Vec::new();
    let mut selected_row = 0;
    let mut last_section: Option<Section> = None;
    // Subcommand rows are a table (`/plugin uninstall  <name>`) and read better
    // with their syntax column lined up; the command list is not (names there
    // already reach the widest point).
    let hint_column = candidates
        .iter()
        .all(|candidate| candidate.path.contains(' '))
        .then(|| {
            candidates
                .iter()
                .map(|candidate| candidate.path.chars().count())
                .max()
                .unwrap_or(0)
        });

    for (i, candidate) in candidates.iter().enumerate() {
        let section = if candidate.is_skill {
            Section::Skills
        } else {
            Section::Commands
        };
        if last_section != Some(section) {
            let label = match section {
                Section::Commands => msgs.slash_section_commands,
                Section::Skills => msgs.slash_section_skills,
            };
            rows.push(ListRow::header(vec![Span::styled(
                format!(" {label}"),
                Style::default()
                    .fg(theme.muted)
                    .add_modifier(Modifier::DIM | Modifier::BOLD),
            )]));
            last_section = Some(section);
        }

        if i == selected {
            selected_row = rows.len();
        }
        rows.push(ListRow::item(slash_item_spans(
            candidate,
            i == selected,
            hint_column,
            inner_width,
            &theme,
        )));
    }

    // The popup can show at most `max_visible` items plus up to two section
    // headers — but never more rows than the main area actually fits (the
    // block borders take 2 rows). The scroll window and the popup height MUST
    // agree: when the window is larger than the real content area, the widget
    // clips the bottom rows and the selected row can land outside the visible
    // content, which reads as "the list does not scroll".
    let available_content = (area.height.saturating_sub(2)) as usize;
    let window = rows
        .len()
        .min(max_visible + 2)
        .min(available_content.max(1));

    let title = match last_section {
        Some(Section::Skills) if candidates.iter().all(|c| c.is_skill) => msgs.slash_section_skills,
        Some(Section::Commands) if candidates.iter().all(|c| !c.is_skill) => {
            msgs.slash_section_commands
        }
        _ => msgs.slash_title_mixed,
    };

    let popup = ListPopup::new(&theme, &rows, popup_width, window as u16 + 2)
        .title(Span::styled(
            format!("{title}{}", msgs.popup_close_hint),
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ))
        .selected_row(selected_row)
        // The focused row is drawn in `theme.accent` with no band, so the
        // caller keeps its own spans — see `SelectionStyle::CallerStyled`.
        .selection(SelectionStyle::CallerStyled)
        .border_type(BorderType::Rounded)
        .accent_border(true)
        .bg(Some(theme.bottom_bar_bg));

    // Expose the popup rect so mouse-wheel scrolls over the list move the
    // selection instead of scrolling the log behind the popup.
    app.mouse
        .set_area(SurfaceId::SlashPopup, popup.layout(area).popup_area);
    frame.render_widget(popup, area);
}

/// One item row: the focused row is the accent, the detail column stays muted.
fn slash_item_spans(
    candidate: &Candidate,
    is_sel: bool,
    hint_column: Option<usize>,
    inner_width: usize,
    theme: &agent_tui_kit::theme::Theme,
) -> Vec<Span<'static>> {
    let prefix = if is_sel { "▶ " } else { "  " };
    // Calculate available width for the right column: inner_width minus
    // overhead minus the completed path's length.
    let path_width = hint_column.unwrap_or_else(|| candidate.path.chars().count());
    let max_desc = inner_width.saturating_sub(ROW_OVERHEAD + path_width).max(5);
    let desc_short = truncate_chars(&candidate.detail, max_desc);
    // Pad the path so every detail column starts at the same x, but only where
    // there is a column to align (see `hint_column`).
    let left = match hint_column {
        Some(width) => format!(
            "{prefix}/{}{}",
            candidate.path,
            " ".repeat(width - candidate.path.chars().count() + 2)
        ),
        None => format!("{prefix}/{}  ", candidate.path),
    };
    vec![
        Span::styled(
            left,
            if is_sel {
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.fg)
            },
        ),
        Span::styled(desc_short, Style::default().fg(theme.muted)),
    ]
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Section {
    Commands,
    Skills,
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
