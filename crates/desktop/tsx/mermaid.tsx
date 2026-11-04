import { Text, View, token } from "nsh";

const mono = "Lilex";

function color(name: string): string {
  return token(name as never) as string;
}

type Shape = "rectangle" | "rounded" | "diamond" | "stadium";

interface MermaidNode {
  id: string;
  label: string;
  shape: Shape;
}

interface MermaidEdge {
  from: string;
  to: string;
  label: string | null;
}

export interface MermaidDiagram {
  direction: "td" | "lr";
  nodes: MermaidNode[];
  edges: MermaidEdge[];
}

const ARROWS = ["-.->", "==>", "-->", "---", "--x", "--o"];

function findArrow(text: string): { start: number; end: number } | null {
  for (let index = 0; index < text.length; index += 1) {
    for (const arrow of ARROWS) {
      if (text.startsWith(arrow, index)) {
        return { start: index, end: index + arrow.length };
      }
    }
  }
  return null;
}

function parseNode(segment: string): MermaidNode | null {
  const text = segment.trim();
  if (!text) return null;
  const open = text.search(/[[({>]/);
  if (open < 0) {
    if (/\s/.test(text)) return null;
    return { id: text, label: text, shape: "rectangle" };
  }
  const id = text.slice(0, open).trim();
  if (!id || id.includes(" ")) return null;
  const opener = text[open];
  const closer = opener === "[" ? "]" : opener === "(" ? ")" : opener === "{" ? "}" : "]";
  const end = text.lastIndexOf(closer);
  if (end <= open) return null;
  const raw = text.slice(open + 1, end).trim();
  const label = raw.replace(/^"+|"+$/g, "");
  const shape: Shape =
    opener === "(" && text.startsWith("((", open)
      ? "stadium"
      : opener === "("
        ? "rounded"
        : opener === "{"
          ? "diamond"
          : "rectangle";
  return { id, label: label || id, shape };
}

function pushNode(nodes: MermaidNode[], node: MermaidNode): void {
  const existing = nodes.find((entry) => entry.id === node.id);
  if (!existing) {
    nodes.push(node);
    return;
  }
  if (node.label !== node.id || node.shape !== "rectangle") {
    existing.label = node.label;
    existing.shape = node.shape;
  }
}

function parseChain(line: string, nodes: MermaidNode[], edges: MermaidEdge[]): void {
  let rest = line.trim();
  let previous: string | null = null;
  for (;;) {
    const arrow = findArrow(rest);
    if (!arrow) {
      const node = parseNode(rest);
      if (node) {
        pushNode(nodes, node);
        if (previous != null) edges.push({ from: previous, to: node.id, label: null });
      }
      return;
    }
    if (previous == null) {
      const node = parseNode(rest.slice(0, arrow.start));
      if (node) {
        pushNode(nodes, node);
        previous = node.id;
      }
    }
    let tail = rest.slice(arrow.end).trimStart();
    let edgeLabel: string | null = null;
    if (tail.startsWith("|")) {
      const close = tail.indexOf("|", 1);
      if (close > 0) {
        edgeLabel = tail.slice(1, close).trim();
        tail = tail.slice(close + 1).trimStart();
      }
    }
    const next = findArrow(tail);
    const targetEnd = next ? next.start : tail.length;
    const node = parseNode(tail.slice(0, targetEnd));
    if (node) {
      pushNode(nodes, node);
      if (previous != null) edges.push({ from: previous, to: node.id, label: edgeLabel });
      previous = node.id;
    }
    rest = tail.slice(targetEnd);
    if (!rest.trim()) return;
  }
}

export function parseMermaid(source: string): MermaidDiagram | null {
  let direction: "td" | "lr" | null = null;
  const nodes: MermaidNode[] = [];
  const edges: MermaidEdge[] = [];
  for (const raw of source.split("\n")) {
    const line = raw.trim();
    if (!line || line.startsWith("%%")) continue;
    if (direction == null) {
      const parts = line.split(/\s+/);
      if (parts[0] === "graph" || parts[0] === "flowchart") {
        direction = parts[1] === "LR" || parts[1] === "RL" ? "lr" : "td";
        continue;
      }
      return null;
    }
    const lower = line.toLowerCase();
    if (
      lower.startsWith("subgraph") ||
      lower === "end" ||
      lower.startsWith("classdef") ||
      lower.startsWith("class ") ||
      lower.startsWith("style ") ||
      lower.startsWith("linkstyle") ||
      lower.startsWith("click ")
    ) {
      continue;
    }
    parseChain(line, nodes, edges);
  }
  if (direction == null || nodes.length === 0) return null;
  return { direction, nodes, edges };
}

export function splitMermaid(markdown: string): {
  body: string;
  diagrams: MermaidDiagram[];
} {
  const out: string[] = [];
  const diagrams: MermaidDiagram[] = [];
  let fence: string[] | null = null;
  for (const line of markdown.split("\n")) {
    const trimmed = line.trim();
    if (fence) {
      if (trimmed.startsWith("```")) {
        const source = fence.join("\n");
        const diagram = parseMermaid(source);
        if (diagram) diagrams.push(diagram);
        else out.push("```mermaid", source, "```");
        fence = null;
      } else {
        fence.push(line);
      }
    } else if (trimmed.toLowerCase() === "```mermaid") {
      fence = [];
    } else {
      out.push(line);
    }
  }
  if (fence) out.push("```mermaid", fence.join("\n"), "```");
  return { body: out.join("\n").trimEnd(), diagrams };
}

function layers(diagram: MermaidDiagram): number[][] {
  const depth = new Map<string, number>();
  for (const node of diagram.nodes) depth.set(node.id, 0);
  for (let pass = 0; pass <= diagram.nodes.length; pass += 1) {
    let changed = false;
    for (const edge of diagram.edges) {
      const from = depth.get(edge.from) ?? 0;
      const to = depth.get(edge.to) ?? 0;
      if (to < from + 1) {
        depth.set(edge.to, from + 1);
        changed = true;
      }
    }
    if (!changed) break;
  }
  const deepest = Math.max(0, ...depth.values());
  const result: number[][] = Array.from({ length: deepest + 1 }, () => []);
  diagram.nodes.forEach((node, index) => {
    result[depth.get(node.id) ?? 0].push(index);
  });
  return result;
}

function radius(shape: Shape): number {
  if (shape === "diamond") return 2;
  if (shape === "stadium") return 12;
  return 6;
}

export function MermaidDiagram(props: { diagram: MermaidDiagram; index: number }) {
  const horizontal = props.diagram.direction === "lr";
  const rows = layers(props.diagram);
  const labels = props.diagram.edges.filter((edge) => edge.label);
  return (
    <View
      accessibilityLabel="Mermaid diagram"
      style={{
        alignSelf: "stretch",
        paddingLeft: 12,
        paddingRight: 12,
        paddingTop: 12,
        paddingBottom: 12,
        gap: 8,
        borderRadius: 8,
        borderWidth: 1,
        borderColor: color("border"),
        backgroundColor: color("background"),
      }}
    >
      <Text style={{ fontSize: 9, fontFamily: mono, color: color("muted-foreground") }}>
        MERMAID
      </Text>
      <View
        style={{
          flexDirection: horizontal ? "row" : "column",
          flexWrap: horizontal ? "wrap" : "nowrap",
          alignItems: horizontal ? "center" : "flex-start",
          gap: 4,
        }}
      >
        {rows.map((layer, layerIndex) => (
          <View
            style={{
              flexDirection: horizontal ? "column" : "row",
              flexWrap: "wrap",
              alignItems: "center",
              gap: 4,
            }}
          >
            <View style={{ flexDirection: "row", flexWrap: "wrap", alignItems: "center", gap: 4 }}>
              {layer.map((nodeIndex) => {
                const node = props.diagram.nodes[nodeIndex];
                return (
                  <View
                    style={{
                      paddingLeft: 9,
                      paddingRight: 9,
                      paddingTop: 5,
                      paddingBottom: 5,
                      borderRadius: radius(node.shape),
                      borderWidth: 1,
                      borderColor: color("border"),
                      backgroundColor: color("card"),
                    }}
                  >
                    <Text style={{ fontSize: 10, fontFamily: mono }}>{node.label}</Text>
                  </View>
                );
              })}
            </View>
            {layerIndex + 1 < rows.length ? (
              <Text style={{ fontSize: 12, fontFamily: mono, color: color("muted-foreground") }}>
                {horizontal ? "→" : "↓"}
              </Text>
            ) : null}
          </View>
        ))}
      </View>
      {labels.length > 0 ? (
        <View style={{ flexDirection: "column", gap: 1 }}>
          {labels.map((edge) => (
            <Text style={{ fontSize: 9, fontFamily: mono, color: color("muted-foreground") }}>
              {edge.from} — {edge.label} → {edge.to}
            </Text>
          ))}
        </View>
      ) : null}
    </View>
  );
}
