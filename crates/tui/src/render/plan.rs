use crate::widgets::state::{App, Status};
use ratatui::{Frame, layout::Rect};
use std::collections::HashSet;

/// Render the Execution Plan panel, showing step list, execution status, and selection highlight.
pub(crate) fn render_plan_panel(frame: &mut Frame, area: Rect, app: &mut App) {
    use ratatui::{
        style::{Modifier, Style},
        text::{Line, Span},
        widgets::{Block, Borders, List, ListItem},
    };
    // Under parallel tool execution, multiple steps can be running at once.
    // Highlight all active steps instead of a single `current_step`.
    let running_indices: HashSet<usize> = app
        .tools
        .active
        .iter()
        .filter_map(|active| {
            app.plan
                .steps
                .iter()
                .position(|step| step.tool_id == active.tool_id)
        })
        .collect();

    let items: Vec<ListItem> = app
        .plan
        .steps
        .iter()
        .enumerate()
        .map(|(i, step)| {
            let mut style = Style::default().fg(app.theme.fg);
            if matches!(app.status, Status::Executing { .. }) && running_indices.contains(&i) {
                style = style.fg(app.theme.warning).add_modifier(Modifier::BOLD);
            }
            let is_selected = app
                .mouse
                .plan_selection
                .map(|(s, e)| i >= s.min(e) && i <= s.max(e))
                .unwrap_or(false);
            if is_selected {
                style = style.add_modifier(Modifier::REVERSED);
            }
            let desc = step.description.clone();
            let prefix = if app.plan.collapsed[i] {
                "▶ "
            } else {
                "▼ "
            };
            let line = Line::from(Span::styled(
                format!("{}{}. {}", prefix, i + 1, desc),
                style,
            ));
            ListItem::new(line)
        })
        .collect();

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(app.theme.block_border_type())
                .border_style(Style::default().fg(app.theme.border))
                .title(app.msgs().plan_title)
                .style(Style::default().bg(app.theme.bg)),
        )
        .highlight_style(Style::default().bg(app.theme.highlight));
    let mut state = app.plan.list_state;
    frame.render_stateful_widget(list, area, &mut state);
    app.plan.list_state = state;
}
