# Design: sensitive-path guard + secret redaction

Date: 2026-09-30
Status: implemented (see Ch 10 §12 for the as-built contract)

Answers the question raised by the `~/.ssh` incident: *why could the agent read a private key, and what
stops it from happening again?* Two independent mechanisms, because they fail independently:

- **A guard** — refuse (or ask for) access to files that are credential material by their nature.
- **Redaction** — a backstop, for everything the guard cannot see.

The guard is a policy. Redaction is a property of the pipe. Neither is a sandbox — see §6.

## 1. Why the hole exists today

Three facts, each verified in this tree:

1. **The file tools are already confined.** `tool::safe_path` canonicalises `work_dir.join(path)` and
   rejects anything that escapes it (`crates/tact/src/tool/path.rs:12`). `read_file("~/.ssh/id_ed25519")`
   does not expand `~` — it looks for a literal `~` directory under the workspace — and an absolute path
   replaces the join and is then rejected. **File tools cannot reach `~/.ssh`.**

2. **`bash` can, and is auto-allowed.** `PermissionPolicy::ShellCommand::resolve`
   (`crates/tact/src/tool/metadata.rs:53`) returns `CapabilityRisk::Read` for anything
   `readonly_shell::is_read_only_shell_command` accepts. `cat` is on that safelist, and
   `split_plain_command` explicitly **accepts a leading `~`**, with the comment: *"a leading `~` is
   harmless — tilde expansion only substitutes the home directory, and every safelisted program stays
   read-only on the expanded result."* That comment is the bug: `cat` stays read-only *on the file*, but
   the file's **contents** are the secret. So `cat ~/.ssh/id_ed25519` resolves to `Read`, and
   `check_with_auto` step 1 returns `Allow` before plan mode, before settings rules, before everything.
   It runs silently, in every mode, in headless too. `cat ~/.netrc` is the same call.

3. **Nothing is redacted on the way out.** Whatever a tool returns is stored verbatim in the session
   store and the transcript. `redact_query_value` exists (`crates/tact/src/mcp/remote.rs:962`) but only
   for one OAuth URL field. There is no secret scanner anywhere on the tool-result path.

4. **`read_file` was seeded into `always_allowed_tools`, and a bare name grants every input.**
   Both `PermissionManager::try_new` and `try_new_with_settings` started with
   `vec!["read_file"]`. While `read_file` was always classified `Read` the entry
   was inert. Once §3 makes a sensitive target escalate it to `High`, the entry
   is consulted by the `High` branch and lets **every** `read_file` input
   through, `.env` included. An allow-list entry nobody granted must not outrank
   the guard, so the seed is removed — and the loss is nil, because `read_file`
   is still auto-allowed by virtue of being `Read`.

5. **`apply_patch` has no path in its prompt policy, so "Always allow" silently means "always".**
   `APPLY_PATCH_METADATA` declares `permission_prompt: PermissionPromptPolicy::Path { field: "path" }`,
   but `ApplyPatchInput` has only `patch` and `dry_run` — there is no `path` field, and the metadata's
   `ResourcePolicy::PatchFiles` names the real one (`patch_field: "patch"`). `PermissionRule::generate`
   therefore finds no string at `path`, falls back to `PermissionRule::Bare { tool }`, and persists
   `apply_patch` as a **bare, input-blind allow rule**. One click on *Always allow this tool* after
   editing one file grants every future patch, in every session, forever. Unrelated to secrets in
   mechanism, identical in consequence — a broad permission granted by a narrow-looking gesture. Fixed
   here because the same change touches the same metadata.

6. **The bare fallback is worse than §1.5 suggests: it fires on any value containing `(`, `)` or `:`.**
   The rule grammar embeds the pattern in `tool(field:pattern)`, so a value it cannot represent fell
   back to a bare rule — and `bash` on `git commit -m "fix: thing"` contains a `:`. One click on
   *Always allow this tool* therefore persisted an input-blind `bash` rule: **every future shell
   command, in every session**. Same defect class as §1.4/§1.5, larger blast radius, and reachable
   without any secret being involved. `generate` returns `None` for these now.

So the leak was not "the sandbox failed" — the sandbox was off and is Linux-only (§6) — it was that a
read of a private key is classified as a *safe read*.

## 2. Constraints the design must respect

| Constraint | Source | Consequence |
|---|---|---|
| `Read` is allowed **before** plan mode is consulted | `check_with_auto` step 1 | A guard bolted onto the risk ladder cannot work for read tools; it must be decided earlier or be its own outcome. |
| A settings `allow` rule outranks the `High` prompt | `check_with_auto` steps 4–6 | Anything expressed as "risk = High" is one click away from permanent approval. Fine for a `.env`; not fine for `id_ed25519`. |
| `PreparedState::Resolved(msg)` renders a persisted refusal | `crates/tact/src/agent/tool_dispatch.rs:672` | A hard denial already has a home: it emits `StepFailed`, skips the prompt, and lands in the tool result. No new UI. |
| `~`/absolute paths cannot be read by the file tools anyway | `safe_path` | The file-tool guard only ever needs to cover **in-workspace** paths. Home-path matching is a `bash` concern. |

## 3. Part A — the sensitive-path guard

### 3.1 New module

```
crates/tact/src/security/
├── mod.rs        // re-exports; lib.rs adds `pub(crate) mod security;`
├── sensitive.rs  // SensitiveKind, Tier, Hit, Scanner, classify_*()
└── redact.rs     // Part B
```

Zero new dependencies: `regex` and `globset` are already in `crates/tact/Cargo.toml`.

### 3.2 Two tiers, two outcomes

The tiers exist because "sensitive" spans two very different things, and collapsing them gives either a
useless warning or a wall.

| Tier | Meaning | Decision | Escapable by |
|---|---|---|---|
| `Credential` | The file **is** the secret: private keys, token stores. No legitimate agent read. | **Deny** — refuses before the prompt, no popup | a hand-written `permissions.sensitive_paths.allow` entry |
| `Secret` | Reading it *may* expose a secret, and the user may well want it (`.env` during a config task, shell history during a debugging task) | **Ask** — the normal `High` prompt, so `deny`/`allow`/`ask` rules and "Always allow" all still work | the existing "Always allow this tool" button |

`Credential` is deliberately **not** reachable by the TUI's "Always allow" button: a refusal whose escape
hatch is one click away is not a refusal. It is reachable by editing `.tact/settings.json` — an explicit,
persisted, reviewable act.

### 3.3 The registry

Every pattern below, split by *where* it applies. `~` is the home dir resolved from `$HOME`
(`TactPath::home_tact_dir` already does this; the scanner keeps its own copy so it can be unit-tested
without env vars).

**Tier `Credential` — home-relative**

| Pattern | Note |
|---|---|
| `~/.ssh/**` minus `config`, `known_hosts`, `*.pub` | the directory itself, `authorized_keys`, `id_*` |
| `~/.netrc`, `~/_netrc` | machine/login/password pairs |
| `~/.authinfo`, `~/.authinfo.gpg`, `~/.msmtprc`, `~/.fetchmailrc` | same class, other clients |
| `~/.aws/**` | `credentials`, `config` |
| `~/.gnupg/**` | GPG keyring |
| `~/.kube/**`, `~/.docker/**`, `~/.terraform.d/**` | cluster / registry / tf credentials |
| `~/.config/gh/**`, `~/.config/gcloud/**`, `~/.azure/**`, `~/.config/heroku/**`, `~/.heroku/**` | CLI token stores |
| `~/.config/op/**` | 1Password CLI |
| `~/.git-credentials`, `~/.npmrc`, `~/.pypirc`, `~/.pgpass`, `~/.my.cnf`, `~/.s3cfg`, `~/.boto`, `~/.gem/credentials`, `~/.cargo/credentials.toml`, `~/.vault-token`, `~/.huggingface/token` | single-file token stores |
| `~/.claude/settings.json`, `~/.claude/.credentials.json`, `~/.claude.json`, `~/.codex/auth.json`, `~/.tact/settings.json` | **agent-host config that carries `env` tokens — this is the file that leaked.** Narrowed to files, not `~/.claude/**`, so `CLAUDE.md` / memories stay readable |

**Tier `Credential` — filename, anywhere**

| Pattern | Note |
|---|---|
| `id_rsa`, `id_dsa`, `id_ecdsa`, `id_ed25519`, `*_rsa`, `*_dsa`, `*_ecdsa`, `*_ed25519`, `*_sk` | private keys; `*.pub` excluded |
| `*.pem`, `*.key`, `*.p12`, `*.pfx`, `*.jks`, `*.keystore`, `*.ppk`, `*.kdbx` | key material, cert bundles, KeePass |
| `*.tfstate`, `*.tfstate.backup`, `*.tfvars` | terraform state holds provider secrets |
| `credentials.json`, `credential.json`, `service-account*.json`, `service_account*.json` | cloud service accounts |
| `secrets.json`, `secrets.yml`, `secrets.yaml`, `secrets.toml`, `secret.json`, `secret.yml` | the conventional names |
| `.npmrc`, `.pypirc`, `.pgpass`, `.netrc`, `.git-credentials`, `.htpasswd`, `.my.cnf`, `.s3cfg`, `.boto` | in-repo copies of the single-file stores above; a checked-in `.npmrc` with `_authToken` is a real leak |

**Tier `Secret` — filename / home, anywhere**

| Pattern | Note |
|---|---|
| `.env`, `.env.*`, `*.env` | the everyday case; `.env.example`/`.sample`/`.template`/`.dist` excluded |
| `.envrc`, `.direnv/**` | direnv can export tokens |
| `~/.ssh/config`, `~/.ssh/known_hosts` | topology, not key material |
| `~/.bash_history`, `~/.zsh_history`, `~/.sh_history`, `~/.python_history`, `~/.psql_history`, `~/.mysql_history`, `~/.sqlite_history`, `~/.node_repl_history`, `~/.irb_history`, `~/.wget-hsts` | secrets typed on the command line end up here |
| `~/.config/**/history*` | the same, for other CLIs |

**Deliberate exclusions** (documented so they are decisions, not oversights):

- `*.pub`, `*.crt`, `*.cer`, `*.der` — public by definition. (`*.pem` is kept: it is far more often a
  private key than a chain, and a needless prompt is cheap; `allow` covers the certbot case.)
- `~/.ssh/known_hosts`, `~/.ssh/config` — `Secret`, not `Credential`.
- `~/.claude/**` as a directory — only the token-bearing files are `Credential`.
- `**/testdata/**`, `**/fixtures/**` — **not** globally excluded. A fake key in a fixture is still a
  fixture the user may not want summarised. The `allow` list is the escape.

### 3.4 Matching

```rust
pub enum Tier { Credential, Secret }

pub struct Hit {
    pub tier: Tier,
    pub kind: SensitiveKind,   // PrivateKey | CredentialStore | AgentConfig |
                               // EnvFile | ShellHistory | KeyMaterial
    pub matched: String,       // the raw token that matched, for the reason text
    pub pattern: String,       // the registry entry, for the reason text
}

pub fn classify_path_with_home(raw: &str, home: Option<&Path>) -> Option<Hit>;
pub fn classify_path(raw: &str) -> Option<Hit>;              // home = $HOME (OnceLock-cached)
pub fn classify_command(command: &str) -> Option<Hit>;        // Part A.3
```

`classify_path` normalises first — expands a leading `~`/`$HOME`/`${HOME}`, then tries, in order:
home-relative match (the path is under `home`), then filename-glob match against the final component.

`classify_command` is a heuristic and is documented as one. It tokenises the raw command on shell
metacharacters and whitespace, strips surrounding quotes, and classifies each token; it also
substring-scans for `~/.ssh`, `$HOME/.ssh`, `~/.netrc`, `.env`-shaped and `*.pem`-shaped tokens, so a
quoted or partially-quoted path is still caught. It does **not** resolve command substitution, variables,
or `python -c`. That is a stated limit, not an oversight — §6 says what actually stops that.

### 3.5 Where it is enforced

The tool's own metadata stays the single place that knows which input field names a path (no
tool-name table). `PermissionPolicy` gains the target-aware variants it was missing:

```rust
pub enum PermissionPolicy {
    Read,                                     // unchanged: no path target (ls-style, metadata-only)
    Write,                                    // unchanged
    High,                                     // unchanged
    ReadPath  { path_field: &'static str },   // read_file, read_image
    WritePath { path_field: &'static str },   // edit_file, write_file
    PatchPaths,                               // apply_patch
    ShellCommand { command_field: &'static str },
}
```

`resolve(input)` keeps its signature and its meaning (*the risk*), and returns `High` on any hit, so the
`Secret` tier needs no new machinery. The hard `Credential` deny rides a second method, because a
`CapabilityRisk` cannot express "deny":

```rust
impl PermissionPolicy {
    pub fn resolve(&self, input: &Value) -> CapabilityRisk;              // unchanged call site
    pub fn sensitive(&self, input: &Value, scanner: &Scanner) -> Option<Hit>;
}
```

`Scanner` carries the home dir plus the user's `enabled` / `extra` / `allow` overlay, and is built once
at session start from `PermissionSettings`. It lives on `AgentRuntime` beside `permission_manager`.

In `preflight_tool_calls` — the one place that already resolves both metadata and risk:

```
let hit   = metadata.permission.sensitive(input, scanner);       // native tools only
let risk  = if hit.is_some() { High } else { metadata.permission.resolve(input) };
if let Some(Credential) = hit.tier {
    // before hooks, before the prompt, before PermissionRequest
    emit StepFailed(refusal_reason);  →  PreparedState::Resolved(refusal_reason)
    continue;
}
```

Ordering is the whole point: the deny is evaluated **before** `invoke_hooks!(PreToolUse)` and before
`check_with_auto`, so no mode (`Auto` included) and no settings `allow` can reach it. `Secret` is not
special-cased at all — it becomes `High` and then follows the ordinary path, so plan mode denies it,
`Auto` mode still allows it, non-interactive denies it, and user rules compose normally.

For `ShellCommand`, `resolve` calls `classify_command` **before** `is_read_only_shell_command`, and the
`~`-is-harmless comment in `readonly_shell.rs` is rewritten: tilde expansion remains free, but the
safelist proves only that the *program* cannot write, never that its output is safe to publish.

For `PatchPaths`, the target paths come from `parse_unified_diff`, which already extracts them into
`FilePatch.path` from the `+++ b/<path>` / `+++ <path>` header — the parser is reused rather than
re-implemented. Any hit makes the whole patch `Credential`-deny or `High`. The stale
`permission_prompt: Path { field: "path" }` on `apply_patch` (§1.4) is corrected in the same change so
that "Always allow this tool" generates an input-aware rule instead of a bare, tool-wide one: a new
`PermissionPromptPolicy::PatchTarget { patch_field }` derives the rule from the patch's target path
(`apply_patch(patch:*<path>*)`, matched as a glob against the `patch` field), and a patch whose targets
cannot be parsed generates a rule that matches nothing, so it asks every time. A bare
`apply_patch` rule is never produced — it is the outcome this change removes.

The refusal text is written to be read by both audiences:

```
Refused: ~/.ssh/id_ed25519 is credential material (private-key). Reading it is not
something this agent does. If the user asked for this, they can permit the path in
.tact/settings.json under permissions.sensitive_paths.allow.
```

## 4. Part B — redaction

The guard is name-based and heuristic. Redaction is the part that still works when it is bypassed
(`python -c "print(open(...).read())"`, a variable holding a path, an MCP tool, a script the agent wrote
two turns ago).

### 4.1 Two levels, because blanket redaction corrupts source code

A generic `(?i)(token|secret|key)\s*=\s*\S+` rule applied to every tool result would rewrite the user's
own Rust source and test fixtures — the model would then be reasoning about `[redacted]` where the code
says `token = "abc"`. So:

| Level | Applied to | Patterns |
|---|---|---|
| `Basic` | **every** tool result | Only high-confidence, low-false-positive shapes |
| `Credential` | results from a call the guard classified sensitive, plus any result whose *input* named a sensitive path | `Basic` + structure-aware key/value rules |

`Basic`:

| Pattern | Marker |
|---|---|
| `-----BEGIN (RSA \|EC \|DSA \|OPENSSH \|PGP )?PRIVATE KEY-----` … `-----END …-----` | `[redacted:private-key]` |
| `sk-[A-Za-z0-9_-]{20,}`, `sk-ant-[A-Za-z0-9_-]{20,}` | `[redacted:api-key]` |
| `AKIA[0-9A-Z]{16}`, `ASIA[0-9A-Z]{16}` | `[redacted:aws-key]` |
| `ghp_`/`gho_`/`ghu_`/`ghs_`/`ghr_` + 36, `github_pat_[A-Za-z0-9_]{22,}` | `[redacted:github-token]` |
| `xox[baprs]-[A-Za-z0-9-]{10,}` | `[redacted:slack-token]` |
| `glpat-[A-Za-z0-9_-]{20,}` | `[redacted:gitlab-token]` |
| `AIza[0-9A-Za-z_-]{35}` | `[redacted:google-key]` |
| `eyJ…eyJ…` (three base64url segments, `{10,}` each) | `[redacted:jwt]` |
| `Bearer\s+[A-Za-z0-9._-]{20,}` | `[redacted:bearer]` |
| `https?://[^/\s:@]+:[^/\s:@]+@` | `[redacted:basic-auth]` |

`Credential` adds, and **keeps the key name** so the shape survives:

| Pattern | Result |
|---|---|
| `.netrc`: `^\s*(machine \S+ login \S+ )?password \S+` | `password [redacted:value]` |
| dotenv / ini / TOML: `^\s*[A-Z0-9_]*(PASSWORD\|PASSWD\|SECRET\|TOKEN\|API_?KEY\|ACCESS_?KEY\|PRIVATE_?KEY\|CREDENTIAL\|AUTH)[A-Z0-9_]*\s*[:=]\s*.*$` | `API_KEY=[redacted:value]` |
| JSON: `"[^"]*(password\|secret\|token\|api_?key\|auth)[^"]*"\s*:\s*"[^"]*"` | `"api_key": "[redacted:value]"` |
| npmrc: `^\s*//\S+:_authToken=.*$` | `//registry.npmjs.org/:_authToken=[redacted:value]` |
| PEM/SSH `/etc/hosts`-style `ssh-…` authorized_keys lines | `[redacted:authorized-key]` |

Markers carry the category, never a prefix of the value: a 4-char prefix is still 4 characters of the
secret, and the marker is what the model needs.

### 4.2 What is redacted and what is not

| Redacted | Not redacted | Why |
|---|---|---|
| Tool **results** (native + MCP) | Tool **use inputs** / `arg_full` | The model's own `tool_use` block must round-trip byte-identical or the next request is malformed. If the model emitted a secret it already had it — redacting the echo does not un-leak it. |
| `bash` stdout/stderr, final **and** live | Hook stdout | Hooks are user-authored; their output is already the user's own text. |
| Step detail / collapsed result view | Compaction summaries (they carry the already-redacted text) | Single choke point, upstream. |

### 4.3 Placement

One choke point for the final text: `run_tool_waves` builds `ExecResult { content, … }` for both the
native and MCP paths — redact there. That covers the TUI detail view, the transcript, and the session
store in one edit, and it happens before `build_tool_results` assembles `ContentBlock::ToolResult`.

Live streaming is a separate problem: bash emits chunks through `ctx.progress_reporter.report(…)` while
the process runs, so a secret straddling a chunk boundary would be shown half-formed. `redact.rs`
therefore also provides a `StreamRedactor`:

```rust
pub struct StreamRedactor { holdback: String, level: RedactionLevel, … }
impl StreamRedactor {
    pub fn push(&mut self, chunk: &str) -> Option<String>;  // returns the safe prefix, None if still held
    pub fn finish(&mut self) -> Option<String>;             // flush; called on step end
}
```

It retains the last `MAX_PATTERN_LEN` bytes (the longest possible partial match) and emits nothing beyond
that until the next chunk or `finish()`. It is owned per invocation by the tool context's progress
reporter. Redacting live bytes is best-effort by construction — `Basic` patterns only, since
`Credential`'s line-anchored rules cannot be evaluated on a partial line.

### 4.4 Configuration

Both parts are configured from `.tact/settings.json` (project) merged with `$HOME/.tact/settings.json`
(global), the same tolerant-JSON store that already holds `allow`/`ask`/`deny` and already preserves
unknown keys. No TOML surface: `[permission]` in `config.toml` stays `mode`-only, so there is one place
where security rules live.

```jsonc
{
  "permissions": {
    "sensitive_paths": {
      "enabled": true,
      "extra":  ["**/my-vault/**"],                  // added to the built-in registry
      "allow":  ["~/.ssh/config", "**/fixtures/**"]  // exempt: no guard at all
    },
    "redaction": {
      "enabled": true,                               // false = the documented escape hatch
      "level": "basic",                              // "basic" | "credential" | "off"
      "extra_patterns": ["MYCO-[0-9a-f]{32}"],
      "ignore_paths": ["**/fixtures/**"]             // result came from here → Basic only
    }
  }
}
```

Precedence inside the guard: `allow` (exempt) > `extra` (add) > built-in registry. An explicit
`allow` entry lifts even the `Credential` deny — that is the whole point of requiring a file edit.

`redaction.enabled = false` disables redaction globally. It exists because a legitimate task ("what
format is our token in?") is impossible otherwise, and a switch that cannot be flipped is a switch
people work around. It is global and persisted, never per-call.

## 5. Observable behaviour after this change

| Situation | Today | After |
|---|---|---|
| `bash "cat ~/.ssh/id_ed25519"` | runs silently, key lands in context + session DB | refused, no prompt, refusal text recorded as the tool result |
| `bash "cat ~/.netrc"` | same | refused |
| `bash "cat .env"` | runs silently (Read) | asks; `Always allow` makes it stick; non-interactive denies |
| `bash "python -c 'print(open(\"/Users/me/.ssh/id_ed25519\").read())'"` | runs silently | still classified `Write`→ **asks** (not read-only), and the result is redacted if approved. Guard bypassed; redaction holds |
| `read_file ".env"` (in workspace) | allowed before plan mode | asks; denied in plan mode; denied headlessly |
| `read_file "~/.ssh/id_ed25519"` | `No such file` (never worked) | unchanged; the guard is skipped for `~`/absolute paths in file tools, since `safe_path` already refuses them |
| `read_file "src/main.rs"` | allowed | unchanged — `Basic` redaction only touches high-confidence shapes |
| `read_file "tests/fixtures/aws.json"` | allowed | asks once (a `credentials.json`-shaped name); `allow` or the settings rule makes it stick; the fake key is redacted in the result |
| `bash "cat /Users/me/.aws/credentials"` in `Auto` mode | allowed | refused — the `Credential` deny is evaluated before the mode |
| any tool result containing `sk-…` | stored verbatim | `[redacted:api-key]`, in the session store too |
| plan mode, `read_file ".env"` | allowed | denied (`High` is blocked in plan mode) — intentional, see §6 |
| "Always allow this tool" on `apply_patch` | persists a bare `apply_patch` rule — every future patch, any file | persists an input-aware rule naming the patch's target path; a multi-file patch asks again |

## 6. What this does not fix

- **It is not a sandbox.** Both parts are name-based heuristics in the same process as the agent. A
  determined path (`bash -c "cat $(echo ~)/.ssh/id_ed25519"`, a compiled helper, an MCP tool) gets past
  the guard; redaction is the only thing left, and it is pattern-based too. The real boundary is
  `crates/tact/src/sandbox/` — bwrap — which today is **Linux-only and opt-in**
  (`SandboxDegradation::new("no sandbox implementation for this platform yet")` on macOS, default
  `false` in config). This design narrows the exposure; it does not bound it. A macOS sandbox
  (`sandbox-exec`/Seatbelt) is separate work.
- **MCP tools are not guarded.** They are third-party and `High` by default; their inputs are scanned by
  no path field this design knows. Their *results* are still redacted (`Basic`).
- **`Auto` mode still allows the `Secret` tier.** Consistent with `Auto` allowing every non-high
  operation — and `Credential` still denies there.
- **Redaction can be turned off**, and it corrupts reads of genuine test fixtures containing fake keys.
- **Plausible-deniability is not the goal.** The session DB stores the redacted text; the raw bytes may
  still exist in a file the agent wrote, in hook output, or in an MCP server's own logs.

## 7. Tests

| Test | Asserts |
|---|---|
| `registry_table` (unit, table-driven) | every row in §3.3 classifies with the documented tier and kind — one case per row |
| `exclusions_stay_excluded` | `id_rsa.pub`, `.env.example`, `fullchain.crt`, `.env.sample` produce no hit |
| `home_relative_only_at_home` | `~/.netrc` hits; `/tmp/netrc` does not |
| `classify_command_catches_plain_reads` | `cat ~/.ssh/id_ed25519`, `cat $HOME/.netrc`, `grep -r token ~/.aws`, `> .env`, quoted `'/Users/me/.netrc'` all hit |
| `readonly_shell_still_accepts_tilde` | `ls ~` / `cat ~/notes.txt` remain `Read` — the guard narrows the safelist, it does not remove tilde support |
| `sensitive_read_file_asks` (permission, end-to-end) | `.env` in-workspace → `Ask` in Default, `Deny` in Plan; an `allow` rule makes it stick |
| `credential_is_denied_before_everything` | `/Users/x/.ssh/id_ed25519` → `Deny` in `Auto` mode *and* with a settings `allow` rule *and* with an in-session always-allow |
| `credential_allow_entry_lifts_it` | the same call with a matching `sensitive_paths.allow` entry asks like `Secret` |
| `patch_touching_a_key_is_high` | `apply_patch` with a `+++ b/secrets.json` header |
| `apply_patch_always_allow_is_input_aware` | the rule generated for a patch call names the target path and does **not** match a patch to a different file (regression test for §1.4 — the bare-rule fallback) |
| `redact_basic_patterns` / `redact_credential_patterns` | one case per row in §4.1, plus marker text |
| `redact_does_not_touch_source` | `let token = compute(x);` and `api_key: String` survive `Basic`; a URL with `user:pass@` does not |
| `stream_redactor_holds_back_boundary` | a secret split across `push("sk-abc"), push("def…")` is never emitted whole |
| `session_store_holds_redacted_text` | a bash result containing a token, round-tripped through the store, contains the marker |

## 8. Docs to sync

| File | Change |
|---|---|
| `book/10_chapter_permission_zh.md` | New sections: the `Read` short-circuit caveat, the two tiers, `sensitive_paths` / `redaction` config, the `bash` tilde hole (and that the old comment was wrong); rows added to §11 Current Gaps |
| `book/26_chapter_issue_zh.md` | Newest-first entry: date, symptom (`~/.ssh` readable, token printed from `~/.claude/settings.json`), decision, observable behaviour, pointers |
| `ARCHITECTURE.md` §3 | The permission-system diagram gains the guard as a pre-step |
| `docs/tool_rendering.md` | Only if the refusal's rendering differs from a normal denial (it should not — `PreparedState::Resolved`) |
