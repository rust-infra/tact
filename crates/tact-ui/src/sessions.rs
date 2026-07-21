use chrono::{DateTime, Utc};
use tact::store::DynSessionStore;

fn format_timestamp(dt: DateTime<Utc>) -> String {
    dt.format("%Y-%m-%d %H:%M:%S").to_string()
}

pub async fn print_sessions(session_store: &DynSessionStore, root_dir: &str) -> anyhow::Result<()> {
    let sessions = session_store.list_sessions(Some(root_dir)).await?;
    if sessions.is_empty() {
        println!("No sessions found.");
    } else {
        println!("{:<36}  {:>4}  {:<32}  {:<20}", "SESSION ID", "MSGS", "ROOT", "UPDATED");
        println!("{}", "-".repeat(98));
        for s in &sessions {
            let updated = format_timestamp(s.updated_at);
            let root = if s.root_dir.is_empty() { "-".to_string() } else { s.root_dir.clone() };
            println!("{:<36}  {:>4}  {:<32}  {:<20}", s.id, s.message_count, root, updated);
        }
    }
    Ok(())
}
