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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FailureView {
    pub message: Arc<str>,
    pub retryable: bool,
    pub retry_label: Option<Arc<str>>,
}

impl From<HostFailure> for FailureView {
    fn from(failure: HostFailure) -> Self {
        let message = match failure {
            HostFailure::Config(message) => format!("Configuration: {message}"),
            HostFailure::Provider(message) => format!("Provider: {message}"),
            HostFailure::Persistence(message) => format!("Persistence: {message}"),
            HostFailure::Bootstrap(message) => format!("Startup: {message}"),
            HostFailure::DriverPanicked(message) => format!("Runtime stopped: {message}"),
        };
        Self {
            message: Arc::from(message),
            retryable: false,
            retry_label: None,
        }
    }
}

impl FailureView {
    pub fn from_host(failure: HostFailure, locale: CatalogLocale) -> Self {
        let (prefix, message) = match failure {
            HostFailure::Config(message) => (
                match locale {
                    CatalogLocale::En => "Configuration",
                    CatalogLocale::Zh => "配置",
                },
                message,
            ),
            HostFailure::Provider(message) => (
                match locale {
                    CatalogLocale::En => "Provider",
                    CatalogLocale::Zh => "模型提供方",
                },
                message,
            ),
            HostFailure::Persistence(message) => (
                match locale {
                    CatalogLocale::En => "Persistence",
                    CatalogLocale::Zh => "持久化",
                },
                message,
            ),
            HostFailure::Bootstrap(message) => (
                match locale {
                    CatalogLocale::En => "Startup",
                    CatalogLocale::Zh => "启动",
                },
                message,
            ),
            HostFailure::DriverPanicked(message) => (
                match locale {
                    CatalogLocale::En => "Runtime stopped",
                    CatalogLocale::Zh => "运行时已停止",
                },
                message,
            ),
        };
        Self {
            message: Arc::from(format!("{prefix}: {message}")),
            retryable: false,
            retry_label: None,
        }
    }
}
