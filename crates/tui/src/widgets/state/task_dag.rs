//! Task dependency DAG → Mermaid → terminal text (meraid).

use tact_protocol::TaskSnapshot;

use meraid::theme::ThemeType;

/// Overlay popup holding pre-rendered DAG lines.
#[derive(Debug, Clone)]
pub(crate) struct TaskDagPopup {
    pub lines: Vec<String>,
    pub scroll: u16,
    /// Mermaid source (for copy).
    pub mermaid_source: String,
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
        out.push_str(&format!("  T{}[\"{}\"]\n", t.id, label));
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
    // Keep nodes narrow: status marker + id only (subject overflows the popup).
    format!("{marker} #{id}", marker = t.status.marker(), id = t.id)
}

/// Render tasks to Unicode DAG lines via meraid (Mono, no ANSI).
pub(crate) fn render_task_dag_lines(tasks: &[TaskSnapshot]) -> (String, Vec<String>) {
    let source = tasks_to_mermaid(tasks);
    if tasks.is_empty() {
        return (
            source,
            vec![
                "No tasks in this session yet.".into(),
                "Create/update tasks first, then /tasks-dag again.".into(),
            ],
        );
    }
    match meraid::render(&source, ThemeType::Mono) {
        Ok(text) => {
            let lines: Vec<String> = text.lines().map(str::to_string).collect();
            if lines.is_empty() {
                (source, vec!["(empty render)".into()])
            } else {
                (source, lines)
            }
        }
        Err(err) => (
            source.clone(),
            vec![
                format!("meraid render failed: {err}"),
                String::new(),
                "Mermaid source:".into(),
            ]
            .into_iter()
            .chain(source.lines().map(str::to_string))
            .collect(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tact_protocol::TaskStatusSnapshot;

    fn snap(id: u64, subject: &str, blocks: Vec<u64>) -> TaskSnapshot {
        TaskSnapshot {
            id,
            subject: subject.into(),
            status: TaskStatusSnapshot::Pending,
            owner: String::new(),
            blocks,
            blocked_by: Vec::new(),
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

    #[test]
    fn meraid_renders_non_empty_unicode() {
        let tasks = vec![
            snap(1, "root", vec![2]),
            snap(2, "child", vec![]),
        ];
        let (src, lines) = render_task_dag_lines(&tasks);
        assert!(src.contains("T1 --> T2"));
        assert!(lines.len() > 1, "{lines:?}");
        let joined = lines.join("\n");
        assert!(
            joined.contains('─') || joined.contains('-') || joined.contains('│'),
            "expected box art, got:\n{joined}"
        );
    }
}
