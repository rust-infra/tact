# Agent coding & tool-use guidelines

This file documents behavioural conventions for the agent (AI assistant) to
ensure consistent and efficient tool usage in this project.

## Tool-usage limits

### batch_edit

- **`batch_edit` must only be used when the edit spans 3 or more distinct files.**
- For edits touching fewer than 3 different files, prefer individual `edit_file`
  calls instead. This avoids the overhead of batch validation when simple
  single-file edits suffice.

### edit_file

- Diff preview is lazy-loaded: the tool output shows `new_text` directly (it is
  already part of the arguments, no extra cost). The user can click the card to
  run `git diff` for the full comparison.
- Avoid running auto-diff on every edit — it impacts performance.
