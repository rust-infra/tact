# tact_host

`tact_host` is the UI-independent orchestration layer for Tact frontends. It
owns the parts that are identical for the terminal and a desktop client:

- session selection, bootstrap, locking, and teardown;
- the `UserCommand` -> `Agent` command loop;
- Goal Runner and Goal recovery;
- account/quota polling;
- permission-mode mapping and user-message construction.

It doesn't depend on `tui`, Ratatui, Crossterm, or any renderer.

## Frontend shape

Keep the frontend-specific pieces separate:

```text
tact_host
  -> tact_protocol::UpdateReceiver / UserCommand
  -> TUI adapter (tact-ui)
  -> desktop adapter (Tauri / egui / native)
```

`session_bootstrap::Frontend::Interactive` is the claim that the frontend can
answer `RequestSelect` / `RequestMultiSelect` prompts. An observing frontend
must use `Frontend::Observing`; it receives events but cannot answer prompts.

## Minimal desktop flow

```rust,no_run
use std::sync::Arc;

use tact_host::{
    CliArgs, DynSessionStore, Frontend, Notices, Result, SessionLockRegistry, TactPath, UiResponder,
    UiWiring, UserCommand, bootstrap_session, command_channel, init_config, open_session,
    run_command_loop_with_account, shared_skill_registry, update_channel,
};

# async fn run(
#     args: CliArgs,
#     tact_path: TactPath,
#     store: DynSessionStore,
#     locks: Arc<SessionLockRegistry>,
# ) -> Result<()> {
// Call once before opening a session. `CliArgs::default()` lets a desktop
// host choose a config file without depending on process argv.
init_config(&args)?;

let (session_id, lock) = open_session(&args, &tact_path, &store, &locks).await?;
let (updates, update_rx) = update_channel(256);
let (commands, command_rx) = command_channel();
let responder = UiResponder::new();

let agent = bootstrap_session(
    &tact_path,
    tact_path.workdir().to_path_buf(),
    shared_skill_registry(tact_path.workdir())?,
    session_id,
    store,
    Frontend::Interactive(UiWiring {
        tx: updates.clone(),
        responder,
    }),
    Notices::Ui(updates),
)
.await?;

let driver = tokio::spawn(run_command_loop_with_account(
    agent,
    command_rx,
    tact_path.workdir().to_path_buf(),
    None,
));

// The desktop adapter owns update_rx and commands. On teardown, request
// cancellation explicitly, close the command channel, then await the driver.
let _ = commands.send(UserCommand::Cancel);
drop(commands);
let driver_result = driver.await;
lock.release().await?;
let _agent = driver_result?;

# let _ = update_rx;
# Ok(())
# }
```

The host facade re-exports the protocol, agent, path, session-store, and
responder types used above, so a frontend does not need to mirror
`tact_host`'s internal dependency versions. It still needs its own `tokio`
dependency to spawn the driver.

Closing the command channel after the final `SubmitTask` drains the active
turn; it does not imply cancellation. A frontend that is tearing down must send
`UserCommand::Cancel` first, as the example does.

The current `AgentUpdate` / `UserCommand` transport is in-process. A remote or
WebView transport should add a versioned serializable envelope rather than
making the in-process channel a network protocol.
