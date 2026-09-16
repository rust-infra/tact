# Plan: Opt-in BwrapSandbox for the `bash` tool

Spec: [`../specs/2026-09-15-bwrap-sandbox-design.md`](../specs/2026-09-15-bwrap-sandbox-design.md)
Date: 2026-09-15
Status: implemented (2026-09-15) — see "Result and deviations" at the end

## Work Items

1. **`SandboxBackend` enum** — in `crates/tact/src/config/types.rs`, add
   `pub enum SandboxBackend { None, Bwrap }` with
   `#[derive(Serialize, Deserialize)]` + `#[serde(rename_all = "lowercase")]`,
   `Default` → `None`, `Clone, Copy, Debug`. Keep it in `config/types.rs` so the
   sandbox module can depend on config without a cycle.

2. **Config plumbing** — carry the backend through the settings:
   - `ToolsTomlConfig` (`types.rs:224`): add `pub sandbox: Option<SandboxBackend>`.
   - `ToolSettings` (`types.rs:375`): add `pub sandbox: SandboxBackend`.
   - `NonLlmSettings` (`resolve.rs:488`): add `sandbox: SandboxBackend`.
   - `resolve_config` (`resolve.rs:553`): resolve
     `toml_cfg.tools.sandbox.unwrap_or(SandboxBackend::None)`.
   - `resolve_non_llm_settings` constructors at `resolve.rs:628` and `resolve.rs:826`:
     pass `sandbox` through.
   - `config.example.toml` `[tools]` (after `rtk_filter`): add
     `# sandbox = "none"  # "none" (default) | "bwrap" — Linux bubblewrap sandbox`.

3. **Sandbox module** — new `crates/tact/src/sandbox/`:
   - `mod.rs`: the `Sandbox` trait from the spec (`command(&self, program, args,
     work_dir) -> anyhow::Result<tokio::process::Command>` + `describe() ->
     &'static str`) and a `pub fn resolve(backend: SandboxBackend, work_dir: &Path)
     -> (Option<Arc<dyn Sandbox>>, Option<String>)` — the second element is the
     degradation reason.
   - `bwrap.rs`: `BwrapSandbox` (unit struct) implementing `Sandbox`; a **pure**
     `fn bwrap_args(work_dir: &Path) -> anyhow::Result<Vec<String>>` that produces
     the full §10 flag list (existence-probes the optional system mounts and
     toolchain homes, errors on missing `/lib64`, never emits `--new-session`,
     always emits `--die-with-parent --unshare-net --unshare-pid`); and
     `BwrapSandbox::probe() -> Result<(), String>` that runs
     `bwrap <flags> -- /bin/true` and returns the stderr on failure.
   - `#[cfg(target_os = "linux")]` gate `bwrap.rs`; off Linux `resolve()` returns
     `(None, Some("sandbox backend 'bwrap' is Linux-only"))` for `Bwrap`, and
     `(None, None)` for `None`.

4. **Workspace guard** — in `resolve()` (or `bwrap_args`): refuse `work_dir ==
   "/"`, `work_dir == $HOME`, `work_dir` an ancestor of `$HOME`, or under
   `/etc` `/usr` `/boot` — record it as the degradation reason (§5.1).

5. **`ToolContext`** — `crates/tact/src/tool/mod.rs:107`: add
   `pub sandbox: Option<Arc<dyn Sandbox>>` and
   `pub sandbox_degraded_reason: Option<String>`. Update every construction site:
   `crates/tact-ui/src/interactive.rs:309`, `crates/tact-ui/src/headless.rs:107`,
   `crates/tact/src/tool/test_support.rs:71`, and the `read_image.rs` test helper
   (default `None`/`None`).

6. **Startup resolution + warning** — in `interactive.rs` / `headless.rs`, after
   config is installed and before `Agent::new`: call `sandbox::resolve(...)`; if
   the reason is `Some`, emit the startup notice (`tracing::warn!` with the
   reason, plus `eprintln!` so it is visible to a default CLI run), and stash the
   reason in `ToolContext` for the first-command notice.

7. **`bash` integration** — `crates/tact/src/tool/bash.rs:181`: replace the
   `Command::new("sh")` block with the §11 `match &ctx.sandbox` (Some → `sandbox.
   command(...)`; None → direct `sh -c` with `current_dir(&ctx.work_dir)`, today's
   behaviour). Keep `stdout/stderr`, `kill_on_drop`, `configure_process_group`,
   and every later lifecycle step untouched. In the `None` arm, if
   `sandbox_degraded_reason` is `Some`, log a one-time `tracing::warn!` (a
   `Once` per context) so the degraded command is not silent.

8. **Tool description** — §19.1: in `interactive.rs` / `headless.rs`, right after
   `toolset()` (where the `spawn_subagent` description override already runs),
   call `router.set_tool_description("bash", …)` with the sandbox-aware text
   **only when** the resolved sandbox is `Some` (workspace at `/workspace`,
   network disabled, host home unmounted). Leave the stock description when
   `None`.

9. **Docs** — one pass, after the code compiles and tests pass:
   - `book/07_chapter_tool.md` + `_zh.md`: sandbox insertion point + §19 path space.
   - `book/10_chapter_permission.md` + `_zh.md`: Permission = authorization,
     Sandbox = execution boundary.
   - `book/13_chapter_background.md` + `_zh.md`: `background_run` stays unsandboxed.
   - `book/15_chapter_worktree.md` + `_zh.md`: `worktree_run` / lane git stay unsandboxed.
   - `book/21_chapter_config.md` + `_zh.md`: `[tools] sandbox` field.
   - `config.example.toml`: the key + comment (already in item 2).
   - `book/26_chapter_issue.md` + `_zh.md`: newest-first entry (opt-in sandbox,
     degradation semantics).
   - `docs/agent_guidelines.md`: bash/`/workspace` path-space note.

## Tests

| Crate | Test | Gate |
| --- | --- | --- |
| `tact` | `bwrap_args` unit tests: full flag order; `/lib64` missing errors; optional mounts skipped + reported; no `--new-session`; `--unshare-pid` + `--die-with-parent` present; workspace guard rejects `/`, `$HOME`, `$HOME` ancestor | none |
| `tact` | config parse: `sandbox = "bwrap"` parses, default `None`, unknown value is a TOML error | none |
| `tact` | `resolve()`: `None` → `(None, None)`; `Bwrap` with `bwrap` on PATH + probe passing → `Some`; probe failing / bwrap missing → `(None, Some(reason))` | none (probe result injected or PATH overridden) |
| `tact` | `bash` sandboxed integration (§Tests 2–8, 13): `pwd` = `/workspace`, workspace RW, host home unreachable, `/etc/passwd` readable, network blocked via socket probe, `/tmp` is tmpfs | skip when `bwrap` absent |
| `tact` | §Tests 14/15: timeout + cancel + drop leave **no** surviving processes and pipes reach EOF (`sh -c 'sleep 300 & wait'`); parent death kills the sandbox | skip when `bwrap` absent |
| `tact` | §Tests 17: `/proc` shows only sandbox pids (`sh` is pid 2) and `kill -0 <host pid>` fails | skip when `bwrap` absent |
| `tact` | §Tests 16: `cargo metadata --no-deps` succeeds with toolchain homes ro; a home is not writable from inside | skip when `bwrap`/`cargo` absent |

The skip gate is a `bwrap`-presence check inside the test (or an env var), so
`.github/workflows/rust.yml` / `scripts/check-rust.sh` do not go red on a runner
without bubblewrap; the degradation path is covered by the ungated `resolve()`
tests.

## Verification

- One cargo invocation at a time (no parallel builds against `target/`):
  `no_proxy=127.0.0.1,localhost NO_PROXY=127.0.0.1,localhost cargo test -p tact --lib sandbox::`
  then `… cargo test -p tact --lib tool::bash::` then `… cargo test -p tact --lib config::`.
- `cargo fmt -- --check` + `cargo clippy --all-targets -- -D warnings` (touched crates).
- Manual smoke: `[tools] sandbox = "bwrap"` → `bash` `pwd` prints `/workspace`,
  `curl https://example.com` fails, `cargo metadata --no-deps` passes; `sandbox =
  "none"` (or a host without bwrap) → `bash` behaves exactly as today.

## Notes / decisions

- Opt-in: default `"none"`; the sandbox is **not** on unless the user sets it.
- Fail-open: if `"bwrap"` cannot start (missing bwrap, probe failure, kernel
  restriction, non-Linux), it degrades to `"none"` with a loud warning — never a
  hard tool error, never a silent downgrade.
- `--unshare-pid` adopted (teardown + `/proc` isolation); `--new-session` is
  forbidden (breaks the `killpg` contract — §9.1).
- Toolchain homes are a fixed read-only allowlist: `~/.rustup`, `~/.cargo`,
  `~/.config/git`, `~/.npm`, wired via `RUSTUP_HOME`/`CARGO_HOME`/
  `GIT_CONFIG_GLOBAL`/`NPM_CONFIG_CACHE` because `HOME=/workspace`.
- The `bash` description override reuses the existing
  `ToolRouter::set_tool_description` mechanism (already used by `spawn_subagent`);
  it is applied at startup from the **resolved** sandbox state.

---

## Result and deviations

All nine work items shipped. Deviations from the plan as written, each deliberate:

| Planned | Shipped | Why |
|---|---|---|
| `resolve() -> (Option<Arc<dyn Sandbox>>, Option<String>)` | `…, Option<Arc<SandboxDegradation>>` | The reason string alone cannot carry the once-per-session notice state; `SandboxDegradation { reason, noticed }` does, and `ToolContext` is cloned per invocation so the one-shot must live behind an `Arc`. |
| `ToolContext { sandbox, sandbox_degraded_reason: Option<String> }` | `ToolContext { sandbox, sandbox_degraded: Option<Arc<SandboxDegradation>> }` | Same reason; the two fields stay two fields. |
| "Extend `BASH_METADATA.description` **and** the `BashInput` field doc" (spec §19) | Only the description override | The input schema is generated by the `#[tool]` macro from a static struct, so a field doc cannot vary by session — and the spec's own next sentence forbids advertising `/workspace` in a `none` session. The override is the only backend-aware surface; the static field doc is unchanged. |
| Startup warning in `interactive.rs` / `headless.rs` via `tracing::warn!` | TUI `AgentUpdate::Info` / `eprintln!` there, `tracing::warn!` on the first degraded command in `bash.rs` | `tact-ui` does not depend on `tracing`. The visible channel is at startup; the log line is emitted by the crate that owns the subscriber. |
| Workspace guard: `/`, `$HOME`, ancestors of `$HOME`, `/etc` `/usr` `/boot` | Also refuses `/bin`, `/lib`, `/lib64` | Same class of mistake as `/etc`; refusing them costs nothing. |
| `SandboxBackend { None, Bwrap }` (`[tools] sandbox = "none" \| "bwrap"`) | A boolean `[tools] sandbox`, with the backend chosen per platform in `sandbox::resolve` | Changed 2026-09-16: the backend is not a user choice — only Linux has an implementation, so a backend name invited a config that cannot work on the machine that reads it. The old spellings are a parse error, not an ignored key. |

Three findings from implementing; the first two are recorded in code comments:

- **The toolchain binds create an empty `/home/<user>` mount point inside the sandbox.** The homes are the only entries under it (`.ssh` and everything else are still absent), so this is the documented allowlist, not an accidental home mount — but the sandbox test asserts on the home's *contents*, not on the directory.
- **`--unshare-pid` + `--die-with-parent` teardown verified end-to-end.** Timeout and cancellation both return promptly, both pipes reach EOF, and no `bwrap`/`sh`/`sleep` survives (`tool::bash::sandbox_tests`).
- **`HOME=/workspace` collides with `<workdir>/.tact` when the workspace is a repo root**, because `<workdir>` *is* `/workspace` inside the sandbox. Four `permission::settings` tests read the ambient `$HOME/.tact/settings.json` as their global layer, so running the suite from a sandboxed `bash` picked up the repository's own project settings and failed on rule counts. Those tests now use `PermissionSettings::load_from(project, None)` (the injection point the type already documents) and are independent of the machine's `$HOME`. Verified by pointing `HOME` at a directory holding a non-empty global settings file: all 53 pass.
  The same shape affects any process the sandboxed shell spawns that consults `~/.tact/...`. The tact *process* itself is unaffected — it keeps the real `$HOME`.

Verification run: `cargo fmt -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
`cargo test --workspace` (876 lib tests + integration suites, 0 failures; the six sandboxed-bash
tests execute for real because the host has bubblewrap and skip silently where it is absent).
