use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Style},
    text::Span,
    widgets::{Block, List, ListItem},
};

use crate::widgets::state::App;

/// Render a centered file-picker popup listing files under the project root.
pub(crate) fn render_file_picker(frame: &mut Frame, area: Rect, app: &App) {
    let count = app.file_picker.options.len().max(1) as u16;
    // Reserve one extra row for the query/filter display.
    let popup_width = 50u16.min(area.width.saturating_sub(4));
    let popup_height = (count + 5).min(area.height.saturating_sub(4));
    let popup_area = super::centered_list_popup_area(area, popup_width, popup_height);

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
        format!("{}: {} /{}", app.msgs().file_picker_title, rel_dir, app.file_picker.query)
    };
    let inner = super::render_list_popup_chrome(
        frame,
        popup_area,
        title,
        app.theme.block_border_type(),
        app.theme.bottom_bar_bg,
    );

    let items: Vec<ListItem> = if app.file_picker.options.is_empty() {
        vec![ListItem::new(Span::styled(app.msgs().select_empty, Style::default().fg(Color::Gray)))]
    } else {
        let selected = app.file_picker.selected.min(app.file_picker.options.len().saturating_sub(1));
        app.file_picker
            .options
            .iter()
            .enumerate()
            .map(|(i, opt)| {
                let is_selected = i == selected;

                // Determine icon: folder or file
                let (icon, path_display) = if opt.ends_with('/') {
                    ("\u{f114} ", opt.trim_end_matches('/'))
                } else {
                    ("\u{f15b} ", opt.as_str())
                };

                let prefix =
                    if is_selected { format!("{} {}", app.msgs().select_arrow, icon) } else { format!("  {}", icon) };

                let fg = if is_selected {
                    Color::White
                } else {
                    // Color by type: folders use accent, files use extension color
                    if opt.ends_with('/') {
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
                    }
                };

                let style =
                    if is_selected { Style::default().bg(app.theme.highlight).fg(fg) } else { Style::default().fg(fg) };

                ListItem::new(Span::styled(format!("{}{}", prefix, path_display), style))
            })
            .collect()
    };

    let list = List::new(items).block(Block::default());
    frame.render_widget(list, inner);
}
