//! Cancellation and deadline propagation for Kernel invocations.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};

use tact_protocol::RequestId;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

/// Creates child cancellation tokens for independent requests and can cancel
/// all requests when the Runtime shuts down.
#[derive(Clone)]
pub struct CancellationService {
    root: CancellationToken,
    requests: Arc<Mutex<HashMap<RequestId, CancellationToken>>>,
}

impl CancellationService {
    #[must_use]
    pub fn new() -> Self {
        Self {
            root: CancellationToken::new(),
            requests: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    #[must_use]
    pub fn from_token(root: CancellationToken) -> Self {
        Self {
            root,
            requests: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    #[must_use]
    pub fn token(&self) -> CancellationToken {
        self.root.clone()
    }

    #[must_use]
    pub fn child(&self) -> CancellationToken {
        self.root.child_token()
    }

    /// Creates and tracks a child token for a protocol request.
    #[must_use]
    pub fn child_for(&self, request_id: RequestId) -> CancellationToken {
        let token = self.child();
        if let Ok(mut requests) = self.requests.lock() {
            requests.insert(request_id, token.clone());
        }
        token
    }

    /// Cancels one tracked request. Returns `false` when it is not in flight.
    pub fn cancel_request(&self, request_id: &RequestId) -> bool {
        self.requests
            .lock()
            .ok()
            .and_then(|requests| requests.get(request_id).cloned())
            .map(|token| {
                token.cancel();
                true
            })
            .unwrap_or(false)
    }

    /// Removes a completed request from the tracking table.
    pub fn forget_request(&self, request_id: &RequestId) {
        if let Ok(mut requests) = self.requests.lock() {
            requests.remove(request_id);
        }
    }

    pub fn cancel(&self) {
        self.root.cancel();
    }

    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.root.is_cancelled()
    }

    /// Returns a deadline relative to now. A zero duration is already expired.
    #[must_use]
    pub fn deadline_after(&self, timeout: Duration) -> Instant {
        Instant::now() + timeout
    }
}

impl Default for CancellationService {
    fn default() -> Self {
        Self::new()
    }
}
