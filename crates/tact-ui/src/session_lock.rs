use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tact::store::DynSessionStore;

/// Process-wide registry; `main` installs the exit-signal listener once.
pub struct SessionLockRegistry {
    guard: tokio::sync::Mutex<Option<Arc<SessionLockGuard>>>,
}

impl SessionLockRegistry {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            guard: tokio::sync::Mutex::new(None),
        })
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

    /// Listen for process exit signals and release any registered session lock.
    pub fn spawn_exit_listener(self: &Arc<Self>) {
        let registry = Arc::clone(self);
        tokio::spawn(async move {
            wait_for_exit_signal().await;
            if let Err(e) = registry.release_registered().await {
                eprintln!("[session lock] failed to release on exit signal: {e}");
            }
        });
    }
}

pub struct SessionLockGuard {
    store: DynSessionStore,
    session_id: String,
    pid: u32,
    released: AtomicBool,
}

impl SessionLockGuard {
    pub async fn acquire(store: DynSessionStore, session_id: &str) -> anyhow::Result<Arc<Self>> {
        let pid = std::process::id();
        store.try_lock_session(session_id, pid).await?;
        Ok(Arc::new(Self {
            store,
            session_id: session_id.to_string(),
            pid,
            released: AtomicBool::new(false),
        }))
    }

    pub async fn release(&self) -> anyhow::Result<()> {
        if self.released.swap(true, Ordering::SeqCst) {
            return Ok(());
        }
        self.store
            .release_session_lock(&self.session_id, self.pid)
            .await?;
        Ok(())
    }
}

async fn wait_for_exit_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};

        let mut sigint = signal(SignalKind::interrupt()).expect("failed to install SIGINT handler");
        let mut sigterm = signal(SignalKind::terminate()).expect("failed to install SIGTERM handler");
        tokio::select! {
            _ = sigint.recv() => {}
            _ = sigterm.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
