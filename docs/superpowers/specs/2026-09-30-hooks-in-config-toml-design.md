# Design: inline `[hooks]` tables in `config.toml`

Date: 2026-09-30
Status: approved for implementation

Closes the second of the three items in [Ch 9 §13](../../../book/09_chapter_hook.md): *"Inline `[hooks]`
tables in `config.toml` | Codex accepts a third spelling; Tact deliberately has one, because the file
entry point is what `bm hook install`-style tooling writes and a second spelling would need its own
precedence rules."*

## 1. Why the recorded objection does not hold

The objection has two halves, and only one of them is real.

**"A second spelling needs precedence rules."** Hooks are *additive*: Ch 9 already says a higher layer
never replaces a lower one, and **all matching hooks run**. So there is no override to resolve — the
only thing a new source can affect is *registration order*, which matters solely because
`SessionStart` context is concatenated in that order. The rule is therefore one sentence, and the
existing precedent already states it: a new entry point is appended **after** the existing ones, so it
cannot reorder a plugin's or a user file's existing `SessionStart` context.

**"The file entry point is what tooling writes."** True, and it is why `hooks.json` exists. But it is an
argument for *keeping* `hooks.json`, not for refusing to read a `[hooks]` table a user wrote by hand in
the file Tact already owns. Codex reads both; a config copied over should not need its hooks moved.

So the change is additive and backward-compatible, and the only genuinely new decision is below.

## 2. One file, one source

`config_search_paths` returns three paths — `<cwd>/.tact/config.toml`, `<cwd>/config.toml`,
`~/.tact/config.toml` — and the config loader **merges** them into one `TactTomlConfig`. Merging is
wrong for hooks: a hook is a reviewed, identity-bearing object, and the review has to name the file it
came from. A merged table would also make "approve this file's hooks" impossible and would let a
project file and the user's file interleave in one anonymous list.

So each config file that declares `[hooks]` becomes **its own** `HookSource`, labelled with its path,
exactly like a `hooks.json`. `TactTomlConfig` is deliberately **not** given a `hooks` field: the hook
collector reads the files directly, so config loading stays untouched and cannot merge them by
accident.

Registration order, ascending specificity, appended after everything that exists today:

```text
plugins → ~/.tact/hooks.json → <workdir>/.tact/hooks.json
        → ~/.tact/config.toml → <workdir>/config.toml → <workdir>/.tact/config.toml
```

`HookOrigin` is `UserFile` for the home one and `ProjectFile` for the two under the working directory,
so the trust store and the reporting treat them like the files they are.

## 3. The same shape, and the same review

`[hooks]` deserialises into the existing `HooksFile` — the TOML spelling of the JSON one, so
`type: "command"` and the new `type: "mcp_tool"` both work, and `additionalContextLimit` and every other
field mean what they mean there.

```toml
[hooks.SessionStart]

[[hooks.SessionStart.hooks]]
type = "command"
command = "brief.sh"
timeout = 5
```

Because the collector reuses `hooks_file_source` and `admit_trusted`, every property holds unchanged:
an entry starts **unreviewed** and is **never registered** until approved, a repository-supplied
`config.toml` cannot execute anything by being cloned, and editing a definition invalidates its
approval. This is the whole reason to route the new spelling through the existing loader rather than a
parallel path.

A file with no `[hooks]` table yields **no source at all**, not an empty one — otherwise every project
with a `config.toml` would report a hook source with nothing in it, and `hooks list` would fill with
noise.

## 4. Non-goals

- **Managed / enterprise hooks and `bypass_trust`.** The last item in that table, and still a
  trust-model decision rather than a missing spelling: `bypass_trust` is a switch that disables the
  review gate this subsystem exists to enforce. `tact-ui hooks trust --all` is the scriptable
  equivalent. Not to be reversed as a side effect of reading one more file.
- Writing `[hooks]` back out. `hooks trust` is the command that mutates hook state; `config.toml` is
  read-only for hooks, so a hand-edited config is never rewritten.
- Any change to `hooks.json` handling, to the identity hash, or to `hook_definition_hash`'s inputs.

## 5. Tests

- A `config.toml` with a `[hooks]` table becomes a source labelled with its path, and its hooks are
  **pending**, not running.
- A `config.toml` without `[hooks]` contributes no source.
- Two config files each declaring `[hooks]` are two sources: approving one admits only that one.
- An `mcp_tool` entry declared in TOML reaches the same code path as one declared in JSON.
- The new sources come **after** the `hooks.json` ones, so existing `SessionStart` context order is
  unchanged.

## 6. Docs sync

- `book/09_chapter_hook.md` + `_zh.md`: the origins table gains `config.toml`, the registration order
  is stated, and the §13 row is removed.
- `config.example.toml`: a commented `[hooks]` example.
- `book/26_chapter_issue.md` + `_zh.md`: newest-first entry.
