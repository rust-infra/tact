# Driver → ChatService: the concrete migration

## What blocks it today

`run_command_loop_with_account(agent: Agent, ...) -> Agent` takes ownership and
returns it. Inside, `agent` is moved into `tokio::spawn` tasks (508, 516, 541,
560, 573, 586) and returned through `active.await`. Every command handler takes
`&mut Agent`.

`RuntimeChatService` holds `Arc<tokio::sync::Mutex<Option<Agent>>>`. The two
ownership models cannot coexist: either the loop owns the agent (today) or the
service does (the target).

## The migration, in order

1. **Loop signature** — take `Arc<tokio::sync::Mutex<Option<Agent>>>` plus the
   chat service, and return the same Arc. The agent is installed once at the
   start by the caller, never moved out mid-run.
2. **Task bodies** — a spawned task currently moves `agent` in and hands it back.
   Instead it locks the slot for the duration of the turn. This is where the
   serialisation `ChatService` already provides becomes load-bearing: two
   commands cannot both hold the lock, which is the behaviour the current
   single-owner model gave for free.
3. **`SubmitTask`** — becomes `chat.submit(TurnRequest { message: Some(blocks), .. })`
   and the ~70 lines of loop/Stop-hook/completion code in `handle_user_command`
   are deleted. That deletion is the point: the code is not being wrapped, it is
   going away.
4. **`Compact`** — becomes `chat.compact()`.
5. **`Cancel`** — becomes `chat.cancel()`, which is already lock-free.
6. **Return value** — the caller takes the agent out of the slot once, at exit.

## Why it must be one slice, not several

Steps 1–3 have to land together: a loop that locks the slot but still moves the
agent into tasks does not compile, and a task that locks while the loop still
holds the agent deadlocks. Splitting them produces a state that is neither the
old design nor the new one.

## The risk worth naming

Step 2 turns a compile-time ownership guarantee into a runtime one. Today the
compiler proves only one task can hold the agent; after the change, a future
handler that forgets to lock would read a stale agent. The mitigation is that
`handle_user_command` keeps taking `&mut Agent` — the lock lives at the call
site, in one place, rather than being threaded through 164 accesses.
