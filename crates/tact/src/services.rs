//! Kernel-owned service capabilities.
//!
//! These are the capabilities the Kernel itself provides (§4): a plugin invokes
//! them through the same `CapabilityRouter` path as any tool, so storage,
//! events, trajectory, and permission requests have one permission-checked
//! entry point. `runs.*` and `sessions.*` are extension-owned (Agent / Session)
//! and live beside the extension that implements them.

use std::sync::Arc;

use serde_json::{Value, json};
use tact_protocol::{
    CapabilityDeclaration, CapabilityKind, CapabilityRisk, ErrorCategory, InteractionRequest,
    RuntimeEvent, TrajectoryId,
};

use crate::{
    CapabilityRegistration, CapabilityRouter, FnCapabilityHandler, InvocationContext, KernelError,
};

/// Registers every Kernel-owned service capability on `router`.
pub fn register(router: &CapabilityRouter) -> Result<(), KernelError> {
    router.register_many(vec![
        storage_get(),
        storage_set(),
        events_publish(),
        trajectory_read(),
        trajectory_append_plugin_event(),
        permission_request(),
        interaction_request(),
    ])
}

fn declaration(name: &str, risk: CapabilityRisk, description: &str) -> CapabilityDeclaration {
    CapabilityDeclaration {
        name: name.into(),
        kind: CapabilityKind::Service,
        version: "1".into(),
        description: Some(description.into()),
        input_schema: None,
        output_schema: None,
        risk,
    }
}

fn registration<F, Fut>(
    name: &str,
    risk: CapabilityRisk,
    description: &str,
    handler: F,
) -> CapabilityRegistration
where
    F: Fn(InvocationContext, Value) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Result<Value, KernelError>> + Send + 'static,
{
    CapabilityRegistration::new(
        declaration(name, risk, description),
        Arc::new(FnCapabilityHandler::new(handler)),
    )
}

fn string_field(input: &Value, field: &str) -> Result<String, KernelError> {
    input
        .get(field)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| {
            KernelError::new(
                ErrorCategory::InvalidRequest,
                format!("{field} is required"),
                "kernel_service",
                false,
            )
        })
}

/// Refuses a storage namespace the caller does not own.
///
/// A plugin may only touch its own `plugins/<id>` namespace; the Runtime-owned
/// namespaces are reserved for the Runtime's own extensions and are otherwise
/// reached through the dedicated capability methods (trajectory / session).
fn authorize_namespace(
    context: &InvocationContext,
    namespace: &crate::StorageNamespace,
) -> Result<(), KernelError> {
    let caller = context.plugin_id().as_str();
    if let Some(owner) = namespace.plugin_owner() {
        if owner == caller {
            return Ok(());
        }
        return Err(KernelError::permission_denied(format!(
            "plugin {caller} cannot access plugin storage namespace {}",
            namespace.as_str()
        )));
    }
    if namespace.is_runtime_owned() && !caller.starts_with("tact.") {
        return Err(KernelError::permission_denied(format!(
            "storage namespace {} is reserved for the Runtime",
            namespace.as_str()
        )));
    }
    Ok(())
}

fn storage_get() -> CapabilityRegistration {
    registration(
        "storage.get",
        CapabilityRisk::ReadOnly,
        "Read a namespaced value",
        |context, input| async move {
            let namespace = string_field(&input, "namespace")?;
            let key = string_field(&input, "key")?;
            let parsed = crate::StorageNamespace::parse(&namespace)?;
            authorize_namespace(&context, &parsed)?;
            let value = context.storage().get(parsed.as_str(), &key).await?;
            Ok(value.unwrap_or(Value::Null))
        },
    )
}

fn storage_set() -> CapabilityRegistration {
    registration(
        "storage.set",
        CapabilityRisk::Medium,
        "Write a namespaced value",
        |context, input| async move {
            let namespace = string_field(&input, "namespace")?;
            let key = string_field(&input, "key")?;
            let parsed = crate::StorageNamespace::parse(&namespace)?;
            authorize_namespace(&context, &parsed)?;
            let value = input.get("value").cloned().unwrap_or(Value::Null);
            context.storage().set(parsed.as_str(), &key, value).await?;
            Ok(json!(null))
        },
    )
}

fn events_publish() -> CapabilityRegistration {
    registration(
        "events.publish",
        CapabilityRisk::Medium,
        "Publish a runtime event",
        |context, input| async move {
            let event: RuntimeEvent = serde_json::from_value(input).map_err(|error| {
                KernelError::new(
                    ErrorCategory::InvalidRequest,
                    error.to_string(),
                    "kernel_service",
                    false,
                )
            })?;
            context.events().publish(event).await?;
            Ok(json!(null))
        },
    )
}

fn trajectory_read() -> CapabilityRegistration {
    registration(
        "trajectory.read",
        CapabilityRisk::ReadOnly,
        "Read facts from a trajectory sequence",
        |context, input| async move {
            let id = string_field(&input, "trajectory_id")?;
            let trajectory_id = TrajectoryId::new(id).map_err(|error| {
                KernelError::new(
                    ErrorCategory::InvalidRequest,
                    error.to_string(),
                    "kernel_service",
                    false,
                )
            })?;
            let from_sequence = input
                .get("from_sequence")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let facts = context
                .trajectory()
                .query(&trajectory_id, from_sequence)
                .await?;
            serde_json::to_value(facts).map_err(|error| {
                KernelError::new(
                    ErrorCategory::InternalError,
                    error.to_string(),
                    "kernel_service",
                    false,
                )
            })
        },
    )
}

fn trajectory_append_plugin_event() -> CapabilityRegistration {
    registration(
        "trajectory.append_plugin_event",
        CapabilityRisk::Medium,
        "Append a namespaced plugin fact",
        |context, input| async move {
            let event: RuntimeEvent = serde_json::from_value(input).map_err(|error| {
                KernelError::new(
                    ErrorCategory::InvalidRequest,
                    error.to_string(),
                    "kernel_service",
                    false,
                )
            })?;
            let plugin_id = context.plugin_id().clone();
            if let Err(message) = event.validate_plugin_event(plugin_id.as_str()) {
                return Err(KernelError::new(
                    ErrorCategory::InvalidRequest,
                    message,
                    "kernel_service",
                    false,
                ));
            }
            context
                .trajectory()
                .append(context.trajectory_id(), context.run_id(), event)
                .await?;
            Ok(json!(null))
        },
    )
}

fn interaction_request() -> CapabilityRegistration {
    registration(
        "interaction.request",
        CapabilityRisk::Medium,
        "Request a user interaction",
        |context, input| async move {
            let request: InteractionRequest = serde_json::from_value(input).map_err(|error| {
                KernelError::new(
                    ErrorCategory::InvalidRequest,
                    error.to_string(),
                    "kernel_service",
                    false,
                )
            })?;
            let response = context.interaction().request(request, &context).await?;
            serde_json::to_value(response).map_err(|error| {
                KernelError::new(
                    ErrorCategory::InternalError,
                    error.to_string(),
                    "kernel_service",
                    false,
                )
            })
        },
    )
}

fn permission_request() -> CapabilityRegistration {
    registration(
        "permission.request",
        CapabilityRisk::Medium,
        "Request a permission decision",
        |context, input| async move {
            let request: InteractionRequest = serde_json::from_value(input).map_err(|error| {
                KernelError::new(
                    ErrorCategory::InvalidRequest,
                    error.to_string(),
                    "kernel_service",
                    false,
                )
            })?;
            let response = context.permission().request(request, &context).await?;
            serde_json::to_value(response).map_err(|error| {
                KernelError::new(
                    ErrorCategory::InternalError,
                    error.to_string(),
                    "kernel_service",
                    false,
                )
            })
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        EventTransport, InteractionBroker, PermissionService, RuntimeContext, RuntimeServices,
        StorageServiceImpl, TrajectoryService,
    };
    use tact_protocol::{
        CapabilityDeclaration, InteractionRequest, InteractionResponse, PluginId, RequestId, RunId,
    };

    struct AllowPermission;

    #[async_trait::async_trait]
    impl PermissionService for AllowPermission {
        async fn check(
            &self,
            _declaration: &CapabilityDeclaration,
            _context: &InvocationContext,
            _input: &Value,
        ) -> Result<(), KernelError> {
            Ok(())
        }
    }

    struct MemoryTrajectory {
        facts: std::sync::Mutex<Vec<tact_protocol::TrajectoryEvent>>,
    }

    #[async_trait::async_trait]
    impl TrajectoryService for MemoryTrajectory {
        async fn append(
            &self,
            trajectory_id: Option<&TrajectoryId>,
            run_id: Option<&RunId>,
            event: RuntimeEvent,
        ) -> Result<(), KernelError> {
            let mut facts = self.facts.lock().unwrap();
            let sequence = facts.len() as u64;
            facts.push(tact_protocol::TrajectoryEvent {
                trajectory_id: trajectory_id
                    .cloned()
                    .unwrap_or_else(|| TrajectoryId::from("runtime")),
                run_id: run_id.cloned().unwrap_or_else(|| RunId::from("runtime")),
                sequence,
                timestamp: chrono::Utc::now(),
                actor: "test".into(),
                event_type: tact_protocol::TrajectoryEventType::Message,
                parent_step_id: None,
                payload: serde_json::to_value(&event).unwrap(),
                sensitivity: tact_protocol::Sensitivity::Internal,
            });
            Ok(())
        }

        async fn query(
            &self,
            _trajectory_id: &TrajectoryId,
            from_sequence: u64,
        ) -> Result<Vec<tact_protocol::TrajectoryEvent>, KernelError> {
            Ok(self
                .facts
                .lock()
                .unwrap()
                .iter()
                .filter(|fact| fact.sequence >= from_sequence)
                .cloned()
                .collect())
        }
    }

    fn runtime() -> RuntimeContext {
        let router = crate::CapabilityRouter::new();
        register(&router).expect("register kernel services");
        RuntimeContext::with_services(
            router,
            RuntimeServices::new(
                std::sync::Arc::new(EventTransport::new(8)),
                std::sync::Arc::new(MemoryTrajectory {
                    facts: std::sync::Mutex::new(Vec::new()),
                }),
                std::sync::Arc::new(AllowPermission),
                std::sync::Arc::new(StorageServiceImpl::default()),
            ),
        )
    }

    #[tokio::test]
    async fn storage_get_set_round_trips_through_the_router() {
        let runtime = runtime();
        let context = runtime.invocation(
            RequestId::from("req-storage"),
            PluginId::from("test.plugin"),
            "test",
        );
        runtime
            .router()
            .invoke(
                "storage.set",
                context.clone(),
                json!({"namespace": "plugins/test.plugin", "key": "k", "value": 7}),
            )
            .await
            .expect("storage.set");
        let value = runtime
            .router()
            .invoke(
                "storage.get",
                context,
                json!({"namespace": "plugins/test.plugin", "key": "k"}),
            )
            .await
            .expect("storage.get");
        assert_eq!(value, json!(7));
    }

    #[tokio::test]
    async fn trajectory_append_and_read_through_the_router() {
        let runtime = runtime();
        let context = runtime.invocation(
            RequestId::from("req-traj"),
            PluginId::from("test.plugin"),
            "test",
        );
        let fact = RuntimeEvent::Plugin {
            plugin_id: PluginId::from("test.plugin"),
            origin: "plugin".into(),
            event_type: "plugin.test.plugin.progress".into(),
            payload: json!({"done": true}),
        };
        runtime
            .router()
            .invoke(
                "trajectory.append_plugin_event",
                context.clone(),
                serde_json::to_value(fact).unwrap(),
            )
            .await
            .expect("trajectory.append_plugin_event");
        let facts = runtime
            .router()
            .invoke(
                "trajectory.read",
                context,
                json!({"trajectory_id": "runtime", "from_sequence": 0}),
            )
            .await
            .expect("trajectory.read");
        assert_eq!(facts.as_array().map(Vec::len), Some(1));
    }

    #[tokio::test]
    async fn storage_refuses_namespaces_the_caller_does_not_own() {
        let runtime = runtime();
        let context = runtime.invocation(
            RequestId::from("req-namespace"),
            PluginId::from("test.plugin"),
            "test",
        );
        let other = runtime
            .router()
            .invoke(
                "storage.set",
                context.clone(),
                json!({"namespace": "plugins/other.plugin", "key": "k", "value": 1}),
            )
            .await
            .expect_err("another plugin's namespace must be refused");
        assert_eq!(
            other.category(),
            tact_protocol::ErrorCategory::PermissionDenied
        );

        let runtime_owned = runtime
            .router()
            .invoke(
                "storage.set",
                context,
                json!({"namespace": "sessions", "key": "k", "value": 1}),
            )
            .await
            .expect_err("a runtime-owned namespace must be refused for a plugin");
        assert_eq!(
            runtime_owned.category(),
            tact_protocol::ErrorCategory::PermissionDenied
        );
    }

    #[tokio::test]
    async fn interaction_request_round_trips_through_the_router() {
        let broker = InteractionBroker::new(8);
        let router = crate::CapabilityRouter::new();
        register(&router).expect("register kernel services");
        let runtime = RuntimeContext::with_services(
            router,
            RuntimeServices::with_permission(std::sync::Arc::new(AllowPermission))
                .with_interaction(std::sync::Arc::new(broker.clone())),
        );
        let context = runtime.invocation(
            RequestId::from("req-interaction"),
            PluginId::from("test.plugin"),
            "test",
        );
        let mut subscription = broker.subscribe();
        let responder = tokio::spawn(async move {
            let request = subscription.recv().await.expect("a request");
            let request_id = match &request {
                InteractionRequest::Confirm { request_id, .. } => request_id.clone(),
                other => panic!("expected confirm, got {other:?}"),
            };
            broker.respond(InteractionResponse::Approved { request_id });
        });
        let response = runtime
            .router()
            .invoke(
                "interaction.request",
                context,
                json!({"type": "confirm", "request_id": "req-interaction", "prompt": "ok?"}),
            )
            .await
            .expect("interaction.request");
        responder.await.expect("responder");
        let response: InteractionResponse = serde_json::from_value(response).unwrap();
        assert!(matches!(response, InteractionResponse::Approved { .. }));
    }
}
