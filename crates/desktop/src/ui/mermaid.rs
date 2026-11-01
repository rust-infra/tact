//! A small Mermaid flowchart adapter.
//!
//! Assistant answers frequently carry a ```` ```mermaid ```` flowchart. The
//! transcript used to show that fence as source. This module parses the
//! subset agents actually write — `graph`/`flowchart` with `A[Label] --> B`
//! edges — and lays the nodes out in layers so the diagram reads as a
//! diagram. Anything outside the subset is ignored, and the original fence
//! remains available in the assistant's Markdown.

use std::collections::HashMap;

use gpui::{
    AnyElement, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, div, px,
};
use gpui_component::{h_flex, v_flex};

/// Flow direction from the diagram header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    TopDown,
    LeftRight,
}

/// The outline a node was written with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    Rectangle,
    Rounded,
    Diamond,
    Stadium,
}

/// One flowchart node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    pub id: String,
    pub label: String,
    pub shape: Shape,
}

/// One directed edge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edge {
    pub from: String,
    pub to: String,
    pub label: Option<String>,
}

/// A parsed flowchart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagram {
    pub direction: Direction,
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
}

impl Diagram {
    /// Nodes grouped by their longest-path depth from a root.
    pub fn layers(&self) -> Vec<Vec<usize>> {
        let mut depth: HashMap<&str, usize> = HashMap::new();
        for node in &self.nodes {
            depth.insert(node.id.as_str(), 0);
        }
        // Relax until stable; the node count bounds the longest acyclic path
        // and a cycle simply stops moving.
        for _ in 0..=self.nodes.len() {
            let mut changed = false;
            for edge in &self.edges {
                let from = depth.get(edge.from.as_str()).copied().unwrap_or(0);
                let to = depth.entry(edge.to.as_str()).or_insert(0);
                if *to < from + 1 {
                    *to = from + 1;
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        let deepest = depth.values().copied().max().unwrap_or(0);
        let mut layers = vec![Vec::new(); deepest + 1];
        for (index, node) in self.nodes.iter().enumerate() {
            let level = depth.get(node.id.as_str()).copied().unwrap_or(0);
            layers[level].push(index);
        }
        layers
    }
}

/// Extracts every fenced Mermaid block from Markdown.
pub fn blocks(markdown: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut current: Option<Vec<&str>> = None;
    for line in markdown.lines() {
        let trimmed = line.trim();
        match current.as_mut() {
            Some(lines) => {
                if trimmed.starts_with("```") {
                    blocks.push(lines.join("\n"));
                    current = None;
                } else {
                    lines.push(line);
                }
            }
            None => {
                if trimmed.eq_ignore_ascii_case("```mermaid") {
                    current = Some(Vec::new());
                }
            }
        }
    }
    blocks
}

/// Removes fenced Mermaid blocks the transcript renders as diagrams.
///
/// A fence whose source is not a supported diagram stays in the prose, so the
/// reader still sees the original source as a readable code block.
pub fn strip_blocks(markdown: &str) -> String {
    let mut out = String::new();
    let mut fence: Option<Vec<&str>> = None;
    for line in markdown.lines() {
        let trimmed = line.trim();
        match fence.as_mut() {
            Some(lines) => {
                if trimmed.starts_with("```") {
                    let source = lines.join("\n");
                    if parse(&source).is_none() {
                        out.push_str("```mermaid\n");
                        out.push_str(&source);
                        out.push_str("\n```\n");
                    }
                    fence = None;
                } else {
                    lines.push(line);
                }
            }
            None => {
                if trimmed.eq_ignore_ascii_case("```mermaid") {
                    fence = Some(Vec::new());
                } else {
                    out.push_str(line);
                    out.push('\n');
                }
            }
        }
    }
    if let Some(lines) = fence {
        out.push_str("```mermaid\n");
        out.push_str(&lines.join("\n"));
        out.push_str("\n```\n");
    }
    out.trim_end().to_string()
}

/// Parses one Mermaid flowchart source.
pub fn parse(source: &str) -> Option<Diagram> {
    let mut direction = None;
    let mut nodes: Vec<Node> = Vec::new();
    let mut edges = Vec::new();
    for raw in source.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with("%%") {
            continue;
        }
        if direction.is_none() {
            let mut parts = line.split_whitespace();
            if matches!(parts.next(), Some("graph" | "flowchart")) {
                direction = Some(match parts.next().unwrap_or("TD") {
                    "LR" | "RL" => Direction::LeftRight,
                    _ => Direction::TopDown,
                });
                continue;
            }
            return None;
        }
        let lower = line.to_ascii_lowercase();
        if lower.starts_with("subgraph")
            || lower == "end"
            || lower.starts_with("classdef")
            || lower.starts_with("class ")
            || lower.starts_with("style ")
            || lower.starts_with("linkstyle")
            || lower.starts_with("click ")
        {
            continue;
        }
        parse_chain(line, &mut nodes, &mut edges);
    }
    let direction = direction?;
    if nodes.is_empty() {
        return None;
    }
    Some(Diagram {
        direction,
        nodes,
        edges,
    })
}

/// Splits `A --> B --> C` into node segments and edges.
fn parse_chain(line: &str, nodes: &mut Vec<Node>, edges: &mut Vec<Edge>) {
    let mut rest = line.trim();
    let mut previous: Option<String> = None;
    loop {
        let Some((arrow_start, arrow_end, _)) = find_arrow(rest) else {
            if let Some((id, label, shape)) = parse_node(rest) {
                push_node(nodes, &id, label, shape);
                if let Some(from) = previous.take() {
                    edges.push(Edge {
                        from,
                        to: id,
                        label: None,
                    });
                }
            }
            break;
        };
        if previous.is_none()
            && let Some((id, label, shape)) = parse_node(&rest[..arrow_start])
        {
            push_node(nodes, &id, label, shape);
            previous = Some(id);
        }
        let mut tail = rest[arrow_end..].trim_start();
        let mut edge_label = None;
        if tail.starts_with('|')
            && let Some(close) = tail[1..].find('|')
        {
            edge_label = Some(tail[1..1 + close].trim().to_string());
            tail = tail[1 + close + 1..].trim_start();
        }
        let target_end = find_arrow(tail)
            .map(|(start, ..)| start)
            .unwrap_or(tail.len());
        if let Some((id, label, shape)) = parse_node(&tail[..target_end]) {
            push_node(nodes, &id, label, shape);
            if let Some(from) = previous.take() {
                edges.push(Edge {
                    from,
                    to: id.clone(),
                    label: edge_label,
                });
            }
            previous = Some(id);
        }
        rest = &tail[target_end..];
        if rest.trim().is_empty() {
            break;
        }
    }
}

/// Finds the next edge arrow and returns its byte range and text.
fn find_arrow(text: &str) -> Option<(usize, usize, &str)> {
    for (index, _) in text.char_indices() {
        let window = &text[index..];
        for arrow in ["-.->", "==>", "-->", "---", "--x", "--o"] {
            if window.starts_with(arrow) {
                return Some((index, index + arrow.len(), arrow));
            }
        }
    }
    None
}

/// Parses `id`, `id[Label]`, `id(Label)`, `id{Label}` or `id((Label))`.
fn parse_node(segment: &str) -> Option<(String, String, Shape)> {
    let segment = segment.trim();
    if segment.is_empty() {
        return None;
    }
    let Some(open) = segment.find(['[', '(', '{', '>']) else {
        if segment.chars().any(char::is_whitespace) {
            return None;
        }
        return Some((segment.to_string(), segment.to_string(), Shape::Rectangle));
    };
    let id = segment[..open].trim();
    if id.is_empty() || id.contains(' ') {
        return None;
    }
    let close = match segment.as_bytes()[open] {
        b'[' => ']',
        b'(' => ')',
        b'{' => '}',
        _ => ']',
    };
    let end = segment.rfind(close)?;
    if end <= open {
        return None;
    }
    let label = segment[open + 1..end].trim().trim_matches('"');
    let shape = match segment.as_bytes()[open] {
        b'(' if segment[open..].starts_with("((") => Shape::Stadium,
        b'(' => Shape::Rounded,
        b'{' => Shape::Diamond,
        _ => Shape::Rectangle,
    };
    Some((
        id.to_string(),
        if label.is_empty() {
            id.to_string()
        } else {
            label.to_string()
        },
        shape,
    ))
}

fn push_node(nodes: &mut Vec<Node>, id: &str, label: String, shape: Shape) {
    if let Some(existing) = nodes.iter_mut().find(|node| node.id == id) {
        // A later bare reference (`B --> C`) must not erase the shape or
        // label an earlier declaration gave the node.
        if label != id || shape != Shape::Rectangle {
            existing.label = label;
            existing.shape = shape;
        }
    } else {
        nodes.push(Node {
            id: id.to_string(),
            label,
            shape,
        });
    }
}

/// Renders one diagram with the design's compact mono scale.
pub fn element(id: impl Into<SharedString>, diagram: &Diagram, dark: bool) -> AnyElement {
    let id = id.into();
    let layers = diagram.layers();
    let border = if dark { 0x3a3a3f } else { 0xd8d8dd };
    let surface = if dark { 0x1c1c1f } else { 0xffffff };
    let ink = if dark { 0xd8d8dc } else { 0x1c1c1e };
    let muted = if dark { 0x8e8e93 } else { 0x6e6e73 };

    // A left-to-right flow lays its layers out as columns; a top-down flow
    // stacks them, with each layer's siblings side by side.
    let mut body = if diagram.direction == Direction::LeftRight {
        h_flex().flex_wrap().items_center().gap_2()
    } else {
        v_flex().items_start().gap_1()
    };

    for (layer_index, layer) in layers.iter().enumerate() {
        let mut nodes = h_flex().flex_wrap().items_center().gap_1();
        for index in layer {
            let node = &diagram.nodes[*index];
            let shape_radius = match node.shape {
                Shape::Diamond => 2.,
                Shape::Stadium => 12.,
                _ => 6.,
            };
            let label = node.label.clone();
            nodes = nodes.child(
                div()
                    .id(SharedString::from(format!("mermaid-{id}-{index}")))
                    .rounded(px(shape_radius))
                    .border_1()
                    .border_color(gpui::rgb(border))
                    .bg(gpui::rgb(surface))
                    .px(px(9.))
                    .py(px(5.))
                    .font_family("Lilex")
                    .text_size(px(10.))
                    .text_color(gpui::rgb(ink))
                    .child(label),
            );
        }
        body = body.child(nodes);
        if layer_index + 1 < layers.len() {
            body = body.child(
                div()
                    .font_family("Lilex")
                    .text_size(px(12.))
                    .text_color(gpui::rgb(muted))
                    .child(if diagram.direction == Direction::LeftRight {
                        "→"
                    } else {
                        "↓"
                    }),
            );
        }
    }

    let selector = id.clone();
    let mut card = v_flex()
        .id(id)
        .debug_selector(move || format!("mermaid-diagram-{selector}"))
        .role(gpui::Role::Group)
        .aria_label("Mermaid diagram")
        .w_full()
        .min_w_0()
        .rounded_md()
        .border_1()
        .border_color(gpui::rgb(border))
        .bg(gpui::rgb(if dark { 0x141416 } else { 0xfafafa }))
        .p_3()
        .gap_2()
        .child(
            div()
                .font_family("Lilex")
                .text_size(px(9.))
                .text_color(gpui::rgb(muted))
                .child("MERMAID"),
        )
        .child(body);
    let labels: Vec<&Edge> = diagram
        .edges
        .iter()
        .filter(|edge| edge.label.is_some())
        .collect();
    if !labels.is_empty() {
        let mut notes = v_flex().gap(px(1.));
        for edge in labels {
            notes = notes.child(
                div()
                    .font_family("Lilex")
                    .text_size(px(9.))
                    .text_color(gpui::rgb(muted))
                    .child(format!(
                        "{} — {} → {}",
                        edge.from,
                        edge.label.as_deref().unwrap_or_default(),
                        edge.to
                    )),
            );
        }
        card = card.child(notes);
    }
    card.into_any_element()
}

#[cfg(test)]
mod tests {
    use super::{Direction, Shape, blocks, parse};

    #[test]
    fn parses_a_chain_into_layers() {
        let diagram = parse("graph TD\n  A[Plan] --> B[Build]\n  B --> C[Ship]").unwrap();
        assert_eq!(diagram.direction, Direction::TopDown);
        assert_eq!(diagram.nodes.len(), 3);
        assert_eq!(diagram.nodes[0].label, "Plan");
        assert_eq!(diagram.layers(), vec![vec![0], vec![1], vec![2]]);
    }

    #[test]
    fn parses_edge_labels_and_left_right() {
        let diagram = parse("flowchart LR\n A -->|yes| B\n A -.-> C").unwrap();
        assert_eq!(diagram.direction, Direction::LeftRight);
        assert_eq!(diagram.edges.len(), 2);
        assert_eq!(diagram.edges[0].label.as_deref(), Some("yes"));
        assert!(diagram.edges[1].label.is_none());
    }

    #[test]
    fn node_shapes_follow_their_delimiters() {
        let diagram = parse("graph TD\n A(Rounded) --> B{Diamond}\n B --> C((Stadium))").unwrap();
        assert_eq!(diagram.nodes[0].shape, Shape::Rounded);
        assert_eq!(diagram.nodes[1].shape, Shape::Diamond);
        assert_eq!(diagram.nodes[2].shape, Shape::Stadium);
    }

    #[test]
    fn directives_and_subgraphs_are_ignored() {
        let diagram =
            parse("graph TD\n subgraph one\n A --> B\n end\n classDef red fill:#f00\n class A red")
                .unwrap();
        assert_eq!(diagram.nodes.len(), 2);
        assert_eq!(diagram.edges.len(), 1);
    }

    #[test]
    fn plain_text_is_not_a_diagram() {
        assert!(parse("just some prose").is_none());
        assert!(parse("sequenceDiagram\n A->>B: hi").is_none());
    }

    #[test]
    fn fences_are_removed_from_the_prose() {
        let markdown = "Intro\n```mermaid\ngraph TD\n A --> B\n```\nOutro";
        assert_eq!(super::strip_blocks(markdown), "Intro\nOutro");
    }

    #[test]
    fn unsupported_fences_stay_as_readable_code() {
        let markdown = "Intro\n```mermaid\nsequenceDiagram\n A->>B: hi\n```\nOutro";
        let stripped = super::strip_blocks(markdown);
        assert!(stripped.contains("```mermaid"));
        assert!(stripped.contains("sequenceDiagram"));
        assert!(stripped.contains("Outro"));
    }

    #[test]
    fn markdown_fences_are_extracted() {
        let markdown = "Intro\n```mermaid\ngraph TD\n A --> B\n```\nOutro";
        let extracted = blocks(markdown);
        assert_eq!(extracted.len(), 1);
        assert!(parse(&extracted[0]).is_some());
        assert!(blocks("no fences").is_empty());
    }
}
