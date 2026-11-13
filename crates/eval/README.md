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
moves the JSON to a file, and `--attempts N` runs each case N times, which is
the `pass@k` denominator.

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

Three graders:

- `command` runs a shell command in the case directory; exit status 0 passes.
  Its output is kept and quoted when it fails. A command that exceeds the
  case's `timeout_secs` makes the attempt inconclusive, not failed.
- `files` asserts on the case directory: `exists`, `absent`, `contains`,
  `matches`.
- `answer` asserts on the final assistant text: `contains`, `not_contains`,
  `matches`, `non_empty`.

A case with no graders grades as inconclusive rather than passing. A run that
produced no answer (`EmptyTurn`) also cannot pass unless the case declares an
`answer` grader, because nothing then shows the request was answered.

`id` defaults to the file name. Unknown keys are rejected, and a pattern that
does not compile fails at load, so a broken case never looks like a failed run.

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

`ReportMeta` records the runner, its identity (model, endpoint, permission
mode), the attempt count,
and a fingerprint over the whole case set: every case's prompt, timeout,
isolation, and graders. A changed grader or timeout changes the fingerprint, so
a comparison cannot silently mix two different suites.

`identity.model` and `identity.base_url` are `null` when the runner could not
observe them, which is the case for a CLI run driven entirely by a `--config`
file. Two runs whose fields are both `null` are not thereby comparable; check
the config files yourself.
