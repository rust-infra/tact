mod install;
mod marketplace;
mod model;
mod store;

use anyhow::{Context, Result};
pub use install::*;
pub use marketplace::*;
pub use model::*;
pub use store::*;
use tokio::{
    sync::mpsc::{UnboundedReceiver, UnboundedSender},
    task::JoinHandle,
};

use crate::consts::PluginHome;

/// A plugin operation requested by the interactive UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginRequest {
    Install { plugin: String, marketplace: String },
    List,
    Reload,
    MarketplaceAdd { source: String },
    MarketplaceList,
    MarketplaceUpdate { name: String },
    MarketplaceRemove { name: String },
}

/// The kind of operation associated with a plugin-worker failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginOperation {
    Install { plugin: String, marketplace: String },
    List,
    Reload,
    MarketplaceAdd,
    MarketplaceList,
    MarketplaceUpdate { marketplace: String },
    MarketplaceRemove { marketplace: String },
}

impl From<&PluginRequest> for PluginOperation {
    fn from(request: &PluginRequest) -> Self {
        match request {
            PluginRequest::Install {
                plugin,
                marketplace,
            } => Self::Install {
                plugin: plugin.clone(),
                marketplace: marketplace.clone(),
            },
            PluginRequest::List => Self::List,
            PluginRequest::Reload => Self::Reload,
            PluginRequest::MarketplaceAdd { .. } => Self::MarketplaceAdd,
            PluginRequest::MarketplaceList => Self::MarketplaceList,
            PluginRequest::MarketplaceUpdate { name } => Self::MarketplaceUpdate {
                marketplace: name.clone(),
            },
            PluginRequest::MarketplaceRemove { name } => Self::MarketplaceRemove {
                marketplace: name.clone(),
            },
        }
    }
}

/// Structured data produced by a successful plugin operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginResult {
    Installed {
        plugin: String,
        marketplace: String,
    },
    ListedInstalled {
        plugins: Vec<InstalledPlugin>,
    },
    Reloaded {
        count: usize,
    },
    MarketplaceAdded {
        marketplace: String,
    },
    ListedMarketplaces {
        marketplaces: Vec<MarketplaceRecord>,
    },
    MarketplaceUpdated {
        marketplace: String,
        count: usize,
    },
    MarketplaceRemoved {
        marketplace: String,
    },
}

/// A structured result produced by the plugin worker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginEvent {
    Succeeded {
        result: PluginResult,
        refresh_skills: bool,
    },
    Failed {
        operation: PluginOperation,
        detail: String,
    },
}

/// Starts the plugin worker. Filesystem and Git operations run on Tokio's
/// blocking pool so interactive input remains responsive.
#[must_use]
pub fn spawn_worker(
    home: PluginHome,
    mut request_rx: UnboundedReceiver<PluginRequest>,
    event_tx: UnboundedSender<PluginEvent>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(request) = request_rx.recv().await {
            let refresh_skills = matches!(
                &request,
                PluginRequest::Install { .. } | PluginRequest::Reload
            );
            let operation = PluginOperation::from(&request);
            let worker_home = home.clone();
            let event =
                match tokio::task::spawn_blocking(move || execute_request(worker_home, request))
                    .await
                {
                    Ok(Ok(result)) => PluginEvent::Succeeded {
                        result,
                        refresh_skills,
                    },
                    Ok(Err(error)) => PluginEvent::Failed {
                        operation,
                        detail: error.to_string(),
                    },
                    Err(error) => PluginEvent::Failed {
                        operation,
                        detail: error.to_string(),
                    },
                };
            if event_tx.send(event).is_err() {
                break;
            }
        }
    })
}

/// Starts a responder for environments where the plugin home cannot be resolved.
///
/// Keeping the request channel alive lets callers surface a clear operation error
/// instead of silently discarding plugin requests.
#[must_use]
pub fn spawn_unavailable_worker(
    mut request_rx: UnboundedReceiver<PluginRequest>,
    event_tx: UnboundedSender<PluginEvent>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(request) = request_rx.recv().await {
            if event_tx
                .send(PluginEvent::Failed {
                    operation: PluginOperation::from(&request),
                    detail: "HOME is not set".into(),
                })
                .is_err()
            {
                break;
            }
        }
    })
}

pub fn execute_request(home: PluginHome, request: PluginRequest) -> Result<PluginResult> {
    let marketplaces = MarketplaceService::new(home.clone());
    match request {
        PluginRequest::Install {
            plugin,
            marketplace,
        } => {
            let installed = PluginInstaller::new(home).install(&plugin, &marketplace)?;
            Ok(PluginResult::Installed {
                plugin: installed.id,
                marketplace: installed.marketplace,
            })
        }
        PluginRequest::List => {
            let plugins = PluginInstaller::new(home).list()?;
            Ok(PluginResult::ListedInstalled { plugins })
        }
        PluginRequest::Reload => {
            let plugins = PluginInstaller::new(home).list()?;
            Ok(PluginResult::Reloaded {
                count: plugins.len(),
            })
        }
        PluginRequest::MarketplaceAdd { source } => {
            let marketplace = block_on_async(
                marketplaces.add_catalog_source(MarketplaceSource::parse(&source)?),
            )?;
            Ok(PluginResult::MarketplaceAdded { marketplace })
        }
        PluginRequest::MarketplaceList => {
            let state = PluginStore::new(home).load_marketplaces()?;
            Ok(PluginResult::ListedMarketplaces {
                marketplaces: state.iter().map(|(_, record)| record.clone()).collect(),
            })
        }
        PluginRequest::MarketplaceUpdate { name } => {
            let catalog = block_on_async(marketplaces.update_marketplace(&name))
                .with_context(|| format!("failed to update marketplace {name}"))?;
            Ok(PluginResult::MarketplaceUpdated {
                marketplace: name,
                count: catalog.plugins.len(),
            })
        }
        PluginRequest::MarketplaceRemove { name } => {
            marketplaces.remove_source(&name)?;
            Ok(PluginResult::MarketplaceRemoved { marketplace: name })
        }
    }
}

/// Runs an async future from a synchronous context, handling both
/// when a tokio runtime is active and when it isn't.
pub(crate) fn block_on_async<F: std::future::Future<Output = T>, T>(future: F) -> T {
    if tokio::runtime::Handle::try_current().is_ok() {
        // Already inside a tokio runtime: temporarily leave it with block_in_place.
        tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(future))
    } else {
        // Not inside a tokio runtime: create a fresh one.
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(future)
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;
    use tokio::sync::mpsc::unbounded_channel;

    use super::{PluginEvent, PluginHome, PluginRequest, spawn_unavailable_worker, spawn_worker};

    async fn run_request(home: PluginHome, request: PluginRequest) -> PluginEvent {
        let (request_tx, request_rx) = unbounded_channel();
        let (event_tx, mut event_rx) = unbounded_channel();
        let worker = spawn_worker(home, request_rx, event_tx);
        request_tx.send(request).unwrap();
        drop(request_tx);
        let event = event_rx
            .recv()
            .await
            .expect("worker should return an event");
        worker.await.expect("worker task should not panic");
        event
    }

    #[tokio::test]
    async fn worker_returns_failed_event_without_mutating_store() {
        let temporary_home = tempdir().unwrap();
        let home = PluginHome::from_home(temporary_home.path());

        let event = run_request(
            home.clone(),
            PluginRequest::Install {
                plugin: "missing".into(),
                marketplace: "fixture".into(),
            },
        )
        .await;

        assert!(matches!(event, PluginEvent::Failed { .. }));
        assert!(!home.root.join("installed.json").exists());
        assert!(!home.root.join("marketplaces.json").exists());
        assert!(
            fs::read_dir(temporary_home.path())
                .unwrap()
                .next()
                .is_none()
        );
    }

    #[tokio::test]
    async fn worker_reports_invalid_marketplace_add_source() {
        let temporary_home = tempdir().unwrap();
        let home = PluginHome::from_home(temporary_home.path());

        let event = run_request(
            home,
            PluginRequest::MarketplaceAdd {
                source: "not-a-marketplace-source".into(),
            },
        )
        .await;

        assert!(matches!(event, PluginEvent::Failed { .. }));
    }

    #[tokio::test]
    async fn worker_reports_plugin_marketplace_unavailable_without_a_home_directory() {
        let (request_tx, request_rx) = unbounded_channel();
        let (event_tx, mut event_rx) = unbounded_channel();
        let worker = spawn_unavailable_worker(request_rx, event_tx);

        request_tx.send(PluginRequest::List).unwrap();
        drop(request_tx);

        assert!(matches!(
            event_rx.recv().await,
            Some(PluginEvent::Failed { .. })
        ));
        worker.await.expect("worker task should not panic");
    }
}
