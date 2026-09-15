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
- When the session has the opt-in sandbox enabled (`[tools] sandbox = "bwrap"`),
  the `bash` tool description says so. Then the shell sees the workspace at
  `/workspace` (the host path is *not* mounted), the network is disabled, and the
  host home directory is unavailable. Rewrite host paths under the workspace to
  `/workspace/...` before using them in a command — in-process tools keep
  reporting host absolute paths.
- Never rely on the sandbox for correctness: it is opt-in, best-effort, and
  degrades to unsandboxed execution (with a warning) when `bwrap` cannot start.
