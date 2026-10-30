use tact::store::DynSessionStore;
use tact::store::session_store::SessionSummary;

pub async fn list_sessions(
    session_store: &DynSessionStore,
    root_dir: &str,
) -> anyhow::Result<Vec<SessionSummary>> {
    session_store.list_sessions(Some(root_dir)).await
}
