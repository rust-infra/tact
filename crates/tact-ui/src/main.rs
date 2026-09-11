use std::{
    fs::{File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use tact::{
    config::{CliCommand, init},
    consts::TactPath,
    store::open_sqlite_session_store,
};
use tact_ui::{
    run_headless, run_interactive, session_lock::SessionLockRegistry, sessions::print_sessions,
};
use tracing_subscriber::prelude::*;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut args = init()?;

    let tokio_console = tact::config::settings().tokio_console;
    if tokio_console || std::env::var_os("RUST_LOG").is_some() {
        let log_path = init_logging(tokio_console);
        if tokio_console {
            eprintln!("[tokio-console] listening on http://127.0.0.1:6669");
            if let Some(log_path) = log_path {
                eprintln!("[tokio-console] tracing log: {}", log_path.display());
            }
        }
    }

    // Self-upgrade does not need a session store or provider config.
    if matches!(args.command, Some(CliCommand::Upgrade { .. })) {
        let Some(CliCommand::Upgrade { repo, yes, check }) = args.command.take() else {
            unreachable!("upgrade command was just matched");
        };
        return tact::upgrade::run_upgrade(tact::upgrade::UpgradeOptions { repo, yes, check })
            .await;
    }

    let tact_path = TactPath::from_cwd()?;
    let db_path = tact_path.session_db_path();
    let session_store = open_sqlite_session_store(&db_path).await?;

    if args.list_sessions {
        print_sessions(&session_store, &tact_path.workdir().display().to_string()).await?;
        return Ok(());
    }

    let lock_registry = SessionLockRegistry::new();
    lock_registry.spawn_exit_listener();

    match args.command.take() {
        Some(CliCommand::Headless { prompt }) => {
            return run_headless(args, prompt, tact_path, session_store, lock_registry).await;
        }
        Some(CliCommand::Plugin { command }) => {
            if let Err(e) = tact_ui::plugin_cli::run_plugin_cli(command).await {
                eprintln!("Error: {e:#}");
                std::process::exit(1);
            }
            return Ok(());
        }
        Some(CliCommand::Mcp { command }) => {
            if let Err(e) = tact_ui::mcp_cli::run_mcp_cli(command).await {
                eprintln!("Error: {e:#}");
                std::process::exit(1);
            }
            return Ok(());
        }
        _ => {}
    }

    run_interactive(args, tact_path, session_store, lock_registry).await
}

fn init_logging(tokio_console: bool) -> Option<PathBuf> {
    // The interactive TUI owns stdout/stderr; never install a terminal fmt
    // layer. `console_subscriber::spawn()` supplies the live tokio-console
    // layer when requested, while the optional fmt layer writes tracing events
    // to a date-rotated file. `RUST_LOG` controls verbosity (for example
    // `RUST_LOG=tact_llm=debug`).
    let log_dir = open_log_dir();
    let path = log_dir
        .as_ref()
        .map(|dir| daily_log_path(dir, &current_date_string()));
    let file_layer = log_dir.map(|dir| {
        // Touch today's file so it exists even before the first trace event.
        let _ = OpenOptions::new()
            .create(true)
            .append(true)
            .open(daily_log_path(&dir, &current_date_string()));
        tracing_subscriber::fmt::layer()
            .with_ansi(false)
            .with_writer(DailyLogMakeWriter::new(dir))
            .with_filter(tracing_subscriber::EnvFilter::from_default_env())
    });
    let console_layer = tokio_console.then(console_subscriber::spawn);

    tracing_subscriber::registry()
        .with(console_layer)
        .with(file_layer)
        .init();

    path
}

fn open_log_dir() -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    let log_dir = TactPath::new(cwd).tact_dir().join("logs");
    std::fs::create_dir_all(&log_dir).ok()?;
    Some(log_dir)
}

fn current_date_string() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

fn daily_log_path(log_dir: &Path, date: &str) -> PathBuf {
    log_dir.join(format!("tact-{date}.log"))
}

#[derive(Clone)]
struct DailyLogMakeWriter {
    dir: PathBuf,
    state: Arc<Mutex<DailyLogState>>,
}

struct DailyLogState {
    date: String,
    file: Option<File>,
}

impl DailyLogMakeWriter {
    fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            state: Arc::new(Mutex::new(DailyLogState {
                date: String::new(),
                file: None,
            })),
        }
    }
}

struct DailyLogWriter {
    inner: DailyLogMakeWriter,
}

impl<'a> tracing_subscriber::fmt::writer::MakeWriter<'a> for DailyLogMakeWriter {
    type Writer = DailyLogWriter;

    fn make_writer(&'a self) -> Self::Writer {
        DailyLogWriter {
            inner: self.clone(),
        }
    }
}

impl Write for DailyLogWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let today = current_date_string();
        let Ok(mut state) = self.inner.state.lock() else {
            return Ok(buf.len());
        };

        if state.date != today {
            let path = daily_log_path(&self.inner.dir, &today);
            state.file = OpenOptions::new().create(true).append(true).open(path).ok();
            state.date = today;
        }

        match state.file.as_mut() {
            Some(file) => file.write(buf),
            None => Ok(buf.len()),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        let Ok(mut state) = self.inner.state.lock() else {
            return Ok(());
        };
        match state.file.as_mut() {
            Some(file) => file.flush(),
            None => Ok(()),
        }
    }
}
