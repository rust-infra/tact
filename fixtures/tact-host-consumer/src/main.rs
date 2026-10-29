#![allow(dead_code)]

use std::sync::Arc;

use tact_host::{
    AgentUpdate, CliArgs, DynSessionStore, Frontend, Notices, Result, SessionLockRegistry,
    TactPath, UiResponder, UiWiring, UserCommand, bootstrap_session, command_channel, init_config,
    open_session, run_command_loop_with_account, shared_skill_registry, update_channel,
};

async fn run_frontend(
    args: &CliArgs,
    tact_path: TactPath,
    store: DynSessionStore,
    locks: Arc<SessionLockRegistry>,
) -> Result<()> {
    let (session_id, lock) = open_session(args, &tact_path, &store, &locks).await?;
    let (updates, mut update_rx) = update_channel(256);
    let (commands, command_rx) = command_channel();

    let agent = bootstrap_session(
        &tact_path,
        tact_path.workdir().to_path_buf(),
        shared_skill_registry(tact_path.workdir())?,
        session_id,
        store,
        Frontend::Interactive(UiWiring {
            tx: updates.clone(),
            responder: UiResponder::new(),
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
    let _ = commands.send(UserCommand::Cancel);
    drop(commands);

    let driver_result = driver.await;
    lock.release().await?;
    let _agent = driver_result?;

    let _pending_update: Option<AgentUpdate> = update_rx.try_recv().ok();
    Ok(())
}

fn initialize_host(args: &CliArgs) -> Result<()> {
    init_config(args)
}

fn main() {}
