# read_image tool + image-capable tool results (align DeepSeek Harness)

> **For agentic workers:** Implement task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Align Tact's image handling with DeepSeek Harness `read_image`. The model sees a local image `file_path` (text), and calls `read_image` to load it. The tool result carries a text envelope (path / media type / dimensions / bytes) **plus the image itself**; on the wire (chat_completions) the image is placed in a separate **user message** following the string-only `role:tool` message, exactly like Harness `serializeMessagesWithImages`.

**Reference (Harness source, verified):** `packages/llm/llm-deepseek/src/serialize.ts` `serializeMessagesWithImages`:
- each `tool-result` image is removed from the `role:'tool'` content (that message keeps only text);
- collected image parts are flushed as a following `role:'user'` message:
  `{ role:'user', content:[ {type:'text', text:'Attached image(s) from tool result:'}, {type:'image_url',...} ] }`.
- `read_image` tool (`packages/fs/tool-fs/src/read-image.ts`) gates on the exact model declaring image input, validates extension/media type/dimension/bytes, normalizes, persists a durable ref, and returns `{path, image:{attachmentId,mediaType,bytes,width,height}}`.

**Constraint:** Tact uses `chat_completions`. `role:tool` content only accepts text; images ride a `user` message (`image_url` part). Do **not** change the `ContentBlock::ToolResult { content: String }` wire shape globally — that breaks Responses/normalize/compact consumers and does not solve the protocol limit.

## Design decision

Make a tool able to return **both** a text envelope and an image. Introduce a lightweight channel that does not touch the shared `ContentBlock::ToolResult` shape:

- New `ContentBlock::Image` already exists and `convert.rs` already maps it to `image_url` parts for **user** messages.
- New variant (internal only, not sent to wire as `ToolResult`): a tool-result **image side-channel**. We keep `ContentBlock::ToolResult { content: String }` as-is for the text envelope; images produced by a tool are emitted as a **companion `ContentBlock::Image` block in the user turn** after the tool-result text.

Concretely, `build_tool_results` will, for a tool whose output is `ToolOutcome::Text(String)`, emit `ContentBlock::ToolResult { content }`. For a tool whose output is `ToolOutcome::TextImage { text, image }`, emit `ContentBlock::ToolResult { content: text }` **then a `ContentBlock::Image { source }`** in the same user message. `convert.rs` already routes the `ToolResult` to `role:tool` (string-only) and the adjacent `Image` to a `user` parts message; we adjust `convert.rs` so an image after a tool-result is folded into the following user image message (Harness `serializeMessagesWithImages`), not emitted as part of the tool message.

This is the smallest change that mirrors Harness without reshaping `ContentBlock::ToolResult` for all consumers.

## Options

- **A (target):** tool side-channel `ToolOutcome::TextImage`; `read_image` returns it; `convert.rs` folds the image into a following user message.
- **B (fallback):** `read_image` returns text-only metadata (no image block). Chosen only if converting the image into a user message proves unworkable in `convert.rs` without breaking existing tool flows.

## Global Constraints

- Cargo commands run **serially** — never two `cargo test`/`build`/`clippy` at once (target/ lock).
- Unset `http_proxy`, `https_proxy`, `all_proxy` for `cargo test` / `git push`.
- Do **not** reshape `ContentBlock::ToolResult`; keep Responses + compact consumers intact.
- Every phase ends with gate: `cargo fmt --all` + `cargo clippy` + targeted `cargo test` all green.
- Update `book/22_chapter_llm{,_zh}.md` and `book/26_chapter_issue{,_zh}.md` in the same change.
- One independently revertable commit per phase.

## File Map

- New: `crates/tact/src/tool/read_image.rs` (the `read_image` tool).
- Modify: `crates/tact/src/tool/mod.rs` / `registry.rs` (route `read_image`), `crates/tact/src/agent/tool_dispatch.rs` (`ToolOutcome` side-channel), `crates/tact_llm/src/convert.rs` (fold image after tool-result into a user message), `crates/tact-ui/src/user_message.rs` (`@image` no longer auto-inline; leave path for `read_image`), `crates/tact-ui/src/driver.rs` (vision gate unchanged / read_image gate).
- Docs: `book/22_chapter_llm{,_zh}.md`, `book/26_chapter_issue{,_zh}.md`.

---

## Phase 0 — Baseline freeze

- [x] **T0.1** Record current `cargo test -p tact --lib` and `cargo test -p tact-ui --lib` counts.
- [x] **T0.2** Confirm `ToolOutcome` current shape and all `run_native_tool` return paths.
- [x] **T0.3 GO/NO-GO.** If side-channel breaks existing tool flows, adopt Option B.

## Phase 1 — `ToolOutcome` side-channel in `tool_dispatch.rs`

- [x] **T1.1** Define `enum ToolOutcome { Text(String), TextImage { text: String, image: ImageSource } }`.
- [x] **T1.2** `run_native_tool` returns `ToolOutcome`; text tools wrap `Text`.
- [x] **T1.3** `build_tool_results` emits `ContentBlock::ToolResult` + adjacent `ContentBlock::Image` for `TextImage`.
- [x] **T1.4** Tests.
- [x] **T1.5 Gate.**

## Phase 2 — `read_image` tool

- [x] **T2.1** `read_image.rs`: input `file_path`, gate `supports_vision()` (reject if text-only), `safe_path`, `prepare_image_attachment`, produce `TextImage { text: envelope, image }`.
- [x] **T2.2** Metadata + presentation (📦 picture kind).
- [x] **T2.3** Tests (image found, missing file, vision gate).
- [x] **T2.4 Gate.**

## Phase 3 — `convert.rs` folds image into a following user message

- [x] **T3.1** In `messages_to_openai`, a user message's `ContentBlock::Image` that follows a `ContentBlock::ToolResult` is not emitted as part of that tool message; it is collected and emitted as a following `role:user` parts message (`[ {text:"Attached image...",}, {image_url} ]`).
- [x] **T3.2** Ensure text-only `ToolResult` path unchanged (still `role:tool` string).
- [x] **T3.3** Tests.
- [x] **T3.4 Gate.**

## Phase 4 — Wire construction site: `build_user_message` leaves image paths

- [x] **T4.1** `@file.png` / `![alt](path)` no longer auto-inline to base64 `Image`; instead keep a text block that names the file and suggests `read_image`.
- [x] **T4.2** Keep `contentHasImage` / vision gate in driver for direct inline images (if any remain).
- [x] **T4.3** Tests + gate.

## Phase 5 — Docs + changelog

- [x] **T5.1** `book/22_chapter_llm{,_zh}.md` describe `read_image` + user-message image envelope.
- [x] **T5.2** `book/26_chapter_issue{,_zh}.md` entry.
- [x] **T5.3** Final full gate.
