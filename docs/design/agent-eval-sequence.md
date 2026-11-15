# Agent evaluation: sequence diagrams

Companion to [`agent-eval.md`](agent-eval.md). The four diagrams below trace one
`tact-eval run` from suite loading to the report, then split the two runners and
the grading fold into their own views. They follow the current source, so code
references are included for the parts most likely to move.

## 1. Suite orchestration (`run_suite`)

- `crates/eval/src/main.rs:109-139`: load, configure, run, print.
- `crates/eval/src/lib.rs:47-89`: sequential cases, grade after each attempt,
  delete the work directory.
- `crates/eval/src/suite.rs:28-62`: loading, fixture resolution, ordering.

```mermaid
sequenceDiagram
    autonumber
    actor U as User/CI
    participant M as main.rs
    participant S as EvalSuite
    participant R as run_suite
    participant RN as Runner (host/cli)
    participant G as grader::grade
    participant RP as EvalReport

    U->>M: tact-eval run suites/smoke --runner ... --attempts N
    M->>S: EvalSuite::load(dir)
    S->>S: read *.toml
    S->>S: parse + validate_shape
    S->>S: resolve relative fixture and check it exists
    S->>S: sort by id, reject duplicate ids
    S-->>M: EvalSuite { cases, declared }
    alt runner = host
        M->>M: tact::config::init_from_args(options)
        M->>RN: HostRunner::new(options)
    else runner = cli
        M->>RN: CliRunner::new(binary).with_options(options)
    end
    M->>R: run_suite(&suite, &runner, attempts)
    loop every case
        loop every attempt
            R->>RN: run(case, attempt)
            Note over RN: see diagrams 2 and 3
            RN-->>R: RunOutcome { artifact, work_dir }
            R->>G: grade(case, work_dir, artifact)
            G-->>R: Vec<Grade>
            R->>R: CaseOutcome::from_parts(artifact, grades, judged_on_answer)
            R->>R: remove_dir_all(work_dir)
        end
    end
    R->>RP: EvalReport::new(meta, outcomes)
    RP-->>M: EvalReport
    M->>U: to_text() → stderr
    M->>U: to_json() → stdout or --json-out
    opt --strict
        M->>M: cases_passing_all() < case count → exit(1)
    end
```

## 2. HostRunner: one attempt in process

- `crates/eval/src/host_runner.rs:69-243`: bootstrap, loop, teardown.
- `crates/eval/src/host_runner.rs:252-277`: the event pump stops on a signal,
  not on the channel closing, because a detached background task can hold a
  sender forever.
- The typed stop reason and turn count come from `agent_loop_detailed`; they
  never cross a process boundary.

```mermaid
sequenceDiagram
    autonumber
    participant R as run_suite
    participant H as HostRunner
    participant T as tact_host / Agent
    participant P as drain_events

    R->>H: run(case, attempt)
    H->>H: prepare_work_dir(case)
    opt work dir preparation fails
        H-->>R: RunOutcome(Harness failure)
    end
    H->>T: try_settings()
    opt config is not installed
        H-->>R: RunOutcome(Startup failure)
    end
    H->>T: open_sqlite_session_store(tact_path)
    H->>T: open_session(args, tact_path, store, lock_registry)
    T-->>H: (session_id, lock)
    H->>T: shared_skill_registry(work_dir)
    H->>T: bootstrap_session(Frontend::Observing(tx), Notices::Stderr)
    T-->>H: agent
    opt ensure_session fails
        H->>T: shutdown_mcp() + lock.release()
        H-->>R: RunOutcome(Startup failure)
    end
    H->>T: build_user_message(prompt, work_dir)
    H->>P: spawn drain_events(rx, shutdown_rx)
    H->>T: agent_loop_detailed(Some(prompt))
    Note over H,T: select! races the loop result against timeout_secs
    alt loop returns in time
        T-->>H: AgentLoopOutcome or Err
    else timeout_secs expires
        H->>T: cancel_handle().cancel()
        H->>T: wait up to 30s for cooperative shutdown
    end
    H->>T: detach_event_observers()
    H->>P: shutdown_tx.send(())
    P-->>H: (tools, tokens)
    H->>T: last_assistant_text(agent.context())
    Note over H: stop/turns from the outcome<br/>budget expired → failure=Timeout<br/>loop Err → failure=Harness<br/>no return → no outcome
    H->>T: subagent_manager.cancel_all_and_persist()
    H->>T: dispatch_session_end_hooks()
    H->>T: shutdown_mcp()
    H->>T: lock.release()
    H-->>R: RunOutcome { artifact, work_dir }
```

## 3. CliRunner: one attempt as a child process

- `crates/eval/src/cli_runner.rs:84-160`: spawn, parse, infer the stop reason.
- `crates/eval/src/process.rs:58-133`: process group, timeout kill, bounded
  pipe drain.
- The stderr contract is mirrored by hand from `headless_log.rs`; anything the
  parsers cannot attribute stays unattributed rather than guessed.

```mermaid
sequenceDiagram
    autonumber
    participant R as run_suite
    participant C as CliRunner
    participant X as process::run_child
    participant UI as tact-ui headless
    participant RT as pipe readers

    R->>C: run(case, attempt)
    C->>C: prepare_work_dir(case)
    opt work dir preparation fails
        C-->>R: RunOutcome(Harness failure)
    end
    C->>X: run_child(flags + --notifications=false headless prompt, timeout)
    X->>UI: spawn(cwd=work_dir, stdin=null, stdout/stderr=piped)
    Note over X,UI: process_group(0) + kill_on_drop(true)
    X->>RT: spawn stdout/stderr readers
    loop line by line
        RT->>RT: append to the shared buffer
    end
    alt child exits in time
        UI-->>X: ExitStatus
    else timeout
        X->>UI: kill(-pgid, SIGKILL) + start_kill + wait
    end
    X->>RT: bounded join (5s grace)
    RT-->>X: output received so far
    X-->>C: ChildOutcome { stdout, stderr, end }
    C->>C: parse_session_id / parse_tools / parse_tokens / parse_turns
    C->>C: answer = stdout.trim_end()
    alt end = TimedOut
        C-->>R: RunOutcome { Timeout failure }
    else exited
        C->>C: parse_stop(lines, end)
        Note over C: empty-answer warning → EmptyTurn<br/>hook blocked the turn → BlockedByHook<br/>turn counter at its cap → MaxTurns<br/>clean exit → Completed
        opt non-zero exit
            C->>C: failure = Harness
        end
        C-->>R: RunOutcome { artifact, work_dir }
    end
```

## 4. Grading fold and summary

- `crates/eval/src/grader.rs:41-108`: one grade per declared grader.
- `crates/eval/src/report.rs:85-113`: the single place that decides an attempt.
- `crates/eval/src/report.rs:157-195`: per-case rollup, pass@k, and the strict
  gate.

```mermaid
sequenceDiagram
    autonumber
    participant R as run_suite
    participant G as grader::grade
    participant P as process::run_child
    participant W as work_dir
    participant O as CaseOutcome

    R->>G: grade(case, work_dir, artifact)
    alt no graders declared
        G-->>R: [Inconclusive: the case declares no graders]
    end
    loop every GraderSpec
        alt kind = command
            G->>P: run_child(sh -c command, case.timeout_secs)
            P-->>G: ChildOutcome
            alt timed out
                G->>G: Inconclusive
            else exit code = 0
                G->>G: Pass
            else exit code != 0
                G->>G: Fail (quotes the first 8 lines)
            end
        else kind = files
            G->>W: exists / absent / read_to_string + regex
            G->>G: Pass or Fail
        else kind = answer
            G->>G: contains / not_contains / matches / non_empty
            G->>G: Pass or Fail
        end
    end
    G-->>R: Vec<Grade>
    R->>O: CaseOutcome::from_parts(artifact, grades, judged_on_answer)
    O->>O: verdict()
    Note over O: failure → Inconclusive<br/>EmptyTurn without an answer grader → Inconclusive<br/>any Fail → Fail<br/>otherwise any Inconclusive → Inconclusive<br/>all Pass → Pass
    R->>O: summaries()
    Note over R,O: passes / failures / inconclusive<br/>pass_rate counts decisive attempts only<br/>cases_passing_any = pass@k<br/>cases_passing_all = regression gate
```

The diagrams render in any Markdown viewer that supports Mermaid, including
GitHub.
