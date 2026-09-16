# BwrapSandbox for Linux Tool Execution

Status: proposed — revised 2026-09-15 after empirical verification against bubblewrap 0.12; sandbox is opt-in (`[tools] sandbox`, default off) and degrades to off on failure; all open decisions are resolved  
Date: 2026-09-15  
Scope: `crates/tact` + docs

## Empirical verification

Every claim marked **[measured]** below was reproduced on the development host
(bubblewrap 0.12.0, unprivileged user namespaces enabled) using the flag set this
document proposes. Three measurements changed the design:

| Measurement | Result |
| --- | --- |
| `--new-session`, a background job, then `killpg(bwrap_pid)` | bwrap forks a child that calls `setsid()`; the command ends up in its **own** process group (pgid = its own pid). `killpg` kills bwrap only, the grandchild survives while holding the stdout pipe, and **no EOF arrives** — `bash.rs` would block forever in its `closed_pipes < 2` loop. Without `--new-session` the whole tree shares bwrap's pgid and `killpg` clears it. |
| Design as written (no `/proc`, no `/dev`) | `cargo --version` → `Could not locate working directory.: no /proc/self/exe available. Is /proc mounted?`; `git status` → `fatal: could not open '/dev/null'`. The new root holds only `bin etc lib lib64 tmp usr workspace`. |
| `cargo test` in a Rust workspace (test 13) | Fails even with `/proc`/`/dev`: `/usr/bin/cargo` is a rustup shim and `~/.rustup` is not mounted → `rustup could not choose a version of cargo to run`. Passes only after read-only-binding `~/.rustup` + `~/.cargo`. `git commit` additionally needs `~/.config/git/config` (`Author identity unknown`). |

Three further measurements constrain the policy:

- `/lib64` is **required**, not optional: without it bwrap reports
  `bwrap: execvp sh: No such file or directory`, because the dynamic loader is
  missing — an error message that names the wrong thing.
- The sandbox's own loopback works (bwrap brings `lo` up), so commands that bind
  `127.0.0.1` run. *(Measured with `--unshare-net`, later reversed: the sandbox
  now shares the host network namespace — see §8 — so host-side services on
  `127.0.0.1` **are** reachable.)*
- Without `--unshare-pid`, the mounted `/proc` shows the host process table and
  the sandbox can signal same-uid host processes (~475 host pids visible;
  `kill -0 <host pid>` succeeds). With `--unshare-pid` the same mount shows 5
  pids and the sandboxed `sh` is pid 2.

## Problem

`tact` currently executes shell commands directly on the host through `sh -c`.

The existing bash tool already has several execution-safety mechanisms:

- shell command validation,
- permission checks,
- process-group management,
- timeout handling,
- cancellation,
- stdout/stderr streaming,
- `kill_on_drop`.

However, once a command is allowed to execute, the child process still inherits the host process's filesystem and network visibility.

For example, an allowed command can potentially access:

- the user's home directory,
- SSH credentials,
- cloud credentials,
- unrelated repositories,
- host runtime state,
- the host network.

Permission and sandboxing solve different problems:

```text
Permission
    ↓
Can this tool call execute?

Sandbox
    ↓
What can the executed process access?
```

### What the sandbox does and does not bound

The sandbox constrains *the processes a shell command spawns*. It is not a
boundary around the agent, and v1 must not be described as one:

- `read_file` / `write_file` / `edit_file` run in-process and keep full host
  filesystem access (bounded only by Permission).
- `background_run` (`crates/tact/src/background.rs:379`) and `worktree_run`
  (`crates/tact/src/worktree/mod.rs:203`) spawn shells of their own and stay
  unsandboxed (§15).
- Command hooks (`crates/tact/src/plugin/hooks.rs:271`) and stdio MCP servers
  (`crates/tact/src/mcp/mod.rs:839`) are unsandboxed host processes; both are
  user-configured, and neither is reachable by the agent as a tool call.

Therefore the value of v1 is: **code that the agent's approved command pulls in
and executes** — `cargo`/`npm`/`go` build scripts, `postinstall` hooks, test
binaries, `make` recipes — runs without seeing credentials, unrelated
repositories, or the network. The agent itself is already authorized by
Permission, and can trivially reach an unsandboxed path if it wants to
(`background_run` is a registered tool), so no part of this document should be
read as "the agent cannot escape". Closing that gap means sandboxing every
process-spawning path, which is the §15 follow-up, not this version.

v1 is also **opt-in and best-effort**: it is disabled by default and, when a
selected backend cannot start, degrades to unsandboxed with a warning rather than
failing the tool (§3.2). That is a product decision, recorded here so the rest of
the document does not overstate the protection a default install provides.

The first version should introduce a Linux OS-level sandbox without redesigning the existing permission system.

`tact` already has a mature bash execution path, so the safest initial implementation is to wrap the existing `sh -c` process with Linux `bubblewrap` (`bwrap`) rather than replacing the shell implementation.

## Goals

* Add a Linux `BwrapSandbox` implementation.
* Keep the existing `bash` tool behavior and process lifecycle unchanged.
* Be **opt-in**: disabled by default, enabled via `[tools] sandbox`, and degraded to unsandboxed — with a loud warning — when the selected backend cannot start (§3.2).
* Mount the current `ToolContext::work_dir` as the sandbox workspace.
* Allow the agent to read and write files inside the workspace.
* Make the basic host filesystem read-only where required for normal command execution.
* Disable network access **when enabled**.
* Prevent access to the host user's home directory outside the workspace **when enabled**.
* Preserve existing timeout, cancellation, process-group, stdout/stderr streaming, and non-zero exit handling. A timeout or cancel must still terminate the **entire** command tree and let the pipe readers see EOF — §9 names the flag that silently breaks this.
* Mount the container paths a normal command needs (`/proc`, `/dev`) and the dynamic loader (`/lib64`); without them `git`/`cargo` fail immediately (§4).
* Degrade to unsandboxed instead of hard-failing when `bwrap` is unavailable; §3.2 defines the config and the loud-degradation rule.
* Do not inherit the host environment into the sandbox beyond an explicit allowlist (§17).
* Keep the host workspace path out of the sandbox's namespace, and document the tool-visible path space that results (§19).
* Keep the first implementation small enough to evolve after real-world usage.

## Non-goals

* No changes to the existing Permission model.
* No new permission prompts.
* No macOS Seatbelt implementation.
* No Windows sandbox implementation.
* No configurable sandbox *policy* (mounts/network) in the first version — the only knob is the `[tools] sandbox` backend selector (§3.2).
* No network allowlist.
* No container/image management.
* No user-configurable mount system.
* No sandboxing of subagents as a separate policy.
* No changes to the existing shell command validator.
* No replacement of `sh -c` with a custom shell.
* No sandbox for `background_run` or `worktree_run` in the first implementation.
* No sandbox for command hooks or stdio MCP servers — both are user-configured host processes (§18).
* No attempt to provide complete container isolation.
* No guarantee that commands depending on the host home directory (`cargo` under `rustup`, `npm`, `go`) keep working; §17 fixes a small read-only toolchain allowlist instead of exposing `$HOME`.

## Design

### 1. Architecture

The first version inserts the sandbox between the existing tool execution logic and process creation:

```text
execute_tool_call()
        │
        ├── Hook
        ├── Permission
        │
        ▼
     bash tool
        │
        ├── validate_shell_command()
        │
        ▼
   BwrapSandbox
        │
        ▼
      bwrap
        │
        ▼
      sh -c
        │
        ▼
   user command
```

The existing execution lifecycle remains unchanged after `Command` creation:

```text
spawn
  ↓
stdout/stderr streaming
  ↓
timeout / cancellation
  ↓
process-group termination
  ↓
exit status
```

The sandbox only changes how the process is started and what filesystem/network namespace it receives.

### 2. Sandbox abstraction

Introduce a small abstraction under:

```text
crates/tact/src/sandbox/
├── mod.rs
└── bwrap.rs
```

The first interface should stay intentionally small.

```rust
pub trait Sandbox: Send + Sync {
    fn command(
        &self,
        program: &str,
        args: &[String],
        work_dir: &Path,
    ) -> anyhow::Result<Command>;   // tokio::process::Command

    /// Short backend name for startup diagnostics and logs ("bwrap", "none").
    fn describe(&self) -> &'static str;
}
```

`command` builds the *policy* half of the invocation; the backend keeps the
actual flag list to itself. That split is only testable if the flag list is
produced by a pure function (e.g. `fn bwrap_args(work_dir: &Path) -> Vec<String>`)
so the "only bind paths that exist" rule (§Risks 2) can be unit-tested without a
sandbox available.

The abstraction is intentionally command-oriented rather than process-oriented.

The sandbox does not:

- spawn the process,
- read stdout,
- read stderr,
- handle timeout,
- handle cancellation,
- decide whether the command is permitted.

Those responsibilities remain in the existing bash implementation.

The sandbox only constructs the isolated process invocation.

### 3. `BwrapSandbox`

Linux implementation:

```rust
pub struct BwrapSandbox;
```

Construction verifies that `bwrap` exists on the host by running the §3.1 probe.

If the probe fails, construction does **not** return an error that fails the
`bash` tool; it resolves to the `none` backend and records the degradation reason:

```text
BwrapSandbox::new()
        │
        ├── probe passes → BwrapSandbox
        │
        └── probe fails  → warn "sandbox: degrading to none (<reason>)" → None
```

The sandbox backend is selected by configuration (§3.2), so a user who never
opts in never runs `bwrap` at all, and a user who opts in but whose machine
cannot run `bwrap` gets the loud §3.2 degradation instead of a hard error.

The first version therefore follows a **fail-open** policy: a sandbox that cannot
start degrades to unsandboxed execution, and the degradation is announced rather
than silent. This reverses the first draft's "fail closed" rule and is the
explicit decision in §"Resolved questions".

### 3.1 Construction probe

Construction is more than a `PATH` lookup. A sandbox whose policy is
mis-specified fails at *every* command with a misleading message: `/proc` and
`/dev` missing surfaces as `no /proc/self/exe available` / `could not open
'/dev/null'`, `/lib64` missing as `execvp sh: No such file or directory`, and a
kernel that forbids unprivileged user namespaces as a bare `bwrap: …` line with
exit code 1 — indistinguishable from the user's command exiting 1 **[measured]**.

`BwrapSandbox::new()` therefore runs the full flag set once against a
throwaway command (`bwrap <policy flags> -- /bin/true`) and, on failure, records
a typed degradation reason that the §3.2 warning prints, e.g.:

```text
degrade: bwrap not found on PATH
degrade: bwrap present but unusable: <stderr> (see §Risks 1)
degrade: missing required mount: /lib64
```

### 3.2 Configuration and startup resolution

v1 adds exactly one knob:

```toml
[tools]
sandbox = false   # false (default) | true
```

> Changed 2026-09-16: the knob is a boolean, not a backend name. The backend is a
> platform decision made in the code (`sandbox::resolve`), so the user-facing
> config only says whether the sandbox is wanted. See the plan's "Result and
> deviations" table.

- `false` (default) — no sandbox; `bash` runs exactly as it does today (direct
  `sh -c`). A user who does not opt in sees zero behaviour change.
- `true` — the platform's backend with the §4 policy, Linux bubblewrap in v1. On a
  platform with no implementation the switch is inert and takes the degradation
  path below.

The backend is resolved **once at startup**, not per `bash` call. If the selected
backend cannot initialize — `bwrap` missing from PATH, the §3.1 probe failing,
the kernel refusing unprivileged user namespaces — the sandbox **degrades to
unsandboxed**:

```text
[tools] sandbox = true  →  no implementation / probe fails  →  warn  →  run unsandboxed
```

Degradation is announced, never silent: a startup log line, plus one visible
notice the first time a command would have been sandboxed, plus a `tracing::warn!`
carrying the §3.1 reason. This is **fail-open** — the user asked for the tool to
keep working (unsandboxed) instead of hard-failing when the backend is missing —
so the warning is what keeps the downgrade from being an unnoticed security
regression. The trade-off is stated in §"Resolved questions" and Risk 1.

Because the sandbox state is constant for the lifetime of a session, the §19 tool
description is computed once at startup from the resolved state: it must not
advertise "network disabled" or "host home unmounted" when the effective backend
is `"none"`.

### 4. Filesystem policy

The initial filesystem layout is:

```text
Sandbox
├── /workspace    host workspace, RW
├── /usr          RO
├── /bin          RO
├── /lib          RO
├── /lib64        RO — REQUIRED on glibc hosts (dynamic loader)
├── /etc          RO when present
├── /proc         fresh procfs — REQUIRED. Under `--unshare-pid` (adopted, §9.1)
│                 it shows only sandbox processes, not the host process table.
├── /dev          fresh minimal devtmpfs (/dev/null, /dev/zero, …)
├── /tmp          tmpfs
└── other host paths unavailable
```

Three corrections to the first draft, all **[measured]**:

- **`/proc` and `/dev` must be requested explicitly.** bubblewrap 0.12 creates
  neither by default; an unrequested path is simply absent from the new root
  (`ls /` shows only `bin etc lib lib64 tmp usr workspace`). Consequence:
  `cargo` aborts with `no /proc/self/exe available. Is /proc mounted?`, and
  `git status` aborts with `fatal: could not open '/dev/null'`.
- **`/lib64` is required, not "RO when present".** Without it every command fails
  with `bwrap: execvp sh: No such file or directory` (missing `ld.so`), which
  reads like a missing shell. Treat a missing `/lib64` as a construction failure
  (§3.1), not as a silently skipped mount.
- **Missing mounts are not cosmetic.** A skipped system mount produces a
  *working-looking* sandbox that cannot run the tools §6 promises, so the mount
  builder reports what it skipped and the probe in §3.1 runs a real command.

The workspace is mapped:

```text
host:

<context.work_dir>
       │
       │ bind RW
       ▼
sandbox:

/workspace
```

The sandbox working directory is:

```text
/workspace
```

Therefore:

```bash
pwd
```

returns:

```text
/workspace
```

regardless of the host-side absolute workspace path.

### 5. Workspace mount

The workspace is mounted read-write:

```text
--bind <host-work-dir> /workspace
```

This is the only host directory intentionally exposed read-write.

This allows normal development commands such as:

```bash
git status
git diff
cargo test
cargo build
go test ./...
npm test
```

to operate on the agent's current project.

The host path is not exposed under its original absolute location.

For example, if:

```text
host:
/home/rg/projects/tact
```

the sandbox sees:

```text
/workspace
```

rather than:

```text
/home/rg/projects/tact
```

### 5.1 Workspace boundary guard

`work_dir` comes from `ToolContext::work_dir`, which for the interactive path is
`std::env::current_dir()` (`crates/tact/src/consts.rs:157`). Nothing stops it
from being `$HOME` or `/`, and `--bind <work_dir> /workspace` would then hand the
sandbox the entire home directory (SSH keys, cloud credentials) read-write —
silently voiding the guarantee §Goals states, since the home directory *is* the
workspace.

Construction must reject or loudly flag:

```text
work_dir == "/"                 → refuse to start sandboxed bash
work_dir == $HOME               → refuse (the bind would expose the home dir RW)
work_dir is an ancestor of $HOME→ refuse
work_dir under /etc, /usr, /boot→ refuse
otherwise                       → bind read-write
```

A refusal is a startup diagnostic (same channel as §3.2), not a per-call error:
the user needs to know *why* `bash` will not run before the model tries it.

### 6. System filesystem

Normal development commands still need access to the host's installed runtime and libraries.

The first implementation therefore exposes selected system directories read-only:

```text
/usr
/bin
/lib
/lib64
/etc
```

where those paths exist.

No write access is granted to these directories.

This allows commands such as:

```text
sh
git
cargo
rustc
go
node
npm
```

to continue using the host installation while preventing the command from modifying those system files.

The implementation probes each path before binding it, but `/lib64` is a hard
requirement rather than an optional one: it carries the dynamic loader on glibc
hosts, and skipping it makes every command fail with a misleading `execvp`
error **[measured, §4]**. `/usr`, `/bin`, `/lib`, `/etc` are bound only when they
exist, and a skipped path is reported (§4).

### 7. Temporary filesystem

The sandbox receives an isolated `/tmp`:

```text
--tmpfs /tmp
```

This prevents commands from writing temporary data directly into the host's `/tmp`.

Temporary files created by the command are therefore sandbox-local and disappear with the sandbox process.

### 8. Network policy

**Revision 2026-09-16 — this section now documents the shipped behaviour, which
reverses the original decision. The original "network disabled" text is kept
below it, because the measurements in it are still what any future policy has to
reason about.**

The sandbox **shares the host network namespace**:

```text
--share-net
```

`--share-net` is the explicit spelling of bubblewrap's default; the flag is
written out so the policy reads as a decision rather than an omission.

Measured behaviour **[measured, 2026-09-16]**:

- Sharing the namespace alone is **not enough for name resolution**: the host's
  `/etc/resolv.conf` is normally a symlink into `/run/systemd/resolve`, and
  `/run` is not in the mount list, so the symlink dangles and every lookup fails
  with "Temporary failure in name resolution". The policy therefore also mounts
  `--ro-bind /run/systemd/resolve /run/systemd/resolve` (skipped quietly on
  hosts with a plain `resolv.conf`). Binding `/etc/resolv.conf` directly is not
  an option: `bwrap: Can't mount on symlink destination /etc/resolv.conf`.
- The host's connectivity is the sandbox's connectivity, including hosts that
  the host itself cannot reach directly. On this development box the direct
  route is partial — `example.com` answers, `api.binance.com` /
  `api.coinbase.com` / `1.1.1.1:443` all time out — while everything works
  through the host proxy on `127.0.0.1:7890`. Because the namespace is shared,
  that listener **is** reachable from inside.
- Consequently the proxy variables are forwarded verbatim (see §17): dropping
  them turns a working command into a connection error on a proxy-only host.

Tests must not depend on external network reachability; the regression test
connects to a listener the test itself started on the host's loopback, which is
reachable exactly when the namespace is shared and needed no external network.

Network access remains **not configurable**: the sandbox bounds the filesystem,
not connectivity, and there is no allowlist in this version.

<details>
<summary>Original decision (superseded 2026-09-16)</summary>

The first version disables network access:

```text
--unshare-net
```

This means commands such as:

```bash
curl https://example.com
wget https://example.com
git fetch
npm install
cargo fetch
```

should not have network connectivity from inside the sandbox.

Measured behaviour of `--unshare-net` **[measured]**:

- The sandbox gets a **fresh loopback that works** — bubblewrap brings `lo` up,
  so `bind("127.0.0.1", 0)` and connecting to it succeed. Commands (and test
  suites) that talk to their own local server keep working.
- Host-side listeners on `127.0.0.1` are unreachable: a dev server started
  outside the sandbox, a tool talking to a host-side MCP endpoint, etc.
- The host environment still leaks in, so the *error* the user sees may be about
  a proxy rather than about the network. With `http_proxy=127.0.0.1:7890`
  inherited, `curl` reports `Failed to connect to 127.0.0.1:7890 over proxy`
  instead of "network unreachable". §17 fixes this; tests must not depend on the
  proxy variables being present or absent (§Tests 7).

Network access is intentionally not configurable in this version.

</details>

A future design may introduce an explicit network policy:

```text
NetworkPolicy
├── Disabled
├── Allowlist
└── Full
```

but this is outside the current scope.

### 9. Process lifecycle

The sandbox must not replace the existing process lifecycle implementation.

The current bash implementation already handles:

- stdout pipe,
- stderr pipe,
- streaming output,
- timeout,
- cancellation,
- process groups,
- killing descendants,
- partial output,
- non-zero exit status.

The resulting process tree becomes:

```text
tact
  │
  └── bwrap
        │
        └── sh
              │
              └── command
```

`bwrap` should be started with:

```text
--die-with-parent
--unshare-pid
```

and **must not** be started with `--new-session`.

`--die-with-parent` ensures the sandbox process itself terminates when the parent
process disappears.

### 9.1 Why `--new-session` is forbidden here

The first draft claimed `--new-session` "keeps the sandbox process tree aligned
with the existing process-group lifecycle". Measured, the opposite is true
**[measured]**:

```text
tact (process_group(0) → child is its own group leader, pgid == bwrap pid)
  └── bwrap                         pid 8137, pgid 8137, sid <tact's session>
        └── forked session child    pid 8138, sid = pgid = 8138   ← setsid()'d
              └── sh -c "sleep 401 & wait"
                    └── sleep 401   pgid 8138                   ← detached group
```

`killpg(8137)` — exactly what `terminate_child`
(`crates/tact/src/tool/bash.rs:83-93`) does — kills bwrap only. The grandchildren
stay in group 8138, keep the inherited stdout/stderr write ends open, and the
pipe never reaches EOF. `bash`'s loop

```rust
while exit_status.is_none() || closed_pipes < 2 { … }   // bash.rs:221
```

then never exits: the timeout fires, `failure_reason` is set, and the call still
never returns. Without `--new-session` every process keeps pgid == bwrap's pid,
`killpg` clears the tree, and the pipes reach EOF within milliseconds.

This is the exact failure mode the existing code already guards against; the
existing comment at `child.wait()` (`bash.rs:262-266`) describes it. So:

- **No `--new-session`.** tact spawns with piped stdio and no controlling
  terminal, so the TIOCSTI concern the flag addresses does not apply.
- **`--unshare-pid` is adopted** (decision in §"Resolved questions"). It gives
  two things the plain flag set lacks: (a) when bwrap dies the pid namespace
  tears down and no descendant survives — measured, `killpg` then leaves zero
  processes, whereas a shared pid namespace can orphan a detached grandchild —
  and (b) the mounted `/proc` shows only sandbox processes, so the sandbox can
  neither read the host process table nor signal same-uid host processes
  (Risk 12). Cost: bwrap becomes pid 1 of the namespace and `/proc` shows only
  sandbox pids.
- The invariant to hold: **after a timeout, a cancel, or a drop, the host has no
  surviving descendants of the command, and both pipes have reached EOF.**
  §Tests 9/10/14 pin it.

The existing process-group configuration remains enabled.

### 10. Sandbox command construction

Conceptually, an invocation should look like:

```text
bwrap
    --die-with-parent
    --share-net                  # the host's namespace, §8
    --unshare-pid                # adopted, §9.1 / §Resolved questions

    --bind <work_dir> /workspace

    --ro-bind /usr /usr
    --ro-bind /bin /bin
    --ro-bind /lib /lib
    --ro-bind /lib64 /lib64      # required (§4)
    --ro-bind /etc /etc          # when present
    --ro-bind <toolchain homes>  # fixed allowlist, §17

    --proc /proc                 # required (§4)
    --dev /dev                   # required (§4)
    --tmpfs /tmp

    --chdir /workspace

    --
    sh -c "<command>"
```

Notes on the list itself:

- No `--new-session` — §9.1.
- `/proc`, `/dev`, `/lib64` are load-bearing, not optional; the mount builder may
  not "helpfully" drop them. Everything else in the RO list is existence-probed.
- The command is passed as a single argv (`--`, then `sh`, `-c`, the command
  string); no host shell ever sees it, so no quoting layer is added.
- The environment is decided explicitly here (§17), not inherited.

The exact argument construction belongs to `BwrapSandbox`.

The bash tool should not contain individual `bwrap` flags.

This keeps sandbox policy isolated from tool execution logic.

### 11. Bash integration

The current implementation:

```rust
let mut process = Command::new("sh");

process
    .arg("-c")
    .arg(command)
    .current_dir(&ctx.work_dir)
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .kill_on_drop(true);
```

will change to:

```rust
let mut process = match &ctx.sandbox {
    Some(sandbox) => sandbox.command(
        "sh",
        &["-c".to_string(), command.clone()],
        &ctx.work_dir,
    )?,
    None => {
        // Sandbox not enabled, or degraded to none (§3.2). Direct host execution.
        let mut c = Command::new("sh");
        c.arg("-c").arg(command).current_dir(&ctx.work_dir);
        c
    }
};

process
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .kill_on_drop(true);
```

The `None` arm is the documented unsandboxed path (default, or degraded — §3.2
already announced it at startup); it is byte-for-byte today's behaviour, including
`current_dir`. The `Some` arm has no `current_dir` — §12 explains why
`--chdir /workspace` replaces it.

The existing process-group configuration then remains:

```rust
configure_process_group(&mut process);
```

No changes are required to:

```text
validate_shell_command()
timeout
cancellation
stdout reader
stderr reader
ToolOutputBuffer
exit status handling
```

### 12. Working directory

The host-side `current_dir(&ctx.work_dir)` should no longer be used for the sandboxed process.

Instead:

```text
host:
    /some/absolute/project/path

sandbox:
    /workspace
```

and bwrap sets:

```text
--chdir /workspace
```

This prevents leaking the host workspace path into the sandbox process's working directory.

The cost is a **split path space**: shell commands see `/workspace/…`, while every
in-process tool (`read_file`, `edit_file`, `grep`, …) reports host absolute paths.
§19 covers how that is surfaced to the model.

### 13. Sandbox ownership

The first version does not add sandbox state to `Agent` or `AgentRuntime`.

`BwrapSandbox` is stateless:

```rust
pub struct BwrapSandbox;
```

A new sandbox command is constructed for each bash invocation. What is resolved
once at startup (§3.2) is the *backend handle* — `ctx.sandbox:
Option<BwrapSandbox>` (or, once multiple backends exist, an `Arc<dyn Sandbox>`)
carried on `ToolContext`, not on `Agent` or `AgentRuntime`. `BwrapSandbox` itself
stays a stateless unit struct; only that resolved handle is long-lived.

This matches the existing bash semantics, where every invocation starts a new:

```text
sh -c
```

process.

There is no persistent shell state.

Therefore:

```bash
cd src
```

does not affect the next bash invocation.

This behavior remains unchanged.

### 14. Platform behavior

`BwrapSandbox` is Linux-only.

The implementation should be conditionally compiled:

```rust
#[cfg(target_os = "linux")]
mod bwrap;
```

### 14.1 Non-Linux behaviour

Resolved by §3.2: enabling the switch on a platform with no implementation is
inert. `[tools] sandbox = true` on macOS/Windows is unavailable by definition, so
it takes the same degradation path as a failed probe — warn at startup and run
unsandboxed. There is no Seatbelt/Windows backend in v1, and no code path that
hard-fails the `bash` tool on a non-Linux host.

The earlier A/B/C options collapse because the default is now `"none"`: a
non-Linux host keeps working unsandboxed unless the user explicitly selects a
backend that does not exist there, in which case the §3.2 degradation warning
tells them so. Nothing about non-Linux needs a per-platform decision anymore.

A future implementation can add:

```text
Linux
  └── BwrapSandbox

macOS
  └── SeatbeltSandbox

Windows
  └── ...
```

under the same abstraction.

### 15. Tool coverage

Only the existing `bash` tool is sandboxed in this version.

Current state:

```text
bash             → BwrapSandbox  (only when [tools] sandbox = true; else direct sh -c)
background_run   → unchanged
worktree_run     → unchanged
```

This is an intentional first-step limitation.

`background_run` and `worktree_run` currently construct their own shell processes. They should later share a common process execution layer rather than duplicating sandbox setup.

A future refactor can introduce:

```text
ProcessExecutor
       │
       ├── Sandbox
       ├── timeout
       ├── cancellation
       ├── stdout/stderr
       └── process groups
```

and have all process-based tools use it.

### 16. Permission relationship

No Permission changes are introduced.

The execution order remains:

```text
Hook
  ↓
Permission
  ↓
Tool
  ↓
Sandbox
  ↓
Process
```

Permission answers:

```text
"May the agent execute this command?"
```

Sandbox answers:

```text
"What can this command access after execution is allowed?"
```

For example:

```text
Permission = allow
Sandbox = isolated
```

means the command can execute but cannot access arbitrary host resources.

The sandbox therefore does not replace or weaken the existing permission model.

### 17. Environment policy

bubblewrap inherits the parent environment unless told otherwise. Measured, the
first draft leaks `HOME=/home/rg` — a directory that is *not* mounted — along
with whatever credentials the invoking shell exported. Commands then fail with
messages that describe neither the sandbox nor the real cause.

The invocation therefore sets the environment explicitly (`--clearenv` plus
`--setenv`):

| Variable | Value | Why |
| --- | --- | --- |
| `PATH` | `/usr/local/bin:/usr/bin:/bin` | system tools |
| `HOME` | `/workspace` | keeps `~` expansion inside the sandbox; never the host home |
| `LANG` / `LC_ALL` | inherited | UTF-8 output behaviour |
| `TERM` | `dumb` or unset | there is no terminal in the sandbox |
| `RUSTUP_HOME` / `CARGO_HOME` / `GIT_CONFIG_GLOBAL` / `NPM_CONFIG_CACHE` | the host paths of the §17.1 homes | `HOME=/workspace` means `~` no longer resolves there, so these are set explicitly |

`http_proxy` / `https_proxy` / `all_proxy` / `no_proxy` — in both the lowercase
and uppercase spellings — are **forwarded verbatim** when set. This reverses the
original draft, which dropped them: with the host network namespace shared (§8)
the proxy is genuinely reachable from inside, and on a host where the proxy is
the only route out, dropping the variables turns a working command into a
connection error. Everything else still has to be named to get in. The bwrap
flags are `--clearenv` plus one `--setenv` per forwarded variable.

### 17.1 Toolchain homes

"Only the workspace is writable" and "the project's real commands keep working"
pull in opposite directions, and the first draft promised both. Measured: with
only the workspace mounted, `cargo` fails *before reaching the registry* —
`/usr/bin/cargo` is a rustup shim that needs `~/.rustup` — and the crate cache
lives in `~/.cargo`. `git commit` additionally needs `~/.config/git/config`
(`Author identity unknown`).

Resolution: a fixed, read-only allowlist of toolchain homes, mounted **at their
host absolute paths** and wired to the tools with explicit environment variables,
only when the directory exists (no configuration in v1):

```text
/home/<user>/.rustup      RO   rustup toolchains        → RUSTUP_HOME
/home/<user>/.cargo       RO   crate cache / registry   → CARGO_HOME
/home/<user>/.config/git  RO   commit identity          → GIT_CONFIG_GLOBAL
/home/<user>/.npm         RO   npm cache                → NPM_CONFIG_CACHE
```

Mounting them at the host path is a deliberate, narrow exception to §5's "host
path is not exposed": the alternative (bind under a synthetic path and point the
variables there) hides them at the cost of npm-style tools that resolve
`~`/`$HOME` internally. Because `HOME=/workspace`, `~` no longer resolves to the
host home, so the variables above are what make `cargo` / `git commit` / `npm`
work at all — verified end-to-end with `--clearenv`, `HOME=/workspace` and
`RUSTUP_HOME`/`CARGO_HOME`/`GIT_CONFIG_GLOBAL` pointing at the mounted homes
(`cargo metadata --no-deps` and `git status` both pass) **[measured]**.

Everything outside the list stays unreachable, and commands that depend on an
unlisted home (Docker config, `ssh-agent`, cloud CLIs) fail by design
(§Risks 4). The toolchain homes are read-only, so `cargo fetch` / `npm install`
still fail — that is §8, not an oversight.

### 18. Sandbox scope: every process-spawning path

§"What the sandbox does and does not bound" states the threat model. For
implementation, review, and the Ch 26 entry, here is the inventory of paths that
spawn processes today; v1 sandboxes exactly one of them:

| Path | v1 | Agent-callable |
| --- | --- | --- |
| `bash` — `crates/tact/src/tool/bash.rs:181` | **sandboxed** | yes |
| `background_run` — `crates/tact/src/background.rs:379` | unchanged | yes |
| `worktree_run` — `crates/tact/src/worktree/mod.rs:203` | unchanged | yes |
| worktree lane management (`git`) — `crates/tact/src/worktree/mod.rs:137,190,231,251,371,513` | unchanged | via tools |
| command hooks — `crates/tact/src/plugin/hooks.rs:271` | unchanged | no (user config) |
| stdio MCP servers — `crates/tact/src/mcp/mod.rs:839` | unchanged | no (user config) |
| `voice/transcriber.rs:389`, `hook/rtk_filter.rs:17,44` | unchanged | no |
| `read_file` / `write_file` / `edit_file` / `grep` | not processes | yes |

Two consequences the shipped docs must state, in this order of prominence:

1. v1 is **not** a boundary around the agent: `background_run` is one tool call
   away from an unsandboxed shell with network access.
2. v1 **is** a boundary around third-party code that an approved command runs
   (`cargo` build scripts, `npm` lifecycle scripts, test binaries, `make`).

### 19. Agent-visible path space

Inside the sandbox `pwd` is `/workspace`, while every in-process tool reports
host absolute paths (`/home/rg/Projects/tact/src/lib.rs`). A model that copies a
path out of `read_file` into `bash` gets "No such file or directory", and the
reverse (`/workspace/...` handed to `edit_file`) fails too. Three things are
needed, and the first is not optional:

1. **Say it in the tool surface.** `BASH_METADATA.description`
   (`crates/tact/src/tool/bash.rs:146`) and the `BashInput` field doc currently
   say only "Shell command to run in the current workspace." Extend them — once,
   in a backend-independent way — with: the workspace is mounted at `/workspace`
   and is the working directory, the host workspace path is not visible, the
   host network is shared, and the host home directory is not mounted. Describe
   *behaviour*, never the flag list. Because the sandbox is opt-in and can
   degrade (§3.2), the description is computed once at startup from the
   **resolved** state — a `"none"` session must not advertise `/workspace`
   *(revised 2026-09-16: the description no longer claims a disabled network,
   see §8)*.
2. **Keep failures actionable.** A command that references a host path outside
   the sandbox fails today with a bare shell error. Preprocessing the common case
   (an absolute path under `ctx.work_dir`) into a hint in the error output is
   worth it if it stays cheap; it is not a blocker.
3. **Record the alternative.** Binding the workspace at its host path *as well as*
   `/workspace` removes the split entirely, at the cost of putting the host path
   inside the sandbox — which the §Goals deliberately reject. v1 accepts the
   split; a future version may revisit it if models keep tripping over it.

## Tests

### 0. Unit tests (no `bwrap` required)

The first draft had no test that runs without a sandbox, so the "only bind paths
that exist" rule (§Risks 2) was unverified. Add, against the pure argument
builder (§2):

- every existing path in the fixed list appears exactly once, in the documented
  order;
- `/lib64` missing → the argument builder errors (never silently skips);
- a missing optional path (`/etc`, `~/.npm`) is skipped and reported;
- `--new-session` is absent from the argument list (this is the regression that
  would otherwise be re-introduced by a well-meaning edit);
- `--unshare-pid` and `--die-with-parent` are present (teardown + pid isolation, §9.1);
- the workspace guard (§5.1) rejects `/`, `$HOME`, and ancestors of `$HOME`.

### 1. Sandbox construction

Test:

```text
bwrap available and probe passes → BwrapSandbox::new() returns Some
bwrap missing                    → returns None with the §3.1 degradation reason
bwrap present, probe fails       → returns None, reason names the mount/flag
```

There is no "silent fallback to unsandboxed execution": the `None` path is the
documented degraded state and is always announced (§3.2). The unit tests assert
the *reason string* is populated, not that the tool errors.

**CI reality check:** `.github/workflows/rust.yml` runs on `ubuntu-latest` and
`scripts/check-rust.sh` runs the full test suite, so these tests either find
`bwrap` on the runner or must be gated so a missing sandbox skips instead of
failing. Gating must be explicit (an env var or a `bwrap`-presence check inside
the test), and the *degradation* path (no `bwrap` → `None` + reason) must still be
covered by tests that do not need `bwrap` (the §0 unit tests and the
construction-degradation test).

### 2. Working directory

Execute:

```bash
pwd
```

Expected:

```text
/workspace
```

### 3. Workspace read

Create a file on the host workspace:

```text
workspace/test.txt
```

Execute:

```bash
cat /workspace/test.txt
```

Expected: file contents are visible.

### 4. Workspace write

Execute:

```bash
echo hello > /workspace/test.txt
```

Expected:

```text
host workspace/test.txt
```

contains:

```text
hello
```

### 5. Host home isolation

Attempt to access a host path outside the workspace:

```bash
cat /home/<user>/...
```

Expected: inaccessible.

The test should use a temporary fixture rather than relying on a real credential file.

### 6. System filesystem

Execute:

```bash
cat /etc/passwd
```

Expected: succeeds when `/etc` is available.

This verifies that basic system libraries/configuration remain usable.

### 7. Network namespace is shared

The probe must not depend on external reachability, so it targets a listener the
test itself opened on the **host's** loopback:

```text
python3 -c 'import socket; socket.create_connection(("127.0.0.1", <test port>), 3)'
```

Expected: the connection **succeeds** **[measured]**: the sandbox is in the
host's network namespace, so the host's loopback (and a proxy listening on it) is
reachable. Under the superseded `--unshare-net` policy the same probe fails,
which is exactly what makes it the regression guard.

Two further assertions:

- name resolution is configured, not just routable: the tester may assert that
  `/etc/resolv.conf` resolves inside the sandbox (it is a symlink into
  `/run/systemd/resolve`, mounted per §8) — still with no external lookup, so the
  test stays hermetic;
- proxy variables are forwarded exactly as the host has them (present when the
  host sets them, absent otherwise), which is what §17 promises.

Skip gracefully when the probe interpreter is unavailable; never skip the
loopback assertion when it is.

### 8. Temporary filesystem

Execute:

```bash
echo hello > /tmp/tact-sandbox-test
```

Expected: succeeds inside the sandbox.

After the sandbox exits, the host `/tmp/tact-sandbox-test` must not exist.

### 9. Timeout

Run:

```bash
sleep 30
```

with a short tool timeout.

Expected:

```text
timeout
```

and the complete sandbox process tree is terminated.

This verifies that bwrap integration does not break the existing timeout mechanism.

### 10. Cancellation

Start a long-running command and cancel the tool.

Expected:

```text
bwrap
 └── sh
      └── command
```

is terminated as a group.

### 11. Non-zero exit

Execute:

```bash
exit 42
```

Expected:

```text
ToolResult = failure
exit code = 42
```

Existing error handling must remain unchanged.

### 12. Streaming output

Execute:

```bash
printf 'one\n'; sleep 1; printf 'two\n'
```

Expected that stdout continues to stream incrementally rather than being buffered until process completion.

### 13. Real development command

Run a representative project command from a Rust workspace:

```bash
cargo test
```

Expected, **with the §17.1 toolchain homes mounted read-only**:

- command can read the workspace,
- command can write build artifacts,
- command can execute compiler processes,
- command can use system libraries,
- command remains inside the sandbox filesystem boundary.

Without those mounts this test cannot pass **[measured]** — `/usr/bin/cargo` is a
rustup shim that aborts with `rustup could not choose a version of cargo to run`
before it ever reads the workspace. The first draft listed `cargo test` as an
expected-to-work command while specifying a mount policy under which it fails, so
this test doubles as the toolchain-home decision's acceptance test.

### 14. Timeout/cancel leaves no survivors and does not hang

This is the §9.1 regression and the most important behavioural test in the list.
Run a command whose grandchildren outlive their shell — the shape the existing
code already guards against:

```bash
bash -c 'sleep 300 & wait'
```

with a short tool timeout. Expected:

- the tool call **returns** (timeout error), and returns promptly;
- both pipes reached EOF (`closed_pipes == 2`) — i.e. no hang in the
  `while exit_status.is_none() || closed_pipes < 2` loop;
- `pgrep`/`/proc` shows no surviving `sleep`, no surviving `sh`, and no surviving
  `bwrap` after the call returns.

Repeat for user cancellation (cancel flag) and for drop (abort the task /
`kill_on_drop`) — all three paths call the same `terminate_child`.

### 15. `--die-with-parent`

Kill the parent (the test harness), leaving the sandbox running, and assert the
sandbox does not outlive it: no `bwrap` and no command process remains.

### 16. Toolchain-home mounts

- With the §17.1 homes mounted read-only (`~/.rustup`, `~/.cargo`,
  `~/.config/git`, `~/.npm`): `cargo metadata --no-deps` succeeds, and none of
  those homes is writable from inside.
- A command that needs an unmounted home (`docker ps` with a `~/.docker` config,
  `git commit` without `~/.config/git/config`) fails — asserting the boundary is
  a deliberate allowlist rather than a hole.

### 17. PID namespace isolation (`--unshare-pid`)

From inside the sandbox: `/proc` contains only the sandbox's own pids (measured:
~5 pids, the sandboxed `sh` is pid 2), a host process pid is not visible in
`ps`, and `kill -0 <host pid>` fails. This pins the Risk 12 mitigation: the
sandbox can neither read the host process table nor signal same-uid host
processes.

## Docs to sync

The first draft's table named no actual file. Concrete targets (bilingual pairs
must be updated in the same commit):

| Trigger | File |
| --- | --- |
| This design | `docs/superpowers/specs/2026-09-15-bwrap-sandbox-design.md` (this file) |
| Implementation plan | `docs/superpowers/plans/2026-09-15-bwrap-sandbox.md` |
| bash execution lifecycle, sandbox insertion point, §19 path space | `book/07_chapter_tool.md` + `book/07_chapter_tool_zh.md` (§7 "Workspace Path Safety", §11 "Current Gaps") |
| Permission vs Sandbox as separate layers | `book/10_chapter_permission.md` + `book/10_chapter_permission_zh.md` |
| `background_run` remains unsandboxed | `book/13_chapter_background.md` + `_zh.md` |
| `worktree_run` / lane git calls remain unsandboxed | `book/15_chapter_worktree.md` + `_zh.md` (the chapter already states a worktree is not an OS sandbox) |
| `[tools] sandbox` config key | `config.example.toml` + `book/21_chapter_config.md` + `_zh.md` |
| Shipped user-visible change | `book/26_chapter_issue.md` + `book/26_chapter_issue_zh.md`, newest-first entry (date, type, symptom, decision, observable behaviour, pointers) |
| Agent-facing conventions for the split path space (§19) | `docs/agent_guidelines.md` (bash/tool-usage section) |

If an existing security or tool-execution document already describes Permission as the execution boundary, update it to explicitly distinguish:

```text
Permission = authorization
Sandbox = execution boundary
```

## Risks

### 1. `bwrap` availability (degradation, not failure)

Not every Linux environment has bubblewrap installed, and some kernels restrict
unprivileged user namespaces (the Ubuntu 24.04 AppArmor profile is the common
case). Both would surface as a bare exit code 1 at command time — which is why
the probe runs at startup (§3.1).

Behaviour, per the §3.2 decision: the sandbox **degrades to `"none"`** rather
than failing the tool, and the degradation is announced. This is fail-open — if
the config says `"bwrap"` but `bwrap` cannot start, commands run unsandboxed. The
mitigation is that the downgrade is loud and one-time, not silent, and the
*effective* sandbox state is reported at startup, so a user who asked for
`"bwrap"` can see "got `none` (<reason>)". The default is `"none"` anyway, so a
default install is never affected.

Mitigation:

- detect `bwrap` **and probe the full flag set** at startup (§3.1),
- degrade to `"none"` with the typed §3.1 reason and the §3.2 warnings,
- never let the degraded state be quiet: startup log + TUI notice + `tracing::warn!`.

### 2. Distribution differences

Linux distributions differ in:

```text
/lib
/lib64
/usr
/bin
```

and some systems use merged `/usr` layouts.

Mitigation:

- only bind paths that exist — **except `/lib64`**, which carries the dynamic
  loader and must fail construction when absent **[measured]**, rather than
  producing `bwrap: execvp sh: No such file or directory`,
- treat `/proc` and `/dev` as required mounts, not optional ones **[measured]**,
- keep the first mount policy conservative,
- unit-test the argument builder so "skipped because absent" is visible (§Tests 0).

Future versions may derive system mounts dynamically.

### 3. Development commands requiring network

Commands such as:

```text
cargo fetch
npm install
go get
git fetch
```

will fail because networking is disabled.

This is intentional for the first version.

Network access should be added later as an explicit policy rather than silently enabled.

### 4. Commands requiring additional host paths

Some commands may require resources not currently exposed by the sandbox.

Examples may include:

```text
Docker
SSH
system services
desktop applications
hardware devices
```

These are outside the first version's supported execution model.

The solution is to add narrowly scoped capabilities later rather than exposing the host wholesale.

### 5. `background_run` remains unsandboxed

After the first implementation:

```text
bash             sandboxed
background_run   unsandboxed
worktree_run     unsandboxed
```

This is not a small inconsistency: both remaining paths are **agent-callable
tools**, so the sandbox does not bound the agent at all — it bounds the
*third-party code an approved command runs*. §18 lists every process-spawning
path; §"What the sandbox does and does not bound" is the wording the shipped docs
and the Ch 26 entry must reuse.

Mitigation:

- explicitly document this first-version limitation, in those terms,
- do not claim that all process execution is sandboxed, and do not let the
  `bash` tool description imply that the sandbox contains the agent (§19),
- follow up by introducing a shared `ProcessExecutor` (§15) so the other paths
  can share one policy instead of duplicating setup.

### 6. Process-group interaction

Bwrap introduces an additional process layer:

```text
tact → bwrap → sh → command
```

Existing process-group termination must continue to kill the complete descendant
tree. The first draft's mitigation list included `--new-session`; measured, that
flag **breaks** the guarantee (a detached session survives `killpg` and holds the
pipes open, so the tool call never returns — §9.1).

Mitigation:

- do **not** pass `--new-session`,
- retain `--die-with-parent` and the existing process-group configuration,
- `--unshare-pid` is adopted (§9.1); it also isolates `/proc` (Risk 12),
- cover it with §Tests 14 (timeout/cancel/drop: no survivors, pipes reach EOF)
  and §Tests 15 (`--die-with-parent`),
- keep a §Tests 0 unit assertion that `--new-session` is absent from the built
  argument list.

### 7. Sandbox escape assumptions

Bwrap is an OS-level isolation mechanism, but the first version is not intended to provide a hardened container boundary against a malicious kernel-aware workload.

The design assumes:

- trusted Linux kernel,
- supported bubblewrap installation,
- normal user-level Agent workloads.

A stronger threat model may require additional hardening in the future.

### 8. Toolchain homes vs "only the workspace is writable"

The two promises pull against each other, and the first draft asserted both. A
read-only allowlist of toolchain homes (§17.1) is what makes `cargo test` a
supported command at all; anything not on the list fails by design.

Mitigation:

- keep the list fixed and constant in v1 (no configuration),
- document which commands are expected to fail (`cargo fetch`, `npm install`,
  `docker`, cloud CLIs) rather than letting users discover it,
- assert the boundary in §Tests 16 (read-only home is not writable from inside).

### 9. Split path space confuses the model

`/workspace` inside, host absolute paths in every tool result (§19). Measured
consequence: `cat /<host path>` fails even though the file is in the workspace.

Mitigation:

- state the sandbox semantics in the `bash` tool description/schema (§19.1),
- optionally hint when a command references an absolute path under `ctx.work_dir`,
- revisit if real usage shows the model repeatedly tripping.

### 10. Host environment leakage

bubblewrap inherits the environment by default. Measured leaks:
`HOME=/home/rg`, a directory that is not mounted. The same channel can hand
whatever tact itself holds — provider API keys, tokens — to build scripts and
`postinstall` hooks, which is exactly the class of code this sandbox exists to
contain.

Mitigation: `--clearenv` + the explicit allowlist in §17 (the proxy variables
are the one deliberate exception, and they carry no credentials of Tact's own).
Assert the forwarded set in §Tests 7.

### 11. The workspace guard can silently void the isolation

If `work_dir` is `$HOME` or `/`, the workspace bind exposes the entire home
directory read-write and §Goals' home-isolation property disappears without any
error (§5.1). Mitigation: refuse the known-bad roots at construction and report
it with the same startup diagnostic as §3.2.

### 12. `/proc` leaks the host process table without `--unshare-pid`

The mount list needs `/proc` for `cargo`/`git` (§4), but a `/proc` mounted in a
shared pid namespace shows the host's processes and lets the sandboxed command
signal same-uid host processes **[measured]** — `/proc` had ~475 host pids and
`kill -0 <host pid>` succeeded. That is a leak of host runtime state and a same-
user kill surface, neither of which is in §Goals.

Mitigation (adopted): `--unshare-pid` is always passed (§9.1), so `/proc` shows
only the sandbox's own pids and the same-user kill surface is gone. §Tests 17
pins it; a future change that drops the flag must fail that test.

## Resolved questions

* **Should Permission be changed?** No. Permission and Sandbox remain separate layers.

* **Should the first version support macOS?** No backend. On non-Linux hosts `"bwrap"` is unavailable and degrades to `"none"` with a warning (§14.1); `"none"` is the only backend that exists everywhere.

* **Should network be enabled?** No — when the sandbox is enabled. The sandbox keeps its own working loopback; host-side `127.0.0.1` services are unreachable **[measured]**.

* **Should the workspace be writable?** Yes. The current Agent workspace is the only host directory mounted read-write.

* **Should the whole home directory be mounted?** No. Only the workspace is exposed, plus a fixed read-only toolchain allowlist (`~/.rustup`, `~/.cargo`, `~/.config/git`, `~/.npm`) that `cargo`/`git commit`/`npm` require to work at all (§17.1) **[measured]**. Configuring that list is out of scope for v1.

* **Should `/usr`, `/bin`, `/lib`, `/etc` be writable?** No. They are read-only system mounts. `/proc` and `/dev` are additionally **required** mounts, and `/lib64` is required on glibc hosts (§4).

* **Should `--new-session` be used?** No. Measured to detach the command into its own process group, which breaks the existing `killpg` contract and can hang the tool call indefinitely (§9.1). `--die-with-parent` and `--unshare-pid` are both passed; the latter isolates `/proc` (Risk 12).

* **Should `--unshare-pid` be used?** Yes. Teardown is stronger (namespace death reaps members) and `/proc` no longer leaks the host process table (§9.1, Risk 12).

* **Should the host environment be inherited?** No. `--clearenv` plus an explicit allowlist; the proxy variables are the single exception and are forwarded verbatim (§17, revised 2026-09-16 because the namespace is shared again).

* **Should network access be disabled?** It was, and it is not any more: the sandbox shares the host network namespace and mounts `/run/systemd/resolve` for name resolution (§8, revised 2026-09-16). The sandbox bounds the filesystem, not connectivity.

* **Should `/tmp` use the host filesystem?** No. Use sandbox-local tmpfs.

* **Should the sandbox persist between bash calls?** No. Each bash call creates a new sandbox process, preserving current stateless shell semantics.

* **Should `background_run` be migrated now?** No. First ship the bash integration; unify process execution in a follow-up (§15, §18).

* **Should the sandbox decide whether a command is allowed?** No. Existing command validation and Permission remain responsible for authorization.

* **What happens if `bwrap` is unavailable?** The sandbox degrades to unsandboxed — and the degradation is announced at startup and on the first affected command (§3.1, §3.2). Fail-open, not fail-closed: this is the explicit product decision, kept loud rather than silent.

* **Should sandbox policy be configurable in v1?** One knob: a `[tools] sandbox` boolean, default `false`. No per-mount, per-network, or per-tool policy; that stays out of v1.

* **Should the sandbox be enabled by default?** No. Default `false`; the user opts in. A default install behaves exactly as today.

* **What is the intended long-term architecture?**

```text
Tool
 │
 ▼
Permission
 │
 ▼
ProcessExecutor
 │
 ▼
Sandbox
 ├── Linux   → BwrapSandbox
 ├── macOS   → SeatbeltSandbox
 └── future  → other backend
```

The first implementation intentionally stops at:

```text
bash
  ↓
BwrapSandbox
  ↓
bwrap
  ↓
sh -c
```

rather than introducing the complete cross-platform execution abstraction immediately.

## Decisions this revision added

`--unshare-pid` (adopted — §9.1, Risk 12, Tests 17) and `~/.npm` in the
toolchain allowlist (§17.1) are folded into §"Resolved questions" above; there
are no open decisions left.


