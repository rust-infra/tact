# Agent coding & tool-use guidelines

This file documents behavioural conventions for the agent (AI assistant) to
ensure consistent and efficient tool usage in this project.

## Tool-usage limits

### edit_file

- Use `edit_file` for exact string replacements in an existing file.
- Default: replace only the first match. Set `replace_all=true` when every
  occurrence in that file should change (e.g. rename a local symbol).
- Diff preview is lazy-loaded: the tool output shows `new_text` directly (it is
  already part of the arguments, no extra cost). The user can click the card to
  run `git diff` for the full comparison.
- Avoid running auto-diff on every edit — it impacts performance.
- For multi-line or structured changes, prefer `apply_patch`. For new files or
  complete rewrites, use `write_file`.

### bash and the workspace path space

- By default `bash` runs on the host and `pwd` is the project directory, so host
  absolute paths from `read_file` / `grep` results can be used as-is.
- When the session has the opt-in sandbox enabled (`[tools] sandbox = true`,
  Linux only), the `bash` tool description says so. Then the shell sees the workspace at
  `/workspace` (the host path is *not* mounted) and the
  host home directory is unavailable; the host network (including a proxy on
  the host's loopback) is shared, so outbound commands work as usual. Rewrite host paths under the workspace to
  `/workspace/...` before using them in a command — in-process tools keep
  reporting host absolute paths.
- Never rely on the sandbox for correctness: it is opt-in, best-effort, and
  degrades to unsandboxed execution (with a warning) whenever no sandbox can
  start (a platform without an implementation, `bwrap` missing).

### Waiting on background tasks

- To get a background task's result, call `wait_background` (optionally with its
  id) — it returns the moment the task finishes. There is no time limit on the
  task itself: a build or test suite runs until it ends or the user cancels.
- For a command you are starting now *and* expect to finish at once, pass
  `wait_ms` to `background_run` (max 10000) and get the output in the same call.
  Do not use a big `wait_ms` to cover slow work — the cap makes that pointless,
  so leave it out and wait afterwards.
- Do **not** `sleep` to wait for a background task: the duration is a guess, and
  a `sleep` cannot be interrupted by the user's cancel; the next `wait_background`
  poll notices it within ~150 ms.
- Only fall back to `check_background` polling when the turn has other work to do
  while the task runs.
