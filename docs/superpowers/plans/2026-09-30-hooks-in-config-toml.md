# Plan: inline `[hooks]` tables in `config.toml`

Spec: [2026-09-30-hooks-in-config-toml-design.md](../specs/2026-09-30-hooks-in-config-toml-design.md)

One `cargo` invocation at a time (AGENTS.md: parallel runs contend on `target/`).

1. **Read the table** (`crates/tact/src/plugin/hooks.rs`)
   - `HooksFile::from_toml_file(path)` — parse a whole `config.toml` with `HooksFile`, so the other
     tables are ignored and only `[hooks]` is taken.
   - `config_hook_paths(work_dir) -> Vec<(PathBuf, HookOrigin)>` in ascending specificity: home
     `~/.tact/config.toml` (user), `<workdir>/config.toml`, `<workdir>/.tact/config.toml` (project).
2. **Collect them**
   - `collect_hook_sources_with` appends one `HookSource` per config file that actually declares
     `[hooks]`, after the two `hooks.json` sources. A file with no table adds nothing.
   - Label is the file's path; `dirs` follows the `hooks.json` rule (root = the file's directory,
     data = the `.tact` directory).
3. **Tests**
   - `cargo test -p tact --lib plugin::hooks::`.
   - A `[hooks]` table becomes a labelled source whose hooks are pending; no table means no source;
     two config files stay two sources; an `mcp_tool` entry works from TOML; order is preserved.
4. **Docs sync**
   - `book/09_chapter_hook_zh.md`: origins table row + registration order, §13 row removed.
   - `config.example.toml`: a commented `[hooks]` block.
   - `book/26_chapter_issue_zh.md`: newest-first entry.
