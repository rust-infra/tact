use crate::{PluginId, RequestId};
use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCategory {
    InvalidRequest,
    ProtocolMismatch,
    CapabilityNotFound,
    PermissionDenied,
    Timeout,
    Cancelled,
    PluginUnavailable,
    PluginCrashed,
    ProviderError,
    ToolError,
    StorageError,
    InternalError,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProtocolError {
    pub category: ErrorCategory,
    pub message: String,
    pub retryable: bool,
    pub origin: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<RequestId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin_id: Option<PluginId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

impl ProtocolError {
    pub fn new(
        category: ErrorCategory,
        message: impl Into<String>,
        origin: impl Into<String>,
        retryable: bool,
    ) -> Self {
        Self {
            category,
            message: message.into(),
            retryable,
            origin: origin.into(),
            request_id: None,
            plugin_id: None,
            details: None,
        }
    }
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.origin, self.message)
    }
}
impl std::error::Error for ProtocolError {}
