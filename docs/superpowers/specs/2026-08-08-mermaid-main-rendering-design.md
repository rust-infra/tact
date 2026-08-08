# Mermaid Main-Area Rendering Design

- **Date:** 2026-08-08
- **Branch:** `wt/feat-mermaid-main-rendering`
- **Status:** Approved for planning

## Goal

Render Mermaid fenced blocks in the TUI main log area as terminal diagrams instead of treating them as ordinary code cards. Preserve the existing Markdown and code-block behavior everywhere else.

## Scope

### In scope

- Recognize complete fenced blocks whose language is `mermaid` in the main-area Markdown paths.
- Reuse the workspace's existing `ratatui-markdown` Mermaid renderer.
- Support every diagram type implemented by that dependency, including `flowchart`/`graph`, `sequenceDiagram`, `pie`, `gantt`, `stateDiagram`, `classDiagram`, `quadrantChart`, and `block`.
- Use the application theme and the available log width.
- Make streaming output, persisted history, and whole-Markdown cells use the same Mermaid behavior.
- Fall back to the existing code rendering when Mermaid parsing fails or a diagram is unsupported.
- Keep `/tasks-dag` behavior working while sharing the theme adapter where practical.

### Out of scope

- Adding a new Mermaid parser or dependency.
- Implementing Mermaid types not supported by the pinned `ratatui-markdown` revision.
- Changing ordinary Markdown, tables, headings, or non-Mermaid code-card styling.
- Rendering Mermaid images or adding terminal image support.
- Interactive diagram navigation beyond the existing log scrolling and clipping.

## Current Context

The main area currently uses `tui-markdown` through `render_markdown_tui`. During streaming, every explicit fenced language is promoted to a `CodeBlock` overlay when the fence closes. The repository already enables `ratatui-markdown`'s `markdown` and `mermaid` features for the `/tasks-dag` popup, which directly uses `MarkdownRenderer` and an application-theme adapter.

## Design

### 1. Shared Mermaid rendering helper

Add a small helper in the TUI rendering layer that:

1. Accepts Mermaid source, theme, and maximum width.
2. Adapts the application `Theme` to `ratatui_markdown::theme::RichTextTheme`.
3. Calls `ratatui_markdown::mermaid::render_mermaid`.
4. Returns rendered `Line`s only when parsing/rendering succeeds.

Generalize or relocate the existing `/tasks-dag` theme adapter instead of creating a second color mapping. The helper must not panic on malformed or unsupported input.

### 2. Markdown block routing

Extend the existing Markdown rendering pipeline to identify top-level fenced blocks with a case-insensitive `mermaid` language tag. Flush surrounding prose/table content through the current renderer, render valid Mermaid blocks with the shared helper, and preserve all other fences through the existing `tui-markdown` path.

The routed Mermaid block is rendered as terminal `Line`s. Its visible/raw representation should remain aligned so log height, scrolling, and selection do not lose or duplicate rows. If the helper returns no diagram, the original fenced source goes through the ordinary code path.

### 3. Streaming behavior

Keep buffering a Mermaid fence until its closing fence, but mark that buffered block as Mermaid so the stream does not create the ordinary code-card header/overlay. On close:

- valid Mermaid: replace the buffered placeholder rows with the rendered diagram lines;
- invalid Mermaid: use the existing code-card path and retain the source;
- ordinary language fence: retain current behavior unchanged.

The stream's existing incomplete-fence behavior remains safe: if a response ends before the closing fence, its content is displayed as ordinary buffered code/text rather than being silently discarded.

### 4. Width and theme behavior

Use the actual main-area content width whenever it is available. Mermaid layout is width-dependent, so rendered lines must be regenerated when the width or theme cache changes, following the existing log visual-cache invalidation model. Avoid adding a second long-lived cache unless the current Markdown cell/cache boundary requires it.

The viewport continues to clip tall diagrams through `LogColumnRenderer`; no separate Mermaid scrolling state is needed.

### 5. `/tasks-dag` compatibility

Keep task DAG source generation, popup scrolling, width-aware rerendering, and copy behavior unchanged. Only replace its local theme adapter with the shared adapter if that reduces duplication without changing output.

## Error Handling and Fallbacks

- Empty or malformed Mermaid source: ordinary code rendering.
- Unknown diagram keyword: ordinary code rendering if the dependency returns no rendered lines.
- Renderer output wider/taller than the viewport: normal width layout and viewport clipping.
- No new user-facing error message; preserving source is more useful than adding log noise.

## Tests

Add focused tests before implementation code:

1. A `sequenceDiagram` Markdown block renders lifelines/arrows and not raw `sequenceDiagram` source.
2. A `flowchart` Markdown block renders terminal box/edge characters.
3. Invalid Mermaid falls back to code text/card.
4. A streamed valid Mermaid block closes into diagram rows without a normal code-card overlay.
5. A normal language fence (for example `rust`) remains a code card.
6. Existing history/whole-Markdown-cell rendering uses the same Mermaid route.
7. Existing `/tasks-dag` rendering tests remain green.

Run the focused TUI tests first, then the complete `cargo test -p tui --lib` command with proxy variables unset.

## Acceptance Criteria

- Main-area ` ```mermaid ` blocks display terminal diagrams for all types supported by the pinned renderer.
- `sequenceDiagram` is visibly rendered in the main area, including participant/lifeline and message arrows.
- Invalid Mermaid never disappears and remains readable as code.
- Existing Markdown, ordinary code blocks, scrolling, width changes, themes, and task DAG popup behavior remain intact.
- No new dependency is added.
