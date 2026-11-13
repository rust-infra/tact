# Agent evaluation: design

## Problem

There was no way to measure the Tact agent. A change to the system prompt, the
tool set, or the compaction policy could be argued about but not scored, and
CI could only ask whether the code compiled. The goal is a repeatable suite of
cases that produces comparable numbers, plus a shape an external evaluation
platform can drive without linking Rust.

Three facts in the existing system make the shape non-obvious.

`tact_protocol::AgentUpdate` is `Debug + Clone` only. It is not serializable,
and its `RunFinished { run_id }` drops the outcome. `tact::AgentLoopStop` is
deliberately serialization-free, because it was built for the Goal runner
inside the process. So the stop reason, the turn count, and the tool-call count
exist only while the process that ran the agent is alive.

`tact::config::install` installs process-global settings exactly once. Model,
endpoint, and permission mode therefore belong to the run, not to a case.

`tact-ui headless` cannot answer a permission prompt, so it can only run in
`auto` or `plan`. Its stderr log truncates every field to 240 characters.

## Usage

A maintainer writes cases as data, with no Rust per case.

```toml
# suites/smoke/write-a-file.toml
prompt = "Create a file named hello.txt containing exactly: hello eval"
timeout_secs = 180

[[graders]]
kind = "files"
name = "hello-file"

[[graders.assertions]]
kind = "contains"
path = "hello.txt"
text = "hello eval"
```

Then runs the suite and reads the report.

```bash
cargo run -p tact_eval -- run suites/smoke \
  --runner cli --binary ./target/debug/tact-ui \
  --provider openai --model gpt-test --base-url http://127.0.0.1:8080/v1 \
  --json-out target/eval/smoke.json
```

```
runner=cli model=gpt-test endpoint=http://127.0.0.1:8080/v1 mode=auto attempts=1
suite=e0250aa21eb6c8c9

answer-only                      pass 1/1 (100%), fail 0, unknown 0
write-a-file                     pass 1/1 (100%), fail 0, unknown 0

cases passing any attempt: 2/2
cases passing every attempt: 2/2
```

An external platform does the same thing through the binary only: `run`,
`--json-out`, and read `ReportMeta` plus the outcomes. No linking, no private
imports.

Library callers get `run_suite(&suite, &runner, attempts)` back as an
`EvalReport`, with `summaries()`, `cases_passing_any()` for pass@k and
`cases_passing_all()` for the regression gate.

## Shape

One crate, `crates/eval` (package `tact_eval`, binary `tact-eval`). It joins
workspace `members` but not `default-members`, and
`scripts/check-packaging.py` now fails the build if any other crate depends on
it. That fence is the whole boundary: the harness depends on the product, and
nothing in the product can reach back.

The data shape comes first.

```rust
struct EvalCase {                    // TOML, deny_unknown_fields
    id: String,                      // defaults to the file stem
    prompt: String,
    timeout_secs: u64,
    isolation: Isolation,            // Copy { fixture } | Empty
    graders: Vec<GraderSpec>,
}

enum GraderSpec {                    // closed set, serde snake_case
    Command { name, command },       // exit 0 passes
    Files { name, assertions },      // exists/absent/contains/matches
    Answer { name, assertions },     // contains/not_contains/matches/non_empty
}

enum Verdict { Pass, Fail { reason }, Inconclusive { reason } }
```

`Verdict` has three states, not two. A grader that cannot reach a decision, a
case with no graders, and a crashed run are all `Inconclusive`, which the report
counts separately. Conflating "we could not tell" with "it did not work" turns a
broken harness into a fake regression, and the opposite conflation turns an
untested case into a pass.

`RunArtifact` is what both runners produce.

```rust
struct RunArtifact {
    case_id: String,
    attempt: u32,
    session_id: Option<String>,
    answer: String,
    tools: Vec<ToolRecord>,
    tokens: TokenTotals,
    duration_ms: u64,
    stop: Option<StopView>,     // None when the runner cannot see it
    turns: Option<u32>,         // same
    failure: Option<RunFailure>,
}
```

The two `Option`s are the load-bearing decision. A runner layered on
`HostSession` would have had to guess the stop reason from the log, and a guess
in a field that averages into a suite's numbers is worse than an admission.
`None` prints as unknown; it is never a zero.

`Runner` has three methods: `name`, `identity`, and `run`. `identity` returns the
model, endpoint, and permission mode *the runner knows it used*, as `Option`s,
because the CLI runner can be told to read a config file that it never parses.
A report that filled those with a placeholder would compare equal to a run under
a different model.

`HostRunner` calls `bootstrap_session` directly rather than going through
`HostSession`. That is deliberate duplication: `HostSession` exposes
`AgentUpdate`, which loses the outcome, and the typed stop reason is the reason
this runner exists. `HostRunner` gets it from `agent_loop_detailed`.

`CliRunner` spawns `tact-ui headless` and reads the documented stderr contract
from `headless_log.rs`. Its parsers are pure functions over `&[String]` so they
are testable without a process, and every one of them was checked against a real
binary run.

The event pump in `HostRunner` stops on a `oneshot` signal, not on the channel
closing. `headless.rs` documents why: `background_run` hands a clone of the
observer sender to a detached task with no time limit, so waiting for the
channel to close waits for that command, and a run with a background job would
never report at all.

Isolation is one directory per attempt, under `$TMPDIR/tact-eval/<case>/<uuid>`,
deleted after grading. Not a git worktree: session locking is per working
directory, so distinct directories make parallel attempts safe by construction,
and a worktree would carry the repository's own `.tact/` state into a file
comparison.

## Alternatives considered

**Add an `eval` subcommand to `tact-ui`.** Rejected: it puts suite runners and
graders inside the shipped binary, and the eval crate would then be a product
dependency rather than the reverse. The packaging fence exists to keep that
direction.

**Make `AgentUpdate` serializable and use it as the evidence format.** Rejected:
`AgentUpdate` carries no outcome by design, so serializing it would still lose
the stop reason, and it would add a serde obligation to every protocol variant
for the sake of a tool outside the product.

**Use the SQLite session database as the only evidence, for both runners.**
Rejected as the sole source: `token_usages` writes zeroes for absent values, so
"unknown" and "zero" become indistinguishable, and a report built on it cannot
tell them apart. It stays available as a secondary source and is what the
`session_id` in each outcome points at.

**Run cases concurrently.** Rejected for now. Each concurrent case would need
its own process-global config, which the in-process runner cannot have.
`run_suite` iterates sequentially and the README says the parallelism belongs
one level up, in a caller that starts several `tact-eval` processes.

## Tradeoffs accepted

We accept that the CLI runner cannot report a stop reason or turn count in
exchange for working against a release binary or a container. The two fields are
`Option`, so this is visible in every report rather than silently zero.

We accept that `HostRunner` re-implements session open and teardown in exchange
for the typed outcome. The duplication is about forty lines and its boundary is
one function.

We accept that the stderr parsers mirror `headless_log.rs` formats by hand, in
exchange for not adding a machine-readable mode to the product binary. Each
parser has a test built from a real log line, and the parser that matters most
(`parse_stop`) treats anything it does not recognise as unknown.

We accept that a suite cannot vary model or permission mode per case, in
exchange for both runners reading the same run-level configuration.

## Open questions and risks

Should `headless` gain a `--events ndjson` flag that writes `AgentEvent` to a
file? That would let the CLI runner report the same typed fields as the host
runner, and remove the stderr parsers entirely. It is the one change to product
code this design would most benefit from, and it is deliberately out of scope
here.

Is `cases_passing_all` the right regression gate, or should `--strict` compare
against a recorded baseline instead of requiring 100%? The current flag fails on
any case that does not pass every attempt, which is correct for a small curated
suite and too strict for one that measures a noisy model.

The `Answer` grader is substring and regex only. An LLM judge needs calibration
against a suite before it earns a place, and calibrating it against itself is
circular. It belongs here after there is a suite with known-good verdicts.

## Next implementation step

Record a baseline report for a suite that exercises the tool set (not just
answer text), then wire `tact-eval run ... --strict` into a nightly workflow with
the report uploaded as an artifact.
