# Code Review — lease holder liveness, reconcile timer, stateDiagram fallback

Date: 2026-10-17. Scope: the **uncommitted working tree** on `wt/agent-goal` — the
lease holder-identity feature (`store/process_identity.rs` moved out of
`session_store/`, new `holder_pid`/`holder_identity` columns + migration,
`recover_dead_active_runs`, the 15 s `reconcile_dead_runs` timer) and the Mermaid
`stateDiagram` labelled-transition guard. Third round in this series; the earlier
two are `2026-10-15-*` and `2026-10-17-goal-budget-and-steering-review.md`.

Reviewed by reading the final source (not the diffs alone) and re-running the
claimed suites, clippy and fmt. Cross-user process behaviour was measured, not
assumed (see H1).

**Headline:** no CRITICAL. **One HIGH** — a lease written by a process on another
uid is read as *dead*, which lets a second runner breach the single-writer
invariant. One MEDIUM (migration TOCTOU) and two LOW. Everything else holds,
and every test/clippy/fmt claim in the Ch 26 entries reproduces exactly.

> **All findings are fixed in this tree; this document is the record of the round
> that found them.** H1 and M2 each carry a `Fixed` note with the change and its
> regression test. Re-verified after the fixes: `cargo test -p tact --lib` 1405
> passed, and clippy/fmt clean.

---

## HIGH

### H1. A lease held by a process on another uid is misread as dead → two live runners

This is a false-negative on a **verifiable** same-host holder, and the feature
under review is precisely what turned it into an automatic state change: the
15 s timer now converts "I cannot see the holder" into "interrupt its run, park
its Goal, take its lease".

The claim it violates is `store/process_identity.rs`'s own doc:

> Unverifiable holders conservatively report alive … calling it dead would let
> this process steal a lock that a live holder still owns.

That holds for `holder_pid = 0` / `holder_identity = ''` and for foreign-platform
prefixes — but **not** for a holder on a different uid, where a real, verifiable
identity is silently downgraded to dead. Both halves of `holder_is_alive` fail
this way:

```rust
pub fn holder_is_alive(pid: u32, identity: &str) -> bool {
    if pid == 0 || identity.is_empty() || !is_local_identity(identity) { return true; }
    is_process_alive(pid) && process_identity(pid).as_deref() == Some(identity)
}
```

- **`is_process_alive` — `kill -0` returns EPERM, read as "no such process".**
  Measured on this host (uid 501):

  ```
  kill -0 1   → "Operation not permitted"   exit 1   (pid 1 alive, uid 0)
  kill -0 $$  → exit 0                                (own process)
  kill -0 999999 → "No such process"        exit 1    (truly absent)
  ```

  Both live-but-foreign and truly-absent collapse to exit 1, and the code maps
  any non-success to `false` (§`is_process_alive`, `.map(|s| s.success())`).
  EPERM *proves the pid exists*; it is the one signal that must read alive.

- **`macos_starttime` — `proc_pidinfo` returns EPERM across uids.** Measured via
  the same `PROC_PIDTBSDINFO` call the code makes:

  ```
  pid 55172 (self, uid 501) → ret 136  identity 1792252757.506066
  pid 1     (launchd, root) → ret 0    errno EPERM
  pid 999999 (absent)       → ret 0    errno ESRCH
  ```

  The code returns `None` for `ret <= 0`, which is correct per se — but the two
  errnos are indistinguishable at that point, so `Some(identity) == Some(identity)`
  is `false` and the holder reads dead.

**Reachable path, both roles ordinary.** A `tact-ui` run as **root** (routine in
containers / devcontainers, where root owns the workspace) writes holder A with
`holder_pid` = root's pid. An ordinary-user `tact-ui` on the same `.tact/tact.db`
then:

1. `recover_dead_active_runs` (startup) or `reconcile_dead_runs` (≤15 s) ⇒
   `!holder_is_alive` ⇒ `interrupt_running` marks A's run `interrupted`, and the
   store returns a fresh `LeaseToken` at `current_epoch + 1`;
2. `reconcile_dead_runs` parks A's Goal `paused` with stop reason
   `lease holder is gone`;
3. `claim_lease` hands the epoch-incremented lease to B — **while A is still
   running its activation**. A's subsequent fenced writes fail (it holds the old
   epoch), so it is "only" a killed run plus a paused Goal, but two runners did
   briefly advance the same Goal, which is the invariant the lease exists for.

Worth noting the deploy asymmetry: a **root** process's `kill -0` on a
user process *succeeds*, so root sees user-uid holders correctly. Only the
reverse direction breaks — which is the direction where the more privileged
process holds the lease.

**Fix.** Distinguish "does not exist" from "exists but off-limits", and treat
"pid exists but identity is unreadable" as **unverifiable ⇒ alive**, matching the
guard already used for legacy and foreign rows:

- unix: switch to a real syscall and branch on the error — `libc::kill(pid, 0)`
  with `libc::ESRCH` ⇒ dead, `EPERM` ⇒ alive, `0` ⇒ alive (then compare
  identity). `libc = "0.2"` is **already a dependency of `tact`**, so this adds
  no crate.
- macOS identity: branch on `errno` — `ESRCH` ⇒ no process, `EPERM` ⇒ exists but
  unreadable ⇒ alive. Linux: `read_to_string("/proc/<pid>/stat")` fails
  `ENOENT` when gone and `EACCES` when just unreadable; the current `.ok()?`
  flattens exactly the distinction that matters.

Neither branch is testable in CI, because every test runs as one uid — which is
also why the existing suite is green. Measured evidence above is the substitute;
a regression test would need a second uid.

> **Correction (2026-10-22).** The Linux half of the bullet above is wrong:
> `/proc/<pid>/stat` is mode `444`, so an ordinary user *can* read a root
> process's start time — only `kill(1, 0)` answers `EPERM`. `EACCES` needs
> `/proc` mounted with `hidepid`, which is not the default. Measured as uid 1000
> in the host pid namespace: `kill -0 1` → EPERM, `head /proc/1/stat` →
> succeeds, `process_identity(1)` = `Some("linux:…")`. The unverifiable arm is
> therefore never entered on Linux, which is what made
> `a_process_we_may_not_describe_is_alive` fail on CI; see the 2026-10-22 entry
> in `book/26`.

> **Fixed.** `is_process_alive` now uses `libc::kill(pid, 0)` and branches on the
> error: only `ESRCH` is absence, `EPERM` is existence (and it rejects
> `pid > i32::MAX` first, because `u32::MAX as pid_t` is `-1`, i.e. "every
> process I may signal"). `holder_is_alive` reports a live-but-undescribable
> holder as unverifiable → alive, so it falls back to the TTL. Windows got the
> same `GetLastError` split.
>
> The regression test needs no second uid: `a_process_we_may_not_describe_is_alive`
> uses pid 1, which is root-owned on every supported unix. It fails against the
> old code. A third defect surfaced while fixing the first — the `u32::MAX`
> wrap — and is now pinned by `absent_process_with_local_identity_is_dead`.
>
> **Amended 2026-10-22:** the "unreadable identity ⇒ alive" half could *not* be
> asserted through pid 1, because pid 1's identity is readable on Linux. That
> half now lives in `an_unreadable_identity_for_a_live_pid_is_alive`, which
> pins the comparison itself (`alive_holder_matches`).
>
> Note the session lock keeps a *different* policy on purpose: `lock_epoch` is
> local, so an unrecognised epoch there is stale and must stay reclaimable
> (`test_session_lock_steals_stale_epoch` encodes that). Sharing
> `holder_is_alive` with it was tried and reverted. Both now share only the
> `is_process_alive` primitive, and the session lock still never steals from a
> live-but-unreadable holder.

---

## MEDIUM

### M2. `migrate_goal_leases_holder` is TOCTOU: a racing second process fails store init

`PRAGMA table_info` checks for the column, then `ALTER TABLE` runs unconditionally.
Two processes opening a pre-column database at the same time both see "absent" and
both `ALTER`; the loser gets a hard error. Measured:

```
sqlite> ALTER TABLE goal_leases ADD COLUMN holder_pid INTEGER NOT NULL DEFAULT 0;
ok
sqlite> ALTER TABLE goal_leases ADD COLUMN holder_pid INTEGER NOT NULL DEFAULT 0;
Error: in prepare, duplicate column name: holder_pid     # exit 1
```

`migrate_goal_leases_holder` propagates it (`.context("failed to add …")`), so the
second `SqliteGoalStore::new` fails: Goal commands and the panel degrade until
restart. Reachable in the normal way — a headless run and a TUI started together
against an existing database. (The `ALTER … NOT NULL DEFAULT 0` form itself is
valid on a legacy table; verified.) Fresh databases are unaffected, since the
columns are in `CREATE TABLE` and the PRAGMA then reports them.

Fix: treat a `duplicate column name` error as success, or hold `BEGIN IMMEDIATE`
around the check-and-alter. The existing idempotency test reopens **sequentially**,
so it cannot catch this.

> **Fixed** by the first option, in `add_lease_column`: a `duplicate column name`
> failure is absorbed (the column the caller wanted exists), any other error still
> propagates. Regression test
> `a_lost_migration_race_does_not_fail_store_construction` drives the losing half
> of the race directly, since the race itself is decided by timing, and asserts
> that a non-duplicate `ALTER` failure is still reported.

---

## LOW

- **L1. `holder_is_alive` reads the process twice, non-atomically.** A PID that
  exits and is reused between `is_process_alive` and `process_identity` can be
  reported alive (`true`). That is the fail-safe direction — the lease falls back
  to TTL expiry rather than being stolen — so it is a delay, not a breach.
  *Still open, by choice: closing it needs a single syscall returning both facts,
  and the failure mode is already the safe one.*
- **L2. The cross-uid assumption is undocumented.** *Fixed:* `holder_is_alive`
  and `process_identity` now state that a same-host holder on another uid is
  unverifiable rather than dead, and why the safe direction is "wait for the
  TTL".

## Deliberately not reported (checked and cleared)

- **Ch 26 heading numbering.** Every entry is `## 1.` — but `git show HEAD:` shows
  the same, so this predates the changeset and is a pre-existing template artifact,
  not a regression here. (The "one file per chapter" table in `AGENTS.md` is also
  unrelated to it.)
- **`run_budget == None ⇒ idle`** is accurate: the driver sets `run_budget` from
  `running_by_goal.get(&goal.id)`, so it is `Some` exactly while an active run row
  exists. `running (idle)` therefore never contradicts an in-flight activation.
- **`reconcile_dead_runs` narrower than `recover_interrupted`** is correct and
  well-reasoned: parking only the Goals whose run it just interrupted preserves
  the documented `no_progress` → idle `running` state against a background timer.
  Recovery passes two (`stemmed running` → `paused`) correctly stay on
  `recover_interrupted`.
- **Recovery callers.** `recover_interrupted` (startup) ⇄ `recover_dead_runs` +
  pass two, and the timer → `reconcile_dead_runs` ⇄ `recover_dead_runs`. No caller
  was left behind by the rename; both `GoalStore` impls and the test wrapper agree.
- **Migration preserves legacy rows** at `0` / `''`, which the guard reads as
  unverifiable, so pre-existing leases stay on the expiry path (test asserts the
  epoch is not bumped).
- **`stateDiagram` guard + `docs/state_machines.md`.** `render_mermaid_block`
  returning `None` falls through to the source-rendering path in
  `tui/src/widgets/state/app/agent.rs` (the `if … && let Some(diagram) = …` arm),
  so the fallback is what the Ch 26 entry describes. The doc now holds 0
  `stateDiagram-v2` and 6 text fences, so the six machines the guard would have
  regressed were converted in the same change — the trade-off is deliberate and
  explained in the doc, not accidental.
- **`process_identity` platform code.** Linux `starttime` is field 20 of
  `/proc/<pid>/stat`, i.e. index 19 after the split following `rfind(')')` —
  correct. On macOS the observed `proc_pidinfo` size is 136 and
  `pbi_start_tvsec`/`tvusec` sit at 96/104, matching the `repr(C)` layout.
- **Timer plumbing.** `MissedTickBehavior::Delay`, the `tick()` consumed after the
  startup pass, the `swap`-based once-per-outage failure notice, and spawning the
  pass off the command loop (so `Esc` / `/goal pause` stay responsive) are all
  right.

## Verification

Reproduced on this tree; every figure matches the Ch 26 claims. The table is the
**post-fix** state (the fixes added two tests to `tact`).

| Check | Result |
|---|---|
| `cargo test -p tact --lib` | 1405 passed, 1 ignored |
| `cargo test -p tact --lib store::goal_store::` | 31 passed |
| `cargo test -p tact --lib store::process_identity` | 5 passed |
| `cargo test -p tact-ui --lib` | 120 passed |
| `cargo test -p tact-ui --test driver_integration` | 10 passed |
| `cargo test -p agent_tui_kit --lib` | 427 passed |
| `cargo test -p tui --lib` | 747 passed |
| `cargo test -p tact_protocol --lib` | 27 passed |
| `cargo clippy -p tact -p tact-ui -p agent_tui_kit -p tact_protocol -p tui --all-targets -- -D warnings` | clean |
| `cargo fmt --all --check` | clean |

Cross-user measurements were taken on macOS 26 arm64 via `kill -0` and a direct
`proc_pidinfo(PROC_PIDTBSDINFO)` call with the same struct layout the source
declares. The Windows `OpenProcess` branch reports "denied" as dead for the same
reason H1 describes; it needs the same `ERROR_ACCESS_DENIED` vs
`ERROR_INVALID_PARAMETER` split.
