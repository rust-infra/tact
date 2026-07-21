use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use tact::store::DynSessionStore;

/// Process-wide registry; `main` installs the exit-signal listener once.
pub struct SessionLockRegistry {
    guard: tokio::sync::Mutex<Option<Arc<SessionLockGuard>>>,
}

impl SessionLockRegistry {
    pub fn new() -> Arc<Self> {
        Arc::new(Self { guard: tokio::sync::Mutex::new(None) })
    }

    pub async fn register(&self, guard: Arc<SessionLockGuard>) {
        *self.guard.lock().await = Some(guard);
    }

    pub async fn release_registered(&self) -> anyhow::Result<()> {
        if let Some(guard) = self.guard.lock().await.take() {
            guard.release().await?;
        }
        Ok(())
    }

    /// Listen for process exit signals, release any registered session lock, then exit.
    ///
    /// Releasing without terminating would allow another process to acquire the lock
    /// while this instance keeps writing to the same session.
    pub fn spawn_exit_listener(self: &Arc<Self>) {
        let registry = Arc::clone(self);
        tokio::spawn(async move {
            let signal = wait_for_exit_signal().await;
            if let Err(e) = registry.release_registered().await {
                eprintln!("[session lock] failed to release on exit signal: {e}");
            }
            exit_after_signal(signal);
        });
    }
}

pub struct SessionLockGuard {
    store: DynSessionStore,
    session_id: String,
    pid: u32,
    lock_epoch: String,
    released: AtomicBool,
}

impl SessionLockGuard {
    pub async fn acquire(store: DynSessionStore, session_id: &str) -> anyhow::Result<Arc<Self>> {
        let pid = std::process::id();
        const MAX_RETRIES: u32 = 5;
        let mut lock_epoch = None;
        for attempt in 0..MAX_RETRIES {
            match store.try_lock_session(session_id, pid).await {
                Ok(epoch) => {
                    lock_epoch = Some(epoch);
                    break;
                },
                Err(e) if e.to_string().contains("lock conflict; retry") && attempt + 1 < MAX_RETRIES => {
                    tokio::time::sleep(std::time::Duration::from_millis(50 * u64::from(attempt + 1))).await;
                },
                Err(e) => return Err(e),
            }
        }
        let lock_epoch =
            lock_epoch.ok_or_else(|| anyhow::anyhow!("failed to acquire session lock after {MAX_RETRIES} attempts"))?;
        Ok(Arc::new(Self {
            store,
            session_id: session_id.to_string(),
            pid,
            lock_epoch,
            released: AtomicBool::new(false),
        }))
    }

    pub async fn release(&self) -> anyhow::Result<()> {
        if self.released.swap(true, Ordering::SeqCst) {
            return Ok(());
        }
        self.store.release_session_lock(&self.session_id, self.pid, &self.lock_epoch).await?;
        Ok(())
    }
}

enum ExitSignal {
    Interrupt,
    Terminate,
}

async fn wait_for_exit_signal() -> ExitSignal {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};

        let mut sigint = signal(SignalKind::interrupt()).expect("failed to install SIGINT handler");
        let mut sigterm = signal(SignalKind::terminate()).expect("failed to install SIGTERM handler");
        tokio::select! {
            _ = sigint.recv() => ExitSignal::Interrupt,
            _ = sigterm.recv() => ExitSignal::Terminate,
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
        ExitSignal::Interrupt
    }
}

fn exit_after_signal(signal: ExitSignal) {
    let code = match signal {
        ExitSignal::Interrupt => 130,
        ExitSignal::Terminate => 143,
    };
    std::process::exit(code);
}
