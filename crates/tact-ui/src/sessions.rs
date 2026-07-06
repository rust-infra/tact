use chrono::{DateTime, Utc};
use tact::store::DynSessionStore;

fn format_timestamp(dt: DateTime<Utc>) -> String {
    dt.format("%Y-%m-%d %H:%M:%S").to_string()
}

pub(crate) async fn print_sessions(session_store: &DynSessionStore) -> anyhow::Result<()> {
    let sessions = session_store.list_sessions().await?;
    if sessions.is_empty() {
        println!("No sessions found.");
    } else {
        println!("{:<36}  {:>4}  {:<20}", "SESSION ID", "MSGS", "UPDATED");
        println!("{}", "-".repeat(66));
        for s in &sessions {
            let updated = format_timestamp(s.updated_at);
            println!("{:<36}  {:>4}  {:<20}", s.id, s.message_count, updated);
        }
    }
    Ok(())
}
