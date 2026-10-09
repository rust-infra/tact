use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Value, json};
use tact_protocol::{PluginId, RequestId, SessionId};

use crate::store::{SessionStore, session_store::SqliteSessionStore};
use tact::{InvocationContext, KernelError, PermissionService, RuntimeContext, RuntimeServices};
use tact_llm::{Message, Role};

use super::session::SessionExtension;

struct AllowAll;

#[async_trait]
impl PermissionService for AllowAll {
    async fn check(
        &self,
        _declaration: &tact_protocol::CapabilityDeclaration,
        _context: &InvocationContext,
        _input: &Value,
    ) -> Result<(), KernelError> {
        Ok(())
    }
}

#[tokio::test]
async fn session_read_capability_returns_persisted_messages() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(
        SqliteSessionStore::new(&dir.path().join("sessions.db"))
            .await
            .unwrap(),
    );
    store
        .create_session("session-extension", "/workspace", "")
        .await
        .unwrap();
    store
        .append_message(
            "session-extension",
            Role::User,
            &Message::new_text(Role::User, "remember this").content,
            0,
        )
        .await
        .unwrap();

    let runtime = RuntimeContext::with_services(
        tact::CapabilityRouter::new(),
        RuntimeServices::with_permission(Arc::new(AllowAll)),
    );
    SessionExtension::new(store).register(&runtime).unwrap();
    let result = runtime
        .router()
        .invoke(
            "sessions.read",
            runtime
                .invocation(
                    RequestId::from("request-session-read"),
                    PluginId::from("tact.session"),
                    "test",
                )
                .with_session_id(SessionId::from("session-extension")),
            json!({"session_id": "session-extension"}),
        )
        .await
        .unwrap();

    assert_eq!(result["messages"][0]["content"], "remember this");
}

#[tokio::test]
async fn session_read_capability_rejects_missing_session_id() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(
        SqliteSessionStore::new(&dir.path().join("sessions.db"))
            .await
            .unwrap(),
    );
    let runtime = RuntimeContext::with_services(
        tact::CapabilityRouter::new(),
        RuntimeServices::with_permission(Arc::new(AllowAll)),
    );
    SessionExtension::new(store).register(&runtime).unwrap();

    let error = runtime
        .router()
        .invoke(
            "sessions.read",
            runtime.invocation(
                RequestId::from("request-session-invalid"),
                PluginId::from("tact.session"),
                "test",
            ),
            json!({}),
        )
        .await
        .unwrap_err();

    assert_eq!(
        error.category(),
        tact_protocol::ErrorCategory::InvalidRequest
    );
}

#[tokio::test]
async fn session_read_capability_cannot_read_an_unscoped_session() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(
        SqliteSessionStore::new(&dir.path().join("sessions.db"))
            .await
            .unwrap(),
    );
    store
        .create_session("private-session", "/workspace", "")
        .await
        .unwrap();
    let runtime = RuntimeContext::with_services(
        tact::CapabilityRouter::new(),
        RuntimeServices::with_permission(Arc::new(AllowAll)),
    );
    SessionExtension::new(store).register(&runtime).unwrap();

    let error = runtime
        .router()
        .invoke(
            "sessions.read",
            runtime
                .invocation(
                    RequestId::from("request-session-scope"),
                    PluginId::from("tact.session"),
                    "test",
                )
                .with_session_id(SessionId::from("another-session")),
            json!({"session_id": "private-session"}),
        )
        .await
        .unwrap_err();

    assert_eq!(
        error.category(),
        tact_protocol::ErrorCategory::PermissionDenied
    );
}

#[tokio::test]
async fn session_write_capability_appends_and_scopes_to_the_invocation() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(
        SqliteSessionStore::new(&dir.path().join("sessions.db"))
            .await
            .unwrap(),
    );
    store
        .create_session("session-write", "/workspace", "")
        .await
        .unwrap();

    let runtime = RuntimeContext::with_services(
        tact::CapabilityRouter::new(),
        RuntimeServices::with_permission(Arc::new(AllowAll)),
    );
    SessionExtension::new(store.clone())
        .register(&runtime)
        .unwrap();

    let result = runtime
        .router()
        .invoke(
            "sessions.write",
            runtime
                .invocation(
                    RequestId::from("request-session-write"),
                    PluginId::from("tact.session"),
                    "tester",
                )
                .with_session_id(tact_protocol::SessionId::from("session-write")),
            json!({"session_id": "session-write", "role": "user", "text": "hello"}),
        )
        .await
        .unwrap();
    assert!(result.get("message_id").is_some());
    assert_eq!(store.load_session("session-write").await.unwrap().len(), 1);

    let denied = runtime
        .router()
        .invoke(
            "sessions.write",
            runtime
                .invocation(
                    RequestId::from("request-session-write-other"),
                    PluginId::from("tact.session"),
                    "tester",
                )
                .with_session_id(tact_protocol::SessionId::from("other-session")),
            json!({"session_id": "session-write", "role": "user", "text": "hello"}),
        )
        .await
        .expect_err("a different session must be refused");
    assert_eq!(
        denied.category(),
        tact_protocol::ErrorCategory::PermissionDenied
    );
}
