//! Shared HTTP transport for provider and account adapters.

use std::sync::Arc;
use std::time::Duration;

/// Shared [`reqwest`] client with the project's standard read timeout.
///
/// Wrapping the client in a newtype keeps connection-pool reuse explicit at
/// every call site without forcing each adapter to build its own client.
#[derive(Debug, Clone)]
pub struct SharedHttpClient(Arc<reqwest13::Client>);

impl SharedHttpClient {
    /// Build a client with a 120-second read timeout (matching the previous
    /// per-adapter defaults).
    pub fn new() -> Self {
        let client = reqwest13::Client::builder()
            .read_timeout(Duration::from_secs(120))
            .build()
            .expect("failed to build reqwest client");
        Self(Arc::new(client))
    }

    /// Borrow the underlying [`reqwest::Client`].
    pub fn inner(&self) -> &reqwest13::Client {
        &self.0
    }
}

impl Default for SharedHttpClient {
    fn default() -> Self {
        Self::new()
    }
}
