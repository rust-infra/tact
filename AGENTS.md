# Agent guidelines (Tact)

- Small, focused diffs; no commit/push unless asked. Answers are Markdown lists.
- Cargo: one invocation at a time (parallel runs fight the `target/` lock); filter tests, then widen; channel-waiting async tests need timeouts.
- Before pushing: `scripts/check-rust.sh` (with a proxy exported, also `no_proxy=127.0.0.1,localhost,::1`). Outside network: `export https_proxy=http://127.0.0.1:7890 http_proxy=$https_proxy all_proxy=$https_proxy`.
- Docs to sync before pushing (`book/*_zh.md` Chinese, rest English): compaction/loop → `book/05` + `docs/compaction.md`; store → `book/01`; tokens → `docs/token_usage_schema.md`; config → `config.example.toml` + chapter; feature → `docs/superpowers/{specs,plans}`; user-visible fix → `book/26`.
- Never regress: TUI cells paint their full area and test blank `theme.bg` (`book/26`); injected context is not a user turn (`docs/compaction.md`); hosted web search injects, never replaces, and is no `ToolUse` (`book/22` §6.2.1.1).
