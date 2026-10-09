# Plan: sensitive-path guard + secret redaction

Spec: [2026-09-30-sensitive-path-guard-and-secret-redaction-design.md](../specs/2026-09-30-sensitive-path-guard-and-secret-redaction-design.md)

One `cargo` invocation at a time (AGENTS.md: parallel runs contend on `target/`).

Tasks 1–4 are the guard and must land before 5–6 (redaction needs the same hit to pick its level).
Each task is independently testable and committable. Suggested commit types follow the repo's
`type(scope): what it does` convention.

---

1. **`security::sensitive` — the registry and the classifiers** (new: `crates/tact_extensions/src/security/mod.rs`,
   `crates/tact_extensions/src/security/sensitive.rs`; `crates/tact/src/lib.rs` gains `pub(crate) mod security;`)
   - `Tier { Credential, Secret }`, `SensitiveKind { PrivateKey, CredentialStore, AgentConfig, EnvFile,
     ShellHistory, KeyMaterial }`, `Hit { tier, kind, matched, pattern }` — all with the doc comment
     explaining the two tiers and why the escape hatches differ.
   - The §3.3 tables as three `const` slices: `HOME_DIR_PATTERNS`, `FILENAME_PATTERNS`, `SECRET_PATTERNS`.
     Write them out in full — the enhanced list is the deliverable, not a sample of it. Exclusions
     (`*.pub`, `*.crt`, `*.cer`, `*.der`, `*.example`, `*.sample`, `*.template`, `*.dist`, and
     `~/.ssh/{config,known_hosts}` being demoted to `Secret`) are their own documented const so the
     intent survives a refactor.
   - `classify_path_with_home(raw: &str, home: Option<&Path>) -> Option<Hit>` — normalise (leading
     `~`, `$HOME`, `${HOME}`), then home-relative match, then filename-glob match. Pure; **the tests use
     this, never the env**.
   - `classify_path(raw: &str)` — `$HOME` via a `OnceLock`, mirroring `tool/mod.rs`'s existing pattern.
   - `classify_command(command: &str) -> Option<Hit>` — tokenise on metacharacters/whitespace, strip
     quotes, classify each token, plus the substring scan. Doc-comment the bypass list explicitly
     (command substitution, variables, `python -c`) so the next reader does not mistake it for complete.
   - `Scanner { home: Option<PathBuf>, enabled: bool, extra: Vec<String>, allow: Vec<String> }` with
     `Scanner::from_config(&SecurityConfig)` and `fn classify(&self, raw: &str) -> Option<Hit>` applying
     the `allow` > `extra` > built-in precedence. `Scanner::permissive()` for tests.
   - `SecurityConfig { sensitive: …, redaction: … }` — the *parsed* form of the settings object, kept in
     this module, filled in task 2.
   - Tests: `registry_table` (one assertion per row of §3.3, driven by the same consts so a new row
     without a test case is impossible), `exclusions_stay_excluded`, `home_relative_only_at_home`,
     `allow_entry_lifts_a_credential_hit`, `extra_pattern_adds_a_hit`,
     `classify_command_catches_plain_reads`.
   - Commit: `feat(security): a registry of paths that are secrets by their nature`

2. **Config plumbing** (`crates/tact_extensions/src/permission/settings.rs`)
   - Read `permissions.sensitive_paths` (`enabled`, `extra`, `allow`) and `permissions.redaction`
     (`enabled`, `level`, `extra_patterns`, `ignore_paths`) out of the same document
     `extract_rule_lists` already parses. Tolerant, exactly like the rule lists: a malformed key warns
     and falls back to its default, never fails the load, and is **preserved** on
     `persist_project_allow`'s round-trip (that path re-serialises the whole document — there is an
     existing test for field preservation; extend it).
   - `PermissionSettings::security_config(&self) -> SecurityConfig`, merging global then project: `allow`
     and `extra` union, `enabled` = project if set else global, `level` = first set.
   - An invalid `redaction.level` warns and falls back to `basic` (never to `off` — a typo must not
     silently disable redaction).
   - Tests: defaults when absent; each key present; malformed values; union across global + project;
     the round-trip preservation test extended with both new objects.
   - Commit: `feat(permission): settings carry the sensitive-path and redaction config`

3. **`PermissionPolicy` learns about its target** (`crates/tact_extensions/src/tool/metadata.rs`, plus every
   metadata const that carries a path)
   - Add `ReadPath { path_field }`, `WritePath { path_field }`, `PatchPaths`. `resolve` returns `High`
     when `classify_path` hits, unchanged otherwise; `sensitive(&self, input, scanner) -> Option<Hit>`
     does the field lookup. Keep `resolve`'s signature — `tool_dispatch.rs:649` is its only caller.
   - `ShellCommand::resolve` calls `classify_command` **before** `is_read_only_shell_command`, and
     `sensitive` returns the command-string hit for the deny path.
   - `PatchPaths` reuses `parse_unified_diff` (already extracts `FilePatch.path` from `+++ b/<path>`;
     do not write a second parser) to collect every target path; any hit makes the whole patch
     `Credential`-deny or `High`.
   - **Fix the stale prompt policy in the same pass:** `APPLY_PATCH_METADATA.permission_prompt` names
     `path`, a field `ApplyPatchInput` does not have, so `PermissionRule::generate` falls back to a bare
     `apply_patch` rule and one "Always allow this tool" click permits *every* future patch. Add a
     `PermissionPromptPolicy::PatchTarget { patch_field }` that derives the rule from the patch's target
     path (`apply_patch(patch:*<path>*)`, globbed against the `patch` field) and never falls back to a
     bare rule; a patch whose targets cannot be parsed must generate a rule that matches nothing so it
     asks again. Regression test: `apply_patch_always_allow_is_input_aware`.
   - **File-tool pre-filter:** in `ReadPath`/`WritePath`, skip the guard when the raw path is absolute or
     starts with `~` — `safe_path` rejects those with "Path escapes workspace" anyway and a prompt before
     an inevitable error is noise. One comment, one test.
   - Metadata updates: `read_file`/`read_image` → `ReadPath`, `edit_file`/`write_file` → `WritePath`,
     `apply_patch` → `PatchPaths`. `bash`/`background_run`/`worktree` keep `ShellCommand`.
   - Rewrite the now-false `~`-is-harmless comment in `readonly_shell.rs` and add the test that proves
     the safelist still accepts `~/notes.txt` — the guard narrows what tilde expansion can *reach*, it
     does not remove tilde support.
   - Tests: per-tool `resolve` with a sensitive and a benign path; `.env` → `High`, `src/main.rs` →
     `Read`; `apply_patch` with a `+++ b/secrets.json` header → `High`; `ls ~` still `Read`.
   - Commit: `feat(security): a tool's target, not just its verb, decides the risk`

4. **The guard in preflight** (`crates/tact_extensions/src/agent/tool_dispatch.rs`, `crates/tact_extensions/src/agent/mod.rs`,
   `crates/tact_ui/src/{interactive,headless}.rs`, `crates/tact_extensions/src/tool/subagent.rs`)
   - `AgentRuntime` gains `security: Scanner`; built from `PermissionSettings::security_config()` at each
     of the three construction sites (the subagent inherits the parent's scanner alongside its
     `PermissionSnapshot`).
   - In `preflight_tool_calls`, after `risk` is computed for the native arm: compute `hit`; if
     `hit.tier == Credential`, emit `StepFailed` with the refusal text, push
     `PreparedState::Resolved(refusal)`, `continue` — **before** `invoke_hooks!(PreToolUse)` and before
     `check_with_auto`, so `Auto` mode and a settings `allow` rule cannot reach it.
   - The refusal text is a function in `security::sensitive` (not an inline format string): it names the
     matched path, the kind, and the `sensitive_paths.allow` escape, because it is read by both the model
     and the human.
   - `Secret`-tier hits need no new code beyond `resolve` returning `High` — assert that by test rather
     than adding a branch.
   - Non-interactive: nothing to do — `High` already denies there, and `Credential` denies in
     interactive too.
   - Tests: `.env` read asks in Default / denies in Plan; `id_ed25519` denies in `Auto`, in
     non-interactive, with a settings `allow` rule, and with an in-session always-allow; the same call
     with a matching `sensitive_paths.allow` entry asks instead. Put the end-to-end ones in
     `agent::tool_dispatch`'s test module (the decision lives there, unlike the MCP router case).
   - Commit: `feat(security): a private key is refused before any mode or rule can allow it`

5. **`security::redact` + final-result redaction** (new `crates/tact_extensions/src/security/redact.rs`;
   `crates/tact_extensions/src/agent/tool_dispatch.rs`)
   - `RedactionLevel { Off, Basic, Credential }`, `redact(text, level, &RedactionConfig) -> Cow<str>`
     and `redact_for_call(text, hit: Option<&Hit>, cfg)` — the latter picks `Credential` when the call
     was classified sensitive or its result came from an `ignore_paths`-exempt location, `Basic`
     otherwise.
   - The §4.1 pattern tables, compiled once in a `OnceLock<RegexSet>`/`Vec<(Regex, &str)>`; the
     Credential rules are line-anchored so they are only ever run on whole text, never on a chunk.
   - Markers carry the category only (`[redacted:api-key]`), never a value prefix — assert that in a
     test so nobody "improves" it later.
   - Wire it at the single `ExecResult { content }` construction in `run_tool_waves`, covering the native
     and MCP paths at once, before `build_tool_results` turns it into a `ContentBlock::ToolResult`.
     Deliberately **not** applied to `ToolUse.input` / `arg_full` — the model's own call must round-trip
     byte-identical (spec §4.2); add the comment saying so.
   - Tests: one per row of §4.1; `redact_does_not_touch_source` (`let token = compute(x);`,
     `api_key: String`, and a bare `token=` word survive `Basic`; `user:pass@host` does not);
     `redaction_off_is_honoured`; `session_store_holds_redacted_text` (round-trip through the store).
   - Commit: `feat(security): redact secrets out of tool results before they are stored`

6. **Live-output redaction** (`crates/tact_extensions/src/security/redact.rs`,
   `crates/tact_extensions/src/tool/progress.rs`, `crates/tact_extensions/src/tool/bash.rs`)
   - `StreamRedactor { holdback, level }` with `push(&mut self, chunk) -> Option<String>` and
     `finish(&mut self) -> Option<String>`; hold back the longest possible partial match.
   - Own it per invocation in the progress reporter (built in `ToolContext::for_invocation`, so the
     per-invocation state is natural), `Basic` rules only — the Credential rules are line-anchored and
     cannot be evaluated on a partial line, which is why the final-result pass in task 5 still runs.
   - `finish()` must be called on every exit path, including the error and cancel paths; a missed flush
     silently truncates output. Add an assertion for the cancel path.
   - Tests: `stream_redactor_holds_back_boundary` (a token split across two `push` calls is never
     emitted whole), `finish_flushes_the_holdback`, `no_holdback_leaks_on_error_path`.
   - Commit: `feat(security): live command output is redacted as it streams`

7. **Full-suite pass** (one invocation at a time)
   - `cargo test -p tact --lib security::` → `permission::` → `agent::tool_dispatch::` → `tool::`
   - `cargo test -p tact-ui --lib`
   - `cargo clippy -p tact --all-targets` — the new `&'static str` fields and the const tables are the
     kind of thing clippy is noisy about.
8. **Docs sync** (per AGENTS.md's trigger table — both languages, structurally aligned)
   - `book/10_chapter_permission_zh.md`: the `Read` short-circuit caveat (step 1 of
     `check_with_auto` returns before plan mode — this is why the guard is not a risk tier); the two
     tiers and their different escape hatches; §9 Configuration gains the two JSON objects with the
     precedence table; §7 gains the `bash` tilde paragraph and the corrected rationale; §10 code map
     gains `security/sensitive.rs` + `security/redact.rs`; §11 Current Gaps gains the "the guard is a
     name-based heuristic; the real boundary is the Linux-only, opt-in sandbox" row.
   - `book/26_chapter_issue_zh.md`: two newest-first entries — (a) the leak: `cat
     ~/.ssh/id_ed25519` ran silently as a `Read`, and `~/.claude/settings.json` printed a live token;
     (b) the `apply_patch` bare-rule bug (§1.4 of the spec), which is a permission-scope bug worth its
     own row. Both need date, symptom, decision, observable behaviour, pointers.
   - `ARCHITECTURE.md` §3: the guard as a pre-step in the permission diagram, and the note that
     `Credential` is evaluated before the mode.
   - Skip the TUI-rendering doc: the refusal reuses `PreparedState::Resolved`, so nothing renders
     differently.
