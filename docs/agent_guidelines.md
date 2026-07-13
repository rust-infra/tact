# Agent coding & tool-use guidelines

This file documents behavioural conventions for the agent (AI assistant) to
ensure consistent and efficient tool usage in this project.

## Tool-usage limits

### batch_edit

- **`batch_edit` must only be used when the edit spans 3 or more distinct files.**
- For fewer than 3 files, prefer `apply_patch` (structured / multi-line) or
  `write_file` (full rewrite / new file). This avoids batch-validation overhead
  for simple single-file changes.

### apply_patch / write_file

- Prefer `apply_patch` for multi-line or structured edits to existing files.
- Use `write_file` for new files or complete rewrites.
- Avoid running auto-diff on every edit — it impacts performance.
