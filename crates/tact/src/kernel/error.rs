//! Stable Kernel error conversion around the protocol error model.

use std::fmt;

use tact_protocol::{ErrorCategory, PluginId, ProtocolError, RequestId};

/// Error returned by Kernel services and capability implementations.
///
/// The wire representation remains [`ProtocolError`]. Keeping that value
/// intact ensures request/plugin identity, origin and retryability survive
/// every adapter boundary.
#[derive(Debug, Clone)]
pub struct KernelError {
    protocol: ProtocolError,
}

impl KernelError {
    #[must_use]
    pub fn new(
        category: ErrorCategory,
        message: impl Into<String>,
        origin: impl Into<String>,
        retryable: bool,
    ) -> Self {
        Self {
            protocol: ProtocolError::new(category, message, origin, retryable),
        }
    }

    #[must_use]
    pub fn from_protocol(error: ProtocolError) -> Self {
        Self { protocol: error }
    }

    #[must_use]
    pub fn as_protocol_error(&self) -> &ProtocolError {
        &self.protocol
    }

    #[must_use]
    pub fn into_protocol_error(self) -> ProtocolError {
        self.protocol
    }

    #[must_use]
    pub fn category(&self) -> ErrorCategory {
        self.protocol.category
    }

    #[must_use]
    pub fn message(&self) -> &str {
        &self.protocol.message
    }

    #[must_use]
    pub fn origin(&self) -> &str {
        &self.protocol.origin
    }

    #[must_use]
    pub fn retryable(&self) -> bool {
        self.protocol.retryable
    }

    #[must_use]
    pub fn request_id(&self) -> Option<&RequestId> {
        self.protocol.request_id.as_ref()
    }

    #[must_use]
    pub fn plugin_id(&self) -> Option<&PluginId> {
        self.protocol.plugin_id.as_ref()
    }

    #[must_use]
    pub fn with_request_id(mut self, request_id: RequestId) -> Self {
        self.protocol.request_id = Some(request_id);
        self
    }

    #[must_use]
    pub fn with_plugin_id(mut self, plugin_id: PluginId) -> Self {
        self.protocol.plugin_id = Some(plugin_id);
        self
    }

    #[must_use]
    pub fn capability_not_found(name: impl Into<String>) -> Self {
        Self::new(
            ErrorCategory::CapabilityNotFound,
            format!("capability not found: {}", name.into()),
            "kernel",
            false,
        )
    }

    #[must_use]
    pub fn duplicate_capability(name: impl Into<String>) -> Self {
        Self::new(
            ErrorCategory::InvalidRequest,
            format!("capability already registered: {}", name.into()),
            "kernel",
            false,
        )
    }

    #[must_use]
    pub fn cancelled() -> Self {
        Self::new(
            ErrorCategory::Cancelled,
            "invocation cancelled",
            "kernel",
            true,
        )
    }

    #[must_use]
    pub fn timeout() -> Self {
        Self::new(
            ErrorCategory::Timeout,
            "invocation deadline exceeded",
            "kernel",
            true,
        )
    }

    #[must_use]
    pub fn permission_denied(message: impl Into<String>) -> Self {
        Self::new(
            ErrorCategory::PermissionDenied,
            message,
            "permission",
            false,
        )
    }

    #[must_use]
    pub fn storage(message: impl Into<String>) -> Self {
        Self::new(ErrorCategory::StorageError, message, "storage", true)
    }
}

impl From<ProtocolError> for KernelError {
    fn from(error: ProtocolError) -> Self {
        Self::from_protocol(error)
    }
}

impl From<KernelError> for ProtocolError {
    fn from(error: KernelError) -> Self {
        error.into_protocol_error()
    }
}

impl fmt::Display for KernelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.protocol.fmt(f)
    }
}

impl std::error::Error for KernelError {}

impl From<tokio::time::error::Elapsed> for KernelError {
    fn from(_: tokio::time::error::Elapsed) -> Self {
        Self::timeout()
    }
}
