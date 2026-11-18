# tact_eval

Repeatable evaluation of the Tact agent: a case suite, two runners, and graders
that read evidence instead of model self-report.

## Run a suite

```bash
cargo run -p tact_eval -- run suites/smoke \
  --provider openai --model gpt-test --base-url http://127.0.0.1:8080/v1
```

Or point at a config file instead of flags:

```bash
cargo run -p tact_eval -- run suites/smoke --config path/to/tact.toml
```

The report goes to stderr as a table and to stdout as JSON. `--json-out FILE`
moves the JSON to a file, creating its parent directory when it is missing, and
`--attempts N` runs each case N times, which is the `pass@k` denominator. `N`
must be at least 1.

The table prints each case's median tokens per attempt and median duration. They
are reported, never gated: a suite that passes while spending twice the tokens
is a real regression, but turning that into a non-zero exit would need a
calibrated noise model this harness does not claim to have.

`run`'s exit code:

| status | meaning | exit |
|---|---|---|
| success | The run finished and something was judged | 0 |
| `--strict` failed | A case did not pass every attempt | 1 |
| no verdict | No attempt reached a pass or a fail — a missing binary, an expired key, every grader hung. The run measured nothing, so it is not a green suite | 4 |
| refused or broken | Bad flags, an unreadable suite, an unwritable `--json-out` | non-zero from the error |

Without `--strict`, a case that failed still exits 0: it was measured, and
failing is the measurement. That is why CI gates on `--strict` or `compare`, not
on the plain exit code of `run`.

`--strict` exits non-zero unless every case passed every attempt. A case with one
failed or inconclusive attempt disqualifies the run, so with `--attempts 3` a
single flake fails the gate. Use it for a small curated suite, not for one that
measures a noisy model.

`--binary PATH` (or `TACT_EVAL_BINARY`) names the `tact-ui` executable for the
CLI runner. A relative path is resolved when the runner is constructed, because
each attempt spawns it from the case's own directory.

Check that a suite is well formed without a model, a binary, or an API key.
This is the part CI can run on every pull request:

```bash
cargo run -p tact_eval -- validate suites/smoke
```

It prints the case count and `suite=<fingerprint>`. That is the same fingerprint
a run records in `ReportMeta` and prints as `suite=`, so a baseline and a
candidate can be checked against each other before either is compared.

## A case

```toml
prompt = "Create hello.txt containing exactly: hello eval"
timeout_secs = 180

[[graders]]
kind = "files"
name = "hello-file"

[[graders.assertions]]
kind = "contains"
path = "hello.txt"
text = "hello eval"
```

A case names a prompt and what decides it. It does not name a model or a
permission mode: those are process-global in Tact, so the run sets them once
(see [Two runners](#two-runners)).

Five graders:

- `command` runs a shell command in the case directory; exit status 0 passes.
  Its output is kept and quoted when it fails. A command that exceeds the
  case's `timeout_secs` makes the attempt inconclusive, not failed. It is POSIX
  `sh -c`, so a case that needs another shell must name it.
- `files` asserts on the case directory: `exists`, `absent`, `contains`,
  `matches`.
- `answer` asserts on the final assistant text: `contains`, `not_contains`,
  `matches`, `non_empty`.
- `json` reads one JSON file the run wrote (`path`) and asserts by a dotted path
  such as `items[1].id`: `present`, `absent`, `equals`, `contains`, `len`. A path
  that cannot parse fails at load, so it never reads as "the agent wrote the
  wrong shape".
- `tools` asserts on the calls the run made, by name: `tool` (called at least
  once), `no_tool`, `first_tool` (the run started with it), `at_most` (a call
  budget). It is how "use `read`, not `bash cat`" becomes a checkable case,
  which is something the other four cannot see.

A tool record the runner could not attribute to a tool (a bare `[tool failed]`
line in the headless log) matches no assertion. It makes `no_tool` and `at_most`
inconclusive rather than passing them, because an unattributed call could be the
one they forbid.

```toml
[[graders]]
kind = "tools"
name = "reads-before-writing"

[[graders.assertions]]
kind = "first_tool"
tool = "read"

[[graders.assertions]]
kind = "at_most"
tool = "edit_file"
max = 3
```

A case with no graders grades as inconclusive rather than passing. A run that
produced no answer (`EmptyTurn`) also cannot pass unless the case declares an
`answer` grader, because nothing then shows the request was answered.

`id` defaults to the file name. It must be a single path component: it names the
case's work directory, which the run deletes, so a `..` or a separator is
refused at load. Unknown keys are rejected, and a pattern that does not compile
fails at load, so a broken case never looks like a failed run.

## Running part of a suite

`--filter ID` (repeatable) runs only the named cases. A filtered run
fingerprints the subset, so comparing it against a full run is `incomparable`
rather than a regression. An id that matches nothing is an error, not an empty
run: a typo would otherwise look like a suite that passed everything at no cost.

```bash
cargo run -p tact_eval -- run suites/regression --filter fixture-editing
```

## Two runners

`--runner host` (the default) runs each case in process through the same
bootstrap the TUI uses. It is the only path that can report the typed stop
reason (`Completed`, `EmptyTurn`, `MaxTurns`, `MaxTokens`, `Cancelled`,
`BlockedByHook`, `GoalYield`) and cache-hit or reasoning tokens, because those
come from `AgentLoopOutcome` and `AgentUpdate::TokenUsage`, which never leave
the process. It requires the caller to install the process-global config first;
the binary does that for you.

`--runner cli` spawns `tact-ui headless`. It works against a release binary or a
container. It reads what that process writes: the final answer, tool lines,
token totals, the turn count, and a stop reason inferred from the log (an
empty-run warning, a turn counter at its cap, or a clean exit). Cache-hit and
reasoning tokens are not on its log line, so they are `null`, as is any tool
argument summary, which the log does not print.

Both runners write the same `CaseOutcome` and go through the same graders, so a
suite scores the same way either way. Because the host runner reads
process-global settings, a run's cases share one model and one permission mode;
a caller that needs distinct configurations starts one `tact-eval` process per
configuration.

## Isolation and cost

Every attempt runs in its own directory under `$TMPDIR/tact-eval/<case>/<uuid>`.
A relative `[isolation] kind = "copy"` fixture resolves against the suite
directory and is copied in; a fixture that does not exist fails at load.

The directory is deleted after grading: a run leaves a full source tree and a
SQLite session behind, and a suite of any size would fill the disk. That
deletion is why `session_id` in a report is a label, not a handle: the
`.tact/tact.db` it names is gone by the time anyone reads the JSON.

Attempts never share a working directory. Each one gets its own `.tact/tact.db`
and its own session id, so neither the session lock nor the database is shared.

## Hermetic runs

An attempt inherits the operator's `$HOME`, so `~/.tact/config.toml`,
`hooks.json`, user skills, the plugin cache, and the per-repository memory
directory all affect the run. `--config` pins the model configuration and
nothing else. A suite that must be hermetic should point `HOME` at a scratch
directory:

```bash
HOME=$(mktemp -d) cargo run -p tact_eval -- run suites/smoke --runner cli ...
```

## Comparing two runs

A comparison answers whether a candidate run got worse than an accepted
baseline. Both inputs are full `EvalReport` JSON documents, the same shape
`run` writes:

```bash
cargo run -p tact_eval -- run suites/smoke ... \
  --json-out target/eval/baseline.json
cargo run -p tact_eval -- run suites/smoke ... \
  --json-out target/eval/candidate.json

cargo run -p tact_eval -- compare \
  target/eval/baseline.json target/eval/candidate.json \
  --json-out target/eval/comparison.json
```

The baseline is a previously accepted report, not a separate compact schema
and not a file of thresholds. It already carries every case's attempt counts
plus the runner, identity, attempt count, and suite fingerprint, so the
comparison derives its deltas from the same evidence a reviewer can inspect.
Accepting a baseline is a human choice about which artifact CI points at.

The comparison prints a text summary to stderr and writes JSON to stdout, or
to `--json-out` instead. Its status and exit code are:

| status | meaning | exit |
|---|---|---|
| `pass` | Comparable, and no case has more failed attempts than baseline | 0 |
| `regression` | Comparable, and at least one case has more failed attempts | 1 |
| `inconclusive` | Comparable, no regression, but a candidate attempt was inconclusive | 3 |
| `incomparable` | The reports do not describe the same conditions, or baseline is inconclusive | 2 |

Precedence is `incomparable`, then `regression`, then `inconclusive`, then
`pass`. An improvement in one case does not cancel a regression in another.
Exit 2 also covers a report that cannot be read or parsed.

An `incomparable` result emits no per-case deltas, even when some cases could
be compared in isolation: a partial comparison would put a trustworthy number
on a run that was not actually comparable. Conditions checked are report
schema, suite fingerprint, attempts per case, runner, permission mode, model,
endpoint, and the ordered case set. `inconclusive` attempts are not failures;
they are reported separately and fail the gate with exit 3. A baseline that
itself contains an inconclusive attempt has an unknown verdict, so comparing
against it is `incomparable`, not a pass.

There are no numeric threshold flags. A case regresses when its failure count
rises, and a noisy candidate lands in `inconclusive` rather than being averaged
into a pass. A percentage tolerance needs a calibrated noise model, which the
first iteration does not claim to have. `--strict` is still a separate, single
run gate: it requires every case to pass every attempt and does not read a
baseline.

`ReportMeta` fingerprints the whole case set: every case's declared prompt,
timeout, isolation, and graders, plus the contents of every `Copy` fixture.
Fixture paths are hashed relative to the suite root and files by sorted
relative path, so the same suite fingerprints the same from any working
directory. Editing a fixture changes the fingerprint: the fixture is part of
what the case tests, so a comparison cannot silently mix two different inputs.

Cost is compared under the same rule: reported, never gated. The comparison
prices each case as a median (so one runaway attempt does not set it) and adds
the medians up per suite; a candidate that spends more while passing the same
reads as `pass` with a bigger number beside it. That is deliberate. A token
budget that reddens the build belongs here only after a suite has enough
recorded runs to say what its own noise is.

`ReportMeta.harness_version` records the build that produced the report. It is
recorded, not compared: a version bump is usually what a nightly gate is
measuring, so gating on it would disable the gate exactly when it matters. A
CLI run against a release binary this harness did not build records the
harness's own version, not that binary's.

`identity.model` and `identity.base_url` are `null` only when neither the flags
nor the resolved configuration name them. A CLI run driven by `--config` reads
that file the same way the child will and records the model and endpoint it
resolves, so two runs of one config are comparable. A config that cannot be read
is recorded as `null` and the run fails on its own: the report never invents an
identity.
