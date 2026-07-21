# Live Tool Output Design

> Date: 2026-07-19
> Status: approved
> Scope: generic tool progress protocol, with `bash` as the first producer

## Goal

Show useful terminal output while a long-running tool is still executing. The
first implementation streams `bash` stdout and stderr into its active TUI card,
while the protocol and reporting boundary remain reusable by MCP, download, and
other long-running tools.

The default TUI presentation is a fixed five-line tail inside the active tool
card. This lets users distinguish a healthy long-running command from a hung
command without allowing build or download logs to displace the surrounding
conversation.

## Current problem

The current path cannot display incremental command output:

1. `bash` calls `Child::wait_with_output()` and receives stdout/stderr only after
   process exit.
2. `AgentUpdate` has tool start and finish events, but no event associated with
   an in-flight tool's output.
3. `ActiveToolBlock` therefore has only a title, spinner, and elapsed duration.

The limitation is below the renderer. Expanding the existing active card alone
cannot reveal output that the tool and protocol never emit.

## Decision summary

| Choice | Decision |
|--------|----------|
| Protocol | Add a generic `ToolProgress` event keyed by `tool_id` |
| First producer | `bash` |
| Active-card layout | Fixed tail of the latest five logical lines |
| Stream handling | Merge by observed arrival order; style stderr as warning text |
| Terminal fidelity | Plain line-oriented text; strip ANSI and interpret carriage return as current-line replacement |
| PTY / interactive input | Out of scope |
| UI batching | One ordered batch per 50 ms; 4 KiB maximum payload; final flush before completion |
| Live detail cap | 50,000 characters, with an omission marker |
| Bash timeout | Configurable `tools.bash_timeout_secs`; default 1,800; `0` disables wall-clock timeout |
| Command semantics | Never inject PTY, `stdbuf`, or rewrite the shell command |

## Scope

In scope:

- A reusable protocol event and per-invocation progress reporter.
- Concurrent stdout/stderr capture for `bash`.
- Incremental UTF-8 decoding, ANSI removal, and carriage-return handling.
- A bounded live-output model attached to `ActiveToolBlock`.
- A running-tool detail popup using bounded output captured so far.
- Configurable bash timeout and cooperative cancellation of a running child.
- Focused protocol, tool, TUI, integration, config, and documentation updates.

Out of scope:

- A PTY, terminal screen emulator, cursor movement, or interactive stdin.
- Rewriting commands to defeat application, pipe, or libc buffering.
- Adding progress producers to every existing native or MCP tool in this change.
- Persisting the full unbounded live transcript separately from the existing
  bounded tool result.
- A new bottom console, split view, or user-selectable live-output layout.

## 1. Protocol and reporting boundary

Add these protocol shapes:

```rust
pub enum ToolOutputStream {
    Stdout,
    Stderr,
    Other,
}

pub struct ToolOutputChunk {
    pub stream: ToolOutputStream,
    pub text: String,
}

pub enum AgentUpdate {
    // existing variants...
    ToolProgress {
        tool_id: String,
        chunks: Vec<ToolOutputChunk>,
    },
}
```

Each chunk is incrementally decoded UTF-8 text and may still contain terminal
control sequences. It is never rendered directly. The shared plain-text state
removes ANSI sequences and recognizes only newline and carriage return as line
controls: `\n` commits a logical line and `\r` replaces the current logical line
for that stream. The ordered chunk vector preserves stdout/stderr interleaving
inside one rate-limited protocol event.

`ToolProgress` is informational. It never means success or failure and never
replaces `StepFinished` or `StepFailed`. Unknown or late progress for a
non-active `tool_id` is ignored by the TUI.

Dispatch creates a `ToolProgressReporter` for each prepared native tool call.
The reporter owns the call's `tool_id` and a clone of the optional UI sender, so
tools cannot accidentally publish output under another invocation. A
per-invocation `ToolContext` receives this reporter before `ToolRouter::call`.

The reporter exposes this tool-facing operation:

```rust
reporter.report(chunks);
```

The reporter is a no-op when there is no UI channel. This preserves headless and
library execution without requiring every caller to branch on UI availability.
Closing the UI receiver also makes reporting a no-op; it does not fail the tool.

`ToolOutputStream::Other` allows a later MCP or native tool to report textual
progress without pretending it owns a shell file descriptor. Structured
percentages, phases, and arbitrary metadata are deferred until a real producer
requires them.

## 2. Bash execution and capture

Replace `wait_with_output()` with an explicit child lifecycle:

```text
spawn child with piped stdout/stderr
  -> stdout reader --\
                      > ordered local aggregation -> capture + reporter
  -> stderr reader --/
  -> child exit / timeout / cancellation
  -> flush decoder and pending progress
  -> final tool result
```

Two Tokio reader tasks consume stdout and stderr concurrently and send chunks
through a bounded local channel. The aggregator defines order as the sequence in
which it receives chunks. This is the best observable ordering available after
the operating system has written to separate pipes; the design does not claim a
stronger cross-descriptor ordering guarantee.

Each stream keeps incremental UTF-8 decoder state so a multibyte character split
across reads is not replaced prematurely. Invalid terminal bytes use lossy
replacement only after the decoder can determine they are invalid or when the
stream closes.

A shared, pure text-buffer component consumes `(stream, text)` records. Both the
final bash capture and the TUI live-output state use the same rules:

- Strip common ANSI CSI and OSC sequences.
- `\n` commits the current logical line.
- `\r` resets the current logical line so subsequent text replaces it.
- Keep stream identity on logical lines for stderr styling.
- Treat unsupported control characters as removed plain-terminal noise rather
  than implementing cursor or screen movement.

The final tool result and the live UI therefore derive from the same ordered
plain-text records. Existing result semantics remain bounded to 50,000
characters. Truncation is explicit rather than silent.

### Command buffering boundary

Tact displays only bytes emitted by the command's stdout/stderr pipes. For
example, `producer | tail -5` may emit nothing until EOF because `tail` owns the
buffering. Tact must not rewrite that pipeline, inject `stdbuf`, or create a PTY
to make hidden upstream output visible.

## 3. Batching and memory bounds

The existing agent-to-TUI channel is unbounded, so the bash aggregator must not
publish every read or line as a separate event.

Progress batching uses these fixed v1 rules:

- The first non-empty batch may be sent immediately.
- After a send, wait at least 50 ms before the next regular progress event.
- Cap a progress event's ordered chunk payload at 4 KiB.
- During the 50 ms cooldown, retain new ordered chunks up to that cap. If the
  producer outruns it, retain the newest tail and prepend an `Other` omission
  chunk.
- Flush pending progress before the terminal step event even if the 50 ms
  cooldown has not elapsed.

Apart from the final flush, this produces at most 20 progress events per second
per active tool. Alternating stdout/stderr remains ordered within each event
without multiplying the event count.

UI loss under rate pressure affects only intermediate display. The final result
capture is independent of UI batching and retains all content within its normal
50,000-character result limit.

The TUI maintains two bounded views per active tool:

- A five-logical-line ring for inline display.
- A 50,000-character detail buffer for the running popup.

The detail buffer adds an explicit marker when older or over-rate live content
is unavailable. It is described as buffered output, not an unbounded transcript.

## 4. TUI behavior

`StepStarted` continues to create the current two-row `ActiveToolBlock`. The
first matching `ToolProgress` event adds a live detail card. Its viewport is
fixed at five logical output rows; after that first expansion, subsequent output
mutates the existing active block without adding physical log messages or
changing its height.

The active card shows:

- A title such as `Live output (46 lines)`.
- The latest five logical lines.
- Normal text styling for stdout and warning styling for stderr.
- The existing double-click hint for the detail popup.

No empty live-output card is shown before the first output arrives.

Progress updates set the app dirty and rebuild only the affected active tool
output. If the log is pinned to the bottom, it remains pinned. If the user has
scrolled upward, progress must not force the offset back to the bottom.

Double-clicking an active command card opens the same tool-detail popup used by
completed command output, populated with buffered output so far. Each concurrent
tool is isolated by `tool_id`; progress cannot attach to the current numeric step
alone.

On completion:

- Success replaces the active block with the current compact completed-card
  presentation. The final output remains available in the popup.
- Failure uses the existing failed-card presentation with up to five error lines.
- The final `StepResult.detail` is authoritative for completed popup content.
- Any pending progress is applied before finalization so the last visible output
  is not lost.

## 5. Timeout, cancellation, and failures

Add the public configuration field:

```toml
[tools]
bash_timeout_secs = 1800
```

Resolution rules:

- Omitted: `1_800` seconds.
- Positive value: wall-clock timeout in seconds.
- `0`: wall-clock timeout disabled.

The resolved value is carried by `ToolSettings` into `ToolContext`. No new CLI
flag is added in v1.

Because a 30-minute or unlimited command must remain cancellable, `ToolContext`
also exposes the agent's existing shared cancellation flag. The bash lifecycle
selects over child completion, timeout (when enabled), and cancellation. Timeout
or cancellation terminates the child, drains or flushes already-read data, and
does not leave reader tasks running. The command driver remains responsible for
emitting `TaskCancelled` for the overall task.

Failure behavior:

| Failure | Behavior |
|---------|----------|
| Spawn failure | Emit no progress; finalize the already-started card with a failed result |
| Stdout/stderr read failure | Terminate child, flush captured text, and return the read error with available output |
| Timeout | Terminate child and return a failure naming the configured timeout while preserving available output |
| User cancellation | Terminate child, flush readers, and let the existing task cancellation path complete |
| UI receiver closed | Stop progress reporting; command and final result continue normally |
| Post-tool hook failure | Existing dispatch behavior remains authoritative after bash returns |

On Unix, bash starts the shell in its own process group so timeout and
cancellation terminate the shell and its command descendants. On non-Unix
platforms, the implementation uses Tokio's child termination support and closes
the associated pipes. Process-group tests are platform-gated.

## 6. Components and ownership

| Component | Responsibility |
|-----------|----------------|
| `tact_protocol` agent types | `ToolProgress` and `ToolOutputStream` transport contract |
| Shared plain-text buffer | ANSI cleanup, newline/carriage-return semantics, bounded lines, stream identity |
| `ToolProgressReporter` | Bind an already-coalesced chunk batch to one `tool_id`; tolerate missing/closed UI |
| `ToolContext` | Carry per-call reporter, resolved bash timeout, and cancellation handle |
| `bash` tool | Spawn, read both pipes, aggregate, capture final output, timeout/cancel child |
| TUI `ToolState` | Store bounded live output on each `ActiveToolBlock` |
| TUI agent update handler | Route progress by `tool_id`, preserve scroll intent, finalize cleanly |
| `ToolWidget` / `ToolCell` | Render the fixed live tail and stream-aware styles |
| Tool popup path | Show bounded output for both active and completed command cards |

These boundaries keep process I/O out of the TUI and rendering details out of
the bash tool. Later producers use the reporter without depending on bash.

## 7. Testing

### Protocol and text-buffer unit tests

- Construct and route an ordered `ToolProgress` batch containing each stream kind.
- Preserve UTF-8 characters split across input chunks.
- Remove supported ANSI CSI/OSC sequences.
- Replace the current line on carriage return without incrementing line count.
- Preserve stderr identity for warning styling.
- Retain only the latest five inline lines.
- Bound detail text at 50,000 characters and add an omission marker.

### Bash tests

- A delayed script emits progress before `StepFinished`.
- Interleaved stdout/stderr is reported in aggregator-observed order.
- The final detail follows the same normalized order as live progress.
- High-frequency output is coalesced rather than sent per line/read.
- UI batching does not truncate the independent final capture below its normal
  result limit.
- Default timeout resolves to 1,800 seconds; a custom short timeout fails and
  preserves partial output; `0` disables timeout.
- Cancellation terminates the running child and reader tasks.
- Missing or closed UI channels do not change the final tool result.

### TUI tests

- The active card remains two rows before first output.
- First output creates a fixed five-row live viewport; further output does not
  grow the card.
- Stdout and stderr lines use their intended styles.
- Carriage-return progress replaces the tail line.
- Concurrent `tool_id` values update only their own active blocks.
- Progress keeps bottom pinning only when the user was already pinned.
- Active-card double-click opens buffered output.
- Success collapses to the existing completed layout; failure retains the
  existing five-line error preview.
- Late progress after completion is ignored.

### Integration tests

- Interactive driver ordering is `StepStarted`, one or more `ToolProgress`, then
  `StepFinished`.
- Headless execution without a UI channel remains successful.
- Timeout and cancellation do not leave a running child behind.

## 8. Documentation sync

Implementation must update these contracts in the same change:

- `book/07_chapter_tool.md` and `book/07_chapter_tool_zh.md`: bash streaming,
  timeout, buffering boundary, and cancellation.
- `book/23_chapter_tui.md` and `book/23_chapter_tui_zh.md`: active-card live tail,
  popup, scrolling, and rendering lifecycle.
- `book/25_chapter_protocol.md` and `book/25_chapter_protocol_zh.md`:
  `ToolProgress` event ordering and state-machine effect.
- `book/18_chapter_agent_loop.md` and `book/18_chapter_agent_loop_zh.md`:
  cancellation of an in-flight bash process before `TaskCancelled`.
- `book/21_chapter_config.md` and `book/21_chapter_config_zh.md`:
  resolved timeout defaults and `0` semantics.
- `docs/tool_rendering.md`: active output state and fixed-height rendering.
- `tact.example.toml`: `tools.bash_timeout_secs` example and semantics.
- `README.md`: add the public bash timeout to the configuration and tool sections.
- `ARCHITECTURE.md`: add live tool progress to the agent-to-TUI protocol overview.

The paired English and Chinese book chapters must retain matching section
structure and behavior descriptions.

## 9. Acceptance criteria

1. A command that emits a line at least once per second visibly updates its
   active TUI card before it exits.
2. The active inline output never exceeds five logical rows and does not grow
   after its first expansion.
3. stderr is visually distinguishable from stdout while arrival order is
   preserved as observed by the aggregator.
4. ANSI sequences are not rendered literally, and carriage-return progress
   updates replace the current line.
5. A user who scrolls upward is not pulled back to the bottom by progress.
6. A completed command exposes the same normalized final output through the
   existing detail popup.
7. A default bash command may run for up to 1,800 seconds; the timeout is
   configurable and `0` disables it.
8. Cancellation terminates a running bash child without waiting for the timeout.
9. Commands that buffer their own output remain semantically unchanged.
10. Headless and closed-UI execution continue without progress-related failure.
