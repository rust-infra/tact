use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Style},
    text::Span,
};

use agent_tui_kit::widgets::list_popup::{ListPopup, ListRow, SelectionStyle};

use crate::widgets::state::{App, InputMode};

/// Render a centered file-picker popup listing files under the project root.
pub(crate) fn render_file_picker(frame: &mut Frame, area: Rect, app: &mut App) {
    if app.input_mode != InputMode::FilePicker {
        // Called every frame; the hit area recorded while active must not
        // outlive the popup (see `render_select_popup`).
        app.mouse.file_picker_popup_area = Rect::default();
        return;
    }

    let count = app.file_picker.options.len().max(1) as u16;
    // Reserve one extra row for the query/filter display.
    let popup_width = 50u16.min(area.width.saturating_sub(4));
    let popup_height = (count + 5).min(area.height.saturating_sub(4));

    let rel_dir = app
        .file_picker
        .current_dir
        .strip_prefix(&app.file_picker.base_dir)
        .unwrap_or(app.file_picker.current_dir.as_path())
        .to_string_lossy()
        .to_string();
    let title = if app.file_picker.query.is_empty() {
        format!("{}: {}", app.msgs().file_picker_title, rel_dir)
    } else {
        format!(
            "{}: {} /{}",
            app.msgs().file_picker_title,
            rel_dir,
            app.file_picker.query
        )
    };

    let selected = app
        .file_picker
        .selected
        .min(app.file_picker.options.len().saturating_sub(1));
    let rows: Vec<ListRow<'_>> = app
        .file_picker
        .options
        .iter()
        .enumerate()
        .map(|(i, opt)| {
            // Focus decides the marker only; the component owns the focused
            // row's colors, so the span keeps its type color either way.
            let is_selected = i == selected;
            let (icon, path_display) = if opt.ends_with('/') {
                ("\u{f114} ", opt.trim_end_matches('/'))
            } else {
                ("\u{f15b} ", opt.as_str())
            };
            let prefix = if is_selected {
                format!("{} {}", app.msgs().select_arrow, icon)
            } else {
                format!("  {}", icon)
            };

            // Color by type: folders use accent, files use extension color.
            let fg = if opt.ends_with('/') {
                app.theme.accent
            } else {
                let ext = opt.rsplit('.').next().unwrap_or("");
                match ext {
                    "rs" => Color::Rgb(239, 146, 65),
                    "py" => Color::Rgb(55, 118, 171),
                    "js" | "ts" | "tsx" | "jsx" => Color::Rgb(247, 223, 30),
                    "md" => Color::Rgb(66, 133, 244),
                    "toml" | "yaml" | "yml" | "json" => Color::Rgb(108, 192, 128),
                    "css" | "scss" => Color::Rgb(214, 79, 148),
                    "html" => Color::Rgb(228, 105, 55),
                    _ => app.theme.fg,
                }
            };

            ListRow::item(vec![Span::styled(
                format!("{prefix}{path_display}"),
                Style::default().fg(fg),
            )])
        })
        .collect();

    let popup = ListPopup::new(&app.theme, &rows, popup_width, popup_height)
        .title(title)
        .selected_row(selected)
        .empty_text(app.msgs().select_empty)
        .selection(SelectionStyle::Highlight)
        .bg(Some(app.theme.bottom_bar_bg));

    app.mouse.file_picker_popup_area = popup.layout(area).popup_area;
    frame.render_widget(popup, area);
}
