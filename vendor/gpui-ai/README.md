# gpui-ai

UI components for AI applications built with [GPUI](https://gpui.rs).

gpui-ai covers streamed responses, tool calls, approvals, chat, prompt input, context usage, and tables. It builds on [gpui-component](https://github.com/longbridge/gpui-component) and uses its active theme. Your application owns model requests, retries, storage, and long-lived state.

[![crates.io](https://img.shields.io/crates/v/gpui-ai.svg)](https://crates.io/crates/gpui-ai) [![docs.rs](https://img.shields.io/docsrs/gpui-ai)](https://docs.rs/gpui-ai)

[Live components](https://labcoder.github.io/gpui-ai/components/) · [API docs](https://labcoder.github.io/gpui-ai/api/gpui_ai/) · [Themes](https://labcoder.github.io/gpui-ai/themes/) · [Changelog](CHANGELOG.md)

## How it fits

```mermaid
flowchart LR
    app[Your application] --> ai[gpui-ai]
    app --> component[gpui-component]
    ai --> component
    ai --> gpui[GPUI]
    component --> gpui
```

gpui-ai composes gpui-component controls and uses GPUI for custom components. Components render the state you pass in and emit typed events keyed by your IDs.

## Install

```sh
cargo add gpui-ai
```

An application also declares GPUI itself, which is published under another name
— GPUI Kit ships a snapshot of Zed's crate as `gpui-pre` — so the dependency
carries a rename, which keeps every `use gpui::` path working:

```toml
[dependencies]
gpui-ai = "0.8.0"
gpui = { package = "gpui-pre", version = "0.3" }
gpui-component = "0.6"
gpui_platform = { package = "gpui-pre-platform", version = "0.3", features = ["font-kit", "x11", "wayland", "runtime_shaders"] }
```

`gpui-ai`, `gpui-component`, and `gpui-pre` move together: a gpui-ai release
names the versions it was built against in [CHANGELOG.md](CHANGELOG.md), and
mixing a different `gpui-pre` under `gpui-component` gives you two incompatible
copies of GPUI's types.

## Quick start

Call `gpui_ai::init(cx)` during application setup and wrap the top-level view in `gpui_component::Root`. [`examples/minimal.rs`](crates/gallery/examples/minimal.rs) is a complete application in eighty lines — window, component, typed events, a style override, and a decoration:

```bash
cargo run -p gallery --example minimal
```

The [gallery source](crates/gallery/src/gallery.rs) then shows every component.

Agents building an application with the library can use the first-party
[`gpui-ai` skill](skills/gpui-ai/SKILL.md). It teaches component selection,
state ownership, typed events, styling, native and WASM constraints, and reads
its component facts from the same generated catalog as the website.

<details>
<summary>Stateless and stateful examples</summary>

Stateless components implement `RenderOnce`. Build them where you render them:

```rust
use gpui_ai::prelude::*;

ToolChip::new("edit", "Edit main.rs").status(ToolStatus::Running)
```

Stateful components are GPUI entities. Keep the entity and its subscriptions in your application state:

```rust
use gpui_ai::prelude::*;

let prompt = cx.new(|cx| PromptBar::new("agent-prompt", window, cx));
let _subscription = cx.subscribe(&prompt, |_, _, event: &PromptBarEvent, _| {
    println!("prompt event: {event:?}");
});
```

</details>

## Components

The [live catalog](https://labcoder.github.io/gpui-ai/components/) contains all 37 components with source code and live WebAssembly demos. The catalog groups them into:

- responses, progress, code, and diffs
- tools, plans, approvals, and tasks
- chat, prompts, attachments, and conversation history
- search, context, tables, navigation, and utility controls

<details>
<summary>View all 37 components</summary>

| Component | Kind | What it does |
|---|---|---|
| [`LoadingState`](https://labcoder.github.io/gpui-ai/components/loading/) | stateless | Loader with shimmer and elapsed time |
| [`Orbs`](https://labcoder.github.io/gpui-ai/components/orbs/) | stateless | Dot-lattice activity indicator with five animations |
| [`StatusBadge`](https://labcoder.github.io/gpui-ai/components/status/) | stateless | One status pill for every lifecycle, swapping states in a fixed slot |
| [`Thinking`](https://labcoder.github.io/gpui-ai/components/thinking/) | stateless | Collapsible reasoning trace for running and completed work |
| [`StreamingText`](https://labcoder.github.io/gpui-ai/components/streaming-text/) | stateless | Streaming Markdown with citations, sources, and follow-up prompts |
| [`CodeBlock`](https://labcoder.github.io/gpui-ai/components/code-block/) | stateless | Syntax-highlighted code with copy and streaming states |
| [`CodeDiff`](https://labcoder.github.io/gpui-ai/components/code-diff/) | stateless | Unified diff viewer with per-hunk accept and reject events |
| [`ToolChip`](https://labcoder.github.io/gpui-ai/components/tool-chips/) | stateless | Compact tool and file-edit status |
| [`ToolCall` / `ToolGroup`](https://labcoder.github.io/gpui-ai/components/tool-calls/) | stateless | Collapsible tool input, output, errors, approvals, and grouped calls |
| [`TaskRow`](https://labcoder.github.io/gpui-ai/components/tasks/) | stateless | Agent task status from `TaskSnapshot` data |
| [`TodoList`](https://labcoder.github.io/gpui-ai/components/todos/) | stateless | Task list with progress |
| [`ImageGeneration`](https://labcoder.github.io/gpui-ai/components/image-generation/) | stateless | Image-generation progress |
| [`SearchResults`](https://labcoder.github.io/gpui-ai/components/search/) | stateless | Web search result cards |
| [`ApprovalCard`](https://labcoder.github.io/gpui-ai/components/approval/) | stateless | Approval request with allow, deny, and resolved states |
| [`PlanCard`](https://labcoder.github.io/gpui-ai/components/plan/) | stateless | Proposed plan with step status and approval actions |
| [`RecommendationCard`](https://labcoder.github.io/gpui-ai/components/recommendation/) | stateless | Recommendation with a confidence meter |
| [`ContextCard`](https://labcoder.github.io/gpui-ai/components/context/) | stateless | Retrieved context with sources |
| [`InsightCard`](https://labcoder.github.io/gpui-ai/components/insights/) | stateless | Paged insights with sparkline charts |
| [`PromptBar`](https://labcoder.github.io/gpui-ai/components/prompt-bar/) | entity | Composer with mentions, commands, model selection, and attachments |
| [`VoiceControls`](https://labcoder.github.io/gpui-ai/components/voice/) | stateless | Dictation and speech state with typed events |
| [`MessageQueue`](https://labcoder.github.io/gpui-ai/components/queue/) | stateless | Queued prompts with move, edit, send, remove, and clear actions |
| [`Chat`](https://labcoder.github.io/gpui-ai/components/chat/) | entity | Virtualized transcript and composer with editing, branches, attachments, and unread state |
| [`Suggestions`](https://labcoder.github.io/gpui-ai/components/suggestions/) | stateless | Prompt suggestion chips with stable IDs |
| [`AttachmentStrip` / `AttachmentPreview`](https://labcoder.github.io/gpui-ai/components/attachments/) | stateless | Attachments with thumbnails, upload state, and typed events |
| [`ArtifactPanel`](https://labcoder.github.io/gpui-ai/components/artifact/) | stateless | Generated documents and code with previews, source, versions, and actions |
| [`ContextMeter`](https://labcoder.github.io/gpui-ai/components/context-meter/) | stateless | Context usage as a ring, bar, or text |
| [`CommandSearch`](https://labcoder.github.io/gpui-ai/components/command-search/) | entity | Command palette with filtering and keyboard navigation |
| [`SidebarNav`](https://labcoder.github.io/gpui-ai/components/sidebar-nav/) | entity | Collapsible, searchable workspace navigation |
| [`ThreadList`](https://labcoder.github.io/gpui-ai/components/thread-list/) | entity | Grouped conversation list with search, archive, rename, and delete actions |
| [`FineTuneCard`](https://labcoder.github.io/gpui-ai/components/fine-tune/) | entity | Design property editor for size, radius, opacity, and typeface |
| [`SelectionActions`](https://labcoder.github.io/gpui-ai/components/selection-actions/) | entity | Selection actions for asking, explaining, and rewriting |
| [`ChoiceGroup` / `Toggle`](https://labcoder.github.io/gpui-ai/components/form-controls/) | stateless | Single-choice groups and toggles in the library's own grammar |
| [`QuestionFlow`](https://labcoder.github.io/gpui-ai/components/question-flow/) | stateless | A short sequence of questions asked one at a time before acting |
| [`RecordsTable`](https://labcoder.github.io/gpui-ai/components/records-table/) | entity | Virtualized, sortable data grid |
| [`DiffTable`](https://labcoder.github.io/gpui-ai/components/diff-table/) | entity | Proposed edits over tabular data |
| [`FilterTable`](https://labcoder.github.io/gpui-ai/components/filter-table/) | entity | Filterable, reorderable table |
| [`ComparisonTable`](https://labcoder.github.io/gpui-ai/components/comparison-table/) | entity | Feature comparison matrix |

</details>

Components use `Progressive<T>` and `ProgressState` for streamed work. Stateful controls expose typed events; stateless controls use fluent builders.

## Styling

Every component takes styles the same way a `div` does, and the words you write
win over the component's own defaults:

```rust
use gpui_ai::prelude::*;
use gpui::prelude::*;

ApprovalCard::new("gate", "Publish the launch plan?")
    .border_color(cx.theme().border)   // over the component's warning ring
    .text_color(gpui::white())         // reaches the title and the description
```

The same on a stateful component, where the style goes on at construction:

```rust
let threads = cx.new(|cx| ThreadList::new("threads", window, cx).w(px(240.)));
```

This is a frame-level thing, not a wrapper: a background, a border, a radius, or
an ink set on a surrounding `div` paints around a component rather than on it.

Component defaults come from the active theme, so overriding is for the cases a
theme cannot know about — a card over a photograph, a panel that has to match
something outside the library.

### Decorations

Cards take a layer under their content and a layer over it, for the surfaces a
component cannot own:

```rust
use gpui_ai::prelude::{Decoration, decoration};

ApprovalCard::new("gate", "Publish?").decoration(
    Decoration::behind(img("nebula.jpg").rounded(decoration::frame_radius(cx)))
        .and_above(div().size_full().bg(scrim)),
)
```

A layer that paints to the edge rounds itself by `decoration::frame_radius`,
because GPUI's content mask is a rectangle and nothing can clip a subtree to a
corner radius on your behalf. A layer that never reaches a corner needs nothing.
`decoration::animated` drives one from a looping 0…1 that stops when the
component scrolls out of view or the reader has asked for less motion.

## Themes

Every component reads gpui-component theme tokens. The gallery includes 55 themes, and the [themes page](https://labcoder.github.io/gpui-ai/themes/) lets you preview or download each JSON file.

## Requirements

| | |
|---|---|
| Rust | 1.89 or newer |
| Platforms | macOS, Linux, and Windows |
| Linux setup | Run [`script/install-linux.sh`](script/install-linux.sh) once |
| Web gallery | A browser with WebGPU support |

## Run the gallery

```sh
git clone https://github.com/labcoder/gpui-ai
cd gpui-ai
cargo run -p gallery
```

Use `npm run dev:web` for the browser host. See [CONTRIBUTING.md](CONTRIBUTING.md) for setup and checks.

## License

[MIT](LICENSE)
# Tact localization fork

This directory is the upstream `gpui-ai` 0.8.0 source with a narrow,
application-facing localization patch. Tact pins the upstream API version and
keeps the fork local through `[patch.crates-io]` so Chinese text can be
supplied to PromptBar, CommandSearch, MessageQueue, SidebarNav, FilterTable,
and ApprovalCard instead of leaking component-owned English copy.

The upstream integration tests are retained under `tests/`. The workspace
excludes this directory from `cargo test --workspace`; Tact's own desktop tests
exercise the patched paths.

## Patch surface

The fork changes only presentation/accessibility copy and the label plumbing
needed to supply it:

- `plan.rs`: `PlanCardLabels` for plan, step, action, and lifecycle copy.
- `sidebar_nav.rs` / `sidebar_nav/render.rs`: badge, selected-item, and
  unavailable descriptions.
- `tool_call.rs`, `thinking.rs`, `question_flow.rs`: localized lifecycle and
  counter labels.
- `prompt_bar*`, `command_search.rs`, `message_queue.rs`, `todo_list.rs`,
  `filter_table.rs`, `records_table*`, `approval*`: application-supplied labels
  and empty-state/accessibility text.

## Updating upstream

1. Update the pinned `gpui-ai` version in Tact's `Cargo.toml`.
2. Fetch the matching upstream source into a temporary directory.
3. Diff it against this directory and reapply the label fields, setters, and
   accessibility formatting above; do not replace the visual grammar or layout.
4. Run `cargo test --lib` in this directory with a shared target directory.
5. Run `cargo test -p tact-desktop --lib` and
   `cargo test -p tact-desktop --test parity` from the workspace root.
6. Update this inventory if a new component-owned string becomes localizable.

