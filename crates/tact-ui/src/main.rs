use tact::config::{CliCommand, init};
use tact::consts::TactPath;
use tact::store::open_sqlite_session_store;

use tact_ui::run_headless;
use tact_ui::run_interactive;
use tact_ui::session_lock::SessionLockRegistry;
use tact_ui::sessions::print_sessions;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut args = init()?;

    if tact::config::settings().tokio_console {
        console_subscriber::init();
        eprintln!("[tokio-console] listening on http://127.0.0.1:6669");
    }

    let tact_path = TactPath::from_cwd()?;
    let db_path = tact_path.session_db_path();
    let session_store = open_sqlite_session_store(&db_path).await?;

    if args.list_sessions {
        print_sessions(
            &session_store,
            &tact_path.workdir().display().to_string(),
        )
        .await?;
        return Ok(());
    }

    let lock_registry = SessionLockRegistry::new();
    lock_registry.spawn_exit_listener();

    if let Some(CliCommand::Headless { prompt }) = args.command.take() {
        return run_headless(args, prompt, tact_path, session_store, lock_registry).await;
    }

    run_interactive(args, tact_path, session_store, lock_registry).await
}
