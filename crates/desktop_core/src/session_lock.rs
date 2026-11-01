use std::sync::{
    Arc, OnceLock,
    atomic::{AtomicUsize, Ordering},
};

use tact_host::SessionLockRegistry;

use crate::DesktopStartError;

static REGISTRY: OnceLock<Arc<SessionLockRegistry>> = OnceLock::new();
static INSTALLS: AtomicUsize = AtomicUsize::new(0);

pub(crate) fn registry() -> Arc<SessionLockRegistry> {
    REGISTRY.get_or_init(SessionLockRegistry::new).clone()
}

/// Install the process-wide lock-release listener exactly once.
pub fn install_exit_listener() -> Result<(), DesktopStartError> {
    if INSTALLS
        .compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Ok(());
    }
    let registry = registry();
    std::thread::Builder::new()
        .name("tact-desktop-exit-listener".into())
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("failed to create session-lock listener runtime");
            runtime.block_on(async move {
                registry.spawn_exit_listener();
                std::future::pending::<()>().await;
            });
        })
        .map_err(|error| {
            INSTALLS.store(0, Ordering::SeqCst);
            DesktopStartError::Runtime(error.to_string())
        })?;
    Ok(())
}

pub fn exit_listener_install_count() -> usize {
    INSTALLS.load(Ordering::SeqCst)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn exit_listener_is_installed_by_desktop_main_once() {
        install_exit_listener().unwrap();
        install_exit_listener().unwrap();
        assert_eq!(exit_listener_install_count(), 1);
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}
