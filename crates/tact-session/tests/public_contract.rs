use std::sync::Arc;

use chrono::{TimeZone, Utc};
use tact_session::{
    DynSessionStore, MAX_INPUT_HISTORY, MAX_TOKEN_USAGE_BODIES, MessageCountByPeriod,
    SessionSummary,
};

fn assert_send_sync<T: Send + Sync>() {}

#[test]
fn session_contract_exports_stable_value_types() {
    assert_eq!(MAX_INPUT_HISTORY, 100);
    assert_eq!(MAX_TOKEN_USAGE_BODIES, 1);

    let summary = SessionSummary {
        id: "session".into(),
        root_dir: "/tmp/project".into(),
        created_at: Utc.timestamp_opt(0, 0).single().unwrap(),
        updated_at: Utc.timestamp_opt(1, 0).single().unwrap(),
        message_count: 0,
    };
    let count = MessageCountByPeriod {
        period: "day".into(),
        label: "1970-01-01".into(),
        count: 0,
    };
    assert_eq!(summary.id, "session");
    assert_eq!(count.period, "day");
}

#[test]
fn dynamic_store_handle_remains_send_sync() {
    assert_send_sync::<DynSessionStore>();
    assert_send_sync::<Arc<dyn tact_session::SessionStore>>();
}
