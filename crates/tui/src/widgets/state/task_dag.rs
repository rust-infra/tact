//! Task dependency DAG → Mermaid → ratatui-markdown rendering.

use ratatui::text::Line;
use ratatui_markdown::markdown::MarkdownRenderer;
use tact_protocol::{TaskSnapshot, TaskStatusSnapshot};

use crate::{render::render_md::TuiRichTextTheme, theme::Theme};

/// Width used to pre-render the DAG before the popup's actual width is known
/// (the popup re-renders at its real width on the first frame).
pub(crate) const DEFAULT_DAG_RENDER_WIDTH: usize = 100;

/// Overlay popup holding the pre-rendered DAG lines.
#[derive(Debug, Clone)]
pub(crate) struct TaskDagPopup {
    pub lines: Vec<Line<'static>>,
    pub scroll: u16,
    /// Mermaid source (for copy).
    pub mermaid_source: String,
    /// Width the current `lines` were rendered for (re-render on change).
    pub render_width: usize,
}

/// Build a Mermaid `flowchart TD` from task snapshots (`blocks` edges).
pub(crate) fn tasks_to_mermaid(tasks: &[TaskSnapshot]) -> String {
    let mut out = String::from("flowchart TD\n");
    if tasks.is_empty() {
        out.push_str("  empty[\"(no tasks)\"]\n");
        return out;
    }
    for t in tasks {
        let label = node_label(t);
        out.push_str(&format!("  T{}[{}]\n", t.id, label));
    }
    for t in tasks {
        for &child in &t.blocks {
            if tasks.iter().any(|x| x.id == child) {
                out.push_str(&format!("  T{} --> T{}\n", t.id, child));
            }
        }
    }
    out
}

fn node_label(t: &TaskSnapshot) -> String {
    // Keep nodes narrow: status glyph + id only (subject overflows the popup).
    // No `[`/`]` in labels — ratatui-markdown's mermaid grammar ends `[...]`
    // node text at the first `]`, so `[x]`-style markers would break parsing.
    format!("{glyph} #{id}", glyph = status_glyph(t.status), id = t.id)
}

fn status_glyph(status: TaskStatusSnapshot) -> &'static str {
    match status {
        TaskStatusSnapshot::Pending => "○",
        TaskStatusSnapshot::InProgress => "◐",
        TaskStatusSnapshot::Completed => "✓",
    }
}

/// DAG markdown: heading + mermaid diagram + legend mapping ids to subjects
/// (node labels stay narrow, so the legend is where subjects are readable).
fn tasks_to_markdown(tasks: &[TaskSnapshot], source: &str) -> String {
    let mut out = String::from("## Tasks DAG\n\n```mermaid\n");
    out.push_str(source);
    out.push_str("```\n\n### Legend\n");
    for t in tasks {
        // Backticks in subjects would break inline code — flatten them.
        let subject = t.subject.replace('`', "'");
        out.push_str(&format!(
            "- `#{id}` {subject} — `{marker}`\n",
            id = t.id,
            subject = subject,
            marker = t.status.marker(),
        ));
    }
    out
}

/// Render the task DAG (mermaid diagram + legend) via ratatui-markdown.
pub(crate) fn render_task_dag_lines(
    tasks: &[TaskSnapshot],
    theme: &Theme,
    width: usize,
) -> (String, Vec<Line<'static>>) {
    let source = tasks_to_mermaid(tasks);
    if tasks.is_empty() {
        return (
            source,
            vec![
                Line::from("No tasks in this session yet."),
                Line::from("Create/update tasks first, then /tasks-dag again."),
            ],
        );
    }
    let md = tasks_to_markdown(tasks, &source);
    let renderer = MarkdownRenderer::new(width);
    let blocks = renderer.parse(&md);
    let lines = renderer.render(&blocks, &TuiRichTextTheme { theme });
    (source, lines)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::ThemeName;

    fn snap(id: u64, subject: &str, blocks: Vec<u64>) -> TaskSnapshot {
        TaskSnapshot {
            id,
            subject: subject.into(),
            status: TaskStatusSnapshot::Pending,
            owner: String::new(),
            blocks,
            blocked_by: Vec::new(),
            ..Default::default()
        }
    }

    #[test]
    fn mermaid_includes_nodes_and_edges() {
        let tasks = vec![
            snap(1, "root", vec![2, 3]),
            snap(2, "a", vec![]),
            snap(3, "b", vec![]),
        ];
        let src = tasks_to_mermaid(&tasks);
        assert!(src.contains("flowchart TD"), "{src}");
        assert!(src.contains("T1["), "{src}");
        assert!(src.contains("T1 --> T2"), "{src}");
        assert!(src.contains("T1 --> T3"), "{src}");
    }

    fn joined(lines: &[Line<'static>]) -> String {
        lines
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn ratatui_markdown_renders_diagram_and_legend() {
        let tasks = vec![snap(1, "root", vec![2]), snap(2, "child", vec![])];
        let theme = Theme::from(ThemeName::Dark);
        let (src, lines) = render_task_dag_lines(&tasks, &theme, 80);
        assert!(src.contains("T1 --> T2"));
        assert!(lines.len() > 1, "{lines:?}");
        let text = joined(&lines);
        assert!(
            text.contains('─') || text.contains('│'),
            "expected mermaid box art, got:\n{text}"
        );
        assert!(text.contains('#'), "expected node ids, got:\n{text}");
        // Legend maps ids back to subjects.
        assert!(
            text.contains("root") && text.contains("child"),
            "legend should list subjects, got:\n{text}"
        );
    }

    #[test]
    fn node_labels_avoid_status_markers() {
        let tasks = vec![snap(1, "root", vec![])];
        let src = tasks_to_mermaid(&tasks);
        // ratatui-markdown's mermaid grammar terminates `[...]` text at the
        // first `]`; `[x]`/`[ ]`-style status markers would break parsing.
        assert!(!src.contains("[x]"), "{src}");
        assert!(!src.contains("[>]"), "{src}");
        assert!(!src.contains("[ ]"), "{src}");
    }
}
