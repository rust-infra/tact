//! Host-side WASI service mediation for a plugin invocation.

use std::{collections::BTreeSet, time::SystemTime};

use serde_json::{Value, json};
use tact::kernel::{CapabilityRouter, InvocationContext, KernelError};
use tact_protocol::{
    CapabilityDeclaration, CapabilityKind, CapabilityRisk, ErrorCategory, PluginId, RuntimeEvent,
};

const BUILTIN_HOST_CAPABILITIES: &[&str] = &[
    "storage.get",
    "storage.set",
    "events.publish",
    "trajectory.append_plugin_event",
    "clock.read",
];

/// Narrow service facade created from a validated WASM manifest.
#[derive(Clone)]
pub struct WasmHostFunctions {
    plugin_id: PluginId,
    granted: BTreeSet<String>,
}

impl WasmHostFunctions {
    pub fn new(plugin_id: PluginId, grants: Vec<String>) -> anyhow::Result<Self> {
        for grant in &grants {
            if !is_known_host_capability(grant) {
                anyhow::bail!("unsupported WASM host capability: {grant}");
            }
        }
        Ok(Self {
            plugin_id,
            granted: grants.into_iter().collect(),
        })
    }

    pub async fn get_storage(
        &self,
        context: &InvocationContext,
        key: &str,
    ) -> Result<Option<Value>, KernelError> {
        let input = json!({ "key": key });
        self.authorize("storage.get", context, &input).await?;
        context.storage().get(&self.plugin_namespace(), key).await
    }

    pub async fn set_storage(
        &self,
        context: &InvocationContext,
        key: &str,
        value: Value,
    ) -> Result<(), KernelError> {
        let input = json!({ "key": key, "value": value });
        self.authorize("storage.set", context, &input).await?;
        context
            .storage()
            .set(&self.plugin_namespace(), key, input["value"].clone())
            .await
    }

    pub async fn publish_event(
        &self,
        context: &InvocationContext,
        event: RuntimeEvent,
    ) -> Result<(), KernelError> {
        event
            .validate_plugin_event(self.plugin_id.as_str())
            .map_err(|message| {
                KernelError::new(ErrorCategory::InvalidRequest, message, "wasm_plugin", false)
            })?;
        let input = json!({ "event": event });
        self.authorize("events.publish", context, &input).await?;
        self.authorize("trajectory.append_plugin_event", context, &input)
            .await?;
        let event: RuntimeEvent =
            serde_json::from_value(input["event"].clone()).map_err(|error| {
                KernelError::new(
                    ErrorCategory::InvalidRequest,
                    error.to_string(),
                    "wasm_plugin",
                    false,
                )
            })?;
        context
            .trajectory()
            .append(context.trajectory_id(), context.run_id(), event.clone())
            .await?;
        context.events().publish(event).await
    }

    pub async fn clock_now(&self, context: &InvocationContext) -> Result<Value, KernelError> {
        self.authorize("clock.read", context, &Value::Null).await?;
        let milliseconds = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_err(|error| {
                KernelError::new(
                    ErrorCategory::InternalError,
                    error.to_string(),
                    "wasm_plugin",
                    false,
                )
            })?
            .as_millis();
        Ok(Value::String(milliseconds.to_string()))
    }

    pub async fn invoke_external(
        &self,
        router: &CapabilityRouter,
        context: InvocationContext,
        capability: &str,
        input: Value,
    ) -> Result<Value, KernelError> {
        let grant = format!("capability:{capability}");
        self.require_grant(&grant)?;
        router.invoke(capability, context, input).await
    }

    /// Dispatch one guest `HostCall` after verifying its manifest grant and
    /// applying the same permission policy used for native and remote tools.
    pub async fn handle_call(
        &self,
        context: &InvocationContext,
        router: &CapabilityRouter,
        capability: &str,
        input: Value,
    ) -> Result<Value, KernelError> {
        match capability {
            "clock.read" => self.clock_now(context).await,
            "storage.get" => {
                let key = input.get("key").and_then(Value::as_str).ok_or_else(|| {
                    KernelError::new(
                        ErrorCategory::InvalidRequest,
                        "storage.get requires a string key",
                        "wasm_plugin",
                        false,
                    )
                })?;
                let value = self.get_storage(context, key).await?;
                serde_json::to_value(value).map_err(|error| {
                    KernelError::new(
                        ErrorCategory::InternalError,
                        error.to_string(),
                        "wasm_plugin",
                        false,
                    )
                })
            }
            "storage.set" => {
                let key = input.get("key").and_then(Value::as_str).ok_or_else(|| {
                    KernelError::new(
                        ErrorCategory::InvalidRequest,
                        "storage.set requires a string key",
                        "wasm_plugin",
                        false,
                    )
                })?;
                let value = input.get("value").cloned().ok_or_else(|| {
                    KernelError::new(
                        ErrorCategory::InvalidRequest,
                        "storage.set requires a value",
                        "wasm_plugin",
                        false,
                    )
                })?;
                self.set_storage(context, key, value).await?;
                Ok(json!({ "stored": true }))
            }
            "events.publish" | "trajectory.append_plugin_event" => {
                let event_value = input.get("event").cloned().unwrap_or(input);
                let event = serde_json::from_value(event_value).map_err(|error| {
                    KernelError::new(
                        ErrorCategory::InvalidRequest,
                        error.to_string(),
                        "wasm_plugin",
                        false,
                    )
                })?;
                self.publish_event(context, event).await?;
                Ok(json!({ "published": true }))
            }
            external if external.starts_with("capability:") => {
                let name = &external["capability:".len()..];
                self.invoke_external(router, context.clone(), name, input)
                    .await
            }
            _ => Err(KernelError::permission_denied(format!(
                "WASM plugin was not granted {capability}"
            ))),
        }
    }

    fn plugin_namespace(&self) -> String {
        format!("plugins/{}", self.plugin_id.as_str())
    }

    fn require_grant(&self, name: &str) -> Result<(), KernelError> {
        if self.granted.contains(name) {
            Ok(())
        } else {
            Err(KernelError::permission_denied(format!(
                "WASM plugin was not granted {name}"
            )))
        }
    }

    async fn authorize(
        &self,
        name: &str,
        context: &InvocationContext,
        input: &Value,
    ) -> Result<(), KernelError> {
        self.require_grant(name)?;
        let declaration = service_declaration(name);
        context
            .permission()
            .check(&declaration, context, input)
            .await
    }
}

pub(crate) fn is_known_host_capability(name: &str) -> bool {
    BUILTIN_HOST_CAPABILITIES.contains(&name)
        || name
            .strip_prefix("capability:")
            .is_some_and(|capability| !capability.trim().is_empty())
}

fn service_declaration(name: &str) -> CapabilityDeclaration {
    CapabilityDeclaration {
        name: name.into(),
        kind: CapabilityKind::Service,
        version: "1".into(),
        description: None,
        input_schema: None,
        output_schema: None,
        risk: match name {
            "storage.set" | "events.publish" | "trajectory.append_plugin_event" => {
                CapabilityRisk::Low
            }
            _ => CapabilityRisk::ReadOnly,
        },
    }
}
