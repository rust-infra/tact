use anyhow::Result;

use super::DynSessionStore;

/// RAII helper for session process locks. Call [`Self::release`] explicitly;
/// there is no `Drop` release (avoids failed cleanup on abnormal exit).
pub struct SessionLock {
    store: DynSessionStore,
    session_id: String,
    pid: u32,
    lock_epoch: String,
    active: bool,
}

impl SessionLock {
    pub async fn acquire(store: DynSessionStore, session_id: &str) -> Result<Self> {
        let pid = std::process::id();
        let lock_epoch = store.try_lock_session(session_id, pid).await?;
        Ok(Self { store, session_id: session_id.to_string(), pid, lock_epoch, active: true })
    }

    pub async fn release(mut self) -> Result<()> {
        if self.active {
            self.store.release_session_lock(&self.session_id, self.pid, &self.lock_epoch).await?;
            self.active = false;
        }
        Ok(())
    }
}
