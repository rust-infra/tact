use chrono::{DateTime, Utc};

use tact::store::DynSessionStore;

fn format_timestamp(dt: DateTime<Utc>) -> String {
    dt.format("%Y-%m-%d %H:%M:%S").to_string()
}

pub async fn print_sessions(session_store: &DynSessionStore, root_dir: &str) -> anyhow::Result<()> {
    let sessions = tact_host::sessions::list_sessions(session_store, root_dir).await?;
    if sessions.is_empty() {
        println!("No sessions found.");
        return Ok(());
    }

    println!(
        "{:<36}  {:>4}  {:<32}  {:<20}",
        "SESSION ID", "MSGS", "ROOT", "UPDATED"
    );
    println!("{}", "-".repeat(98));
    for session in &sessions {
        let updated = format_timestamp(session.updated_at);
        let root = if session.root_dir.is_empty() {
            "-"
        } else {
            &session.root_dir
        };
        println!(
            "{:<36}  {:>4}  {:<32}  {:<20}",
            session.id, session.message_count, root, updated
        );
    }
    Ok(())
}
