# tact_eval

Repeatable evaluation of the Tact agent: a case suite, two runners, and graders
that read evidence instead of model self-report.

## Run a suite

```bash
cargo run -p tact_eval -- run suites/smoke \
  --provider openai --model gpt-test --base-url http://127.0.0.1:8080/v1
```

The report goes to stderr as a table and to stdout as JSON. `--json-out FILE`
moves the JSON to a file, `--attempts N` runs each case N times (the `pass@k`
denominator), and `--strict` exits non-zero when any case fails every attempt.

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

Graders are `command` (exit status 0 passes, output is kept on failure) and
`files` (`exists`, `absent`, `contains`, `matches`). A case with no graders is
reported as inconclusive rather than passed.

## Two runners

`--runner host` (the default) runs each case in process through the same
bootstrap the TUI uses. It is the only path that can report the typed stop
reason, the turn count, and cache or reasoning tokens, because `AgentUpdate`
does not carry them and `AgentLoopOutcome` never leaves the process.

`--runner cli` spawns `tact-ui headless`. It works against a release binary or a
container, and reports only what that process writes: final answer, tool lines,
token totals, and an inferred stop reason. Fields it cannot see stay `null` in
the report instead of being filled with zeroes.

Both runners write the same `CaseOutcome` and go through the same graders, so a
suite scores the same way either way. Because the host runner reads
process-global settings, a run's cases share one model and one permission mode;
a caller that needs distinct configurations starts one `tact-eval` process per
configuration.

## Isolation and cost

Every attempt runs in its own directory under `$TMPDIR/tact-eval/<case>/<uuid>`,
with a fixture copied in when the case declares `[isolation] kind = "copy"`. The
directory is deleted after grading: a run leaves a full source tree and a SQLite
session behind, and a suite of any size would fill the disk.

Session locking is per working directory, which is why attempts never share one.

## Comparing two runs

`ReportMeta` records the runner, model, endpoint, permission mode, attempt count,
and a fingerprint of the case set (id plus prompt). Two reports are comparable
only when those match; a changed prompt changes the fingerprint even under the
same case id.
