use std::sync::Arc;

use tact_catalog::CatalogLocale;
use tact_host::{HostCloseError, HostFailure};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DesktopStartError {
    Config(String),
    Runtime(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DesktopShutdownError {
    Host(HostCloseError),
    Runtime(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DesktopClosed;

/// Machine-readable failure category.
///
/// The banner keeps a localized message, but the shell also needs to know
/// whether a failure is a locked session, a dead driver, or a store problem so
/// it can offer the right recovery action instead of a generic retry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FailureKind {
    Startup,
    SessionLocked,
    SessionOpen,
    DriverClosed,
    RuntimeDisconnected,
    Store,
    GateUnavailable,
    #[default]
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FailureView {
    pub kind: FailureKind,
    pub message: Arc<str>,
    pub retryable: bool,
    pub retry_label: Option<Arc<str>>,
}

impl From<HostFailure> for FailureView {
    fn from(failure: HostFailure) -> Self {
        let (kind, message) = match failure {
            HostFailure::Config(message) => {
                (FailureKind::Startup, format!("Configuration: {message}"))
            }
            HostFailure::Provider(message) => (FailureKind::Other, format!("Provider: {message}")),
            HostFailure::Persistence(message) => {
                (FailureKind::Store, format!("Persistence: {message}"))
            }
            HostFailure::Bootstrap(message) => {
                (FailureKind::Startup, format!("Startup: {message}"))
            }
            HostFailure::DriverPanicked(message) => (
                FailureKind::DriverClosed,
                format!("Runtime stopped: {message}"),
            ),
        };
        Self {
            kind,
            message: Arc::from(message),
            retryable: false,
            retry_label: None,
        }
    }
}

impl FailureView {
    pub fn from_host(failure: HostFailure, locale: CatalogLocale) -> Self {
        let (kind, prefix, message) = match failure {
            HostFailure::Config(message) => (
                FailureKind::Startup,
                match locale {
                    CatalogLocale::En => "Configuration",
                    CatalogLocale::Zh => "配置",
                },
                message,
            ),
            HostFailure::Provider(message) => (
                FailureKind::Other,
                match locale {
                    CatalogLocale::En => "Provider",
                    CatalogLocale::Zh => "模型提供方",
                },
                message,
            ),
            HostFailure::Persistence(message) => (
                FailureKind::Store,
                match locale {
                    CatalogLocale::En => "Persistence",
                    CatalogLocale::Zh => "持久化",
                },
                message,
            ),
            HostFailure::Bootstrap(message) => (
                FailureKind::Startup,
                match locale {
                    CatalogLocale::En => "Startup",
                    CatalogLocale::Zh => "启动",
                },
                message,
            ),
            HostFailure::DriverPanicked(message) => (
                FailureKind::DriverClosed,
                match locale {
                    CatalogLocale::En => "Runtime stopped",
                    CatalogLocale::Zh => "运行时已停止",
                },
                message,
            ),
        };
        Self {
            kind,
            message: Arc::from(format!("{prefix}: {message}")),
            retryable: false,
            retry_label: None,
        }
    }
}
