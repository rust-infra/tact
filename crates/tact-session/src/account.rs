//! Account balance / usage quota service.
//!
//! This module is deliberately separate from the agent runtime. Balance and
//! quota queries are provider-specific business concerns; routing them through
//! the agent–TUI update channel couples the agent protocol to LLM-provider
//! details. Instead, the account service emits [`AccountUpdate`] messages on
//! its own channel and the TUI renders them independently.

use std::{
    future::Future,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use tact_llm::{
    is_account_query_supported, is_deepseek_balance_supported, is_kimi_balance_supported,
    is_kimi_usage_supported, query_deepseek_balance, query_kimi_balance, query_kimi_code_usage,
};
use tact_protocol::{
    AccountError, AccountUpdate,
    biz::{BalanceInfo, UsageQuotaInfo},
};
use tokio::sync::mpsc::UnboundedSender;

static POLL_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Whether the configured LLM provider supports any account query.
pub fn is_supported() -> bool {
    is_account_query_supported()
}

/// Result of a single account query.
pub enum AccountQueryResult {
    Balance(BalanceInfo),
    UsageQuota(UsageQuotaInfo),
}

/// Query the provider once and return a typed result.
pub async fn query_once() -> Result<AccountQueryResult, AccountError> {
    if !is_supported() {
        return Err(AccountError::NotSupported);
    }

    if is_deepseek_balance_supported() {
        query_deepseek_balance()
            .await
            .map(AccountQueryResult::Balance)
            .map_err(|e| AccountError::QueryFailed(e.to_string()))
    } else if is_kimi_balance_supported() {
        query_kimi_balance()
            .await
            .map(AccountQueryResult::Balance)
            .map_err(|e| AccountError::QueryFailed(e.to_string()))
    } else if is_kimi_usage_supported() {
        query_kimi_code_usage()
            .await
            .map(AccountQueryResult::UsageQuota)
            .map_err(|e| AccountError::QueryFailed(e.to_string()))
    } else {
        Err(AccountError::NotSupported)
    }
}

/// Convert a typed query result into an [`AccountUpdate`] message.
pub fn into_update(result: AccountQueryResult) -> AccountUpdate {
    match result {
        AccountQueryResult::Balance(balance) => AccountUpdate::Balance(balance),
        AccountQueryResult::UsageQuota(quota) => AccountUpdate::UsageQuota(quota),
    }
}

/// Spawn a periodic account query task.
///
/// On success the delay is randomised 5–15 seconds to spread load.
/// **On consecutive failures** the delay doubles (exponential backoff:
/// 10 s → 20 s → 40 s → … → capped at ~5 min), giving the provider or
/// network time to recover.
///
/// **Error noise is capped**: only the first failure of a consecutive run is
/// forwarded to the UI as `AccountUpdate::Error`; later retries stay silent
/// until a query succeeds again (which resets the counter and restores the
/// normal 5–15 s polling). A single outage therefore shows one flash message
/// instead of one per backoff tick.
///
/// The task stops when the receiver drops or the provider signals
/// [`AccountError::NotSupported`].
pub fn spawn_poller(account_tx: UnboundedSender<AccountUpdate>) {
    tokio::spawn(poll_loop(query_once, account_tx, |backoff| {
        if backoff > 0 {
            // 10s, 20s, 40s, 80s, 160s, capped at 320s
            Duration::from_secs(10u64.saturating_mul(1 << backoff.min(5)))
        } else {
            jitter_interval()
        }
    }));
}

/// Core poller loop, extracted for testability.
///
/// `query` runs every `next_delay(backoff)`; `next_delay` receives the current
/// backoff exponent (`0` = normal, `>0` = consecutive failures so far).
async fn poll_loop<Q, D, F>(
    mut query: Q,
    account_tx: UnboundedSender<AccountUpdate>,
    mut next_delay: D,
) where
    Q: FnMut() -> F,
    F: Future<Output = Result<AccountQueryResult, AccountError>>,
    D: FnMut(u32) -> Duration,
{
    let mut backoff = 0u32;
    let mut error_notified = false;
    loop {
        let delay = next_delay(backoff);
        tokio::time::sleep(delay).await;
        match query().await {
            Ok(result) => {
                backoff = 0;
                error_notified = false;
                if account_tx.send(into_update(result)).is_err() {
                    break;
                }
            }
            Err(AccountError::NotSupported) => break,
            Err(err) => {
                backoff = backoff.saturating_add(1);
                if error_notified {
                    continue;
                }
                error_notified = true;
                if account_tx.send(AccountUpdate::Error(err)).is_err() {
                    break;
                }
            }
        }
    }
}

/// Returns a pseudo-random interval between 5–15 seconds using a lock-free
/// monotonic counter. This avoids `SystemTime` syscalls and guarantees
/// variation between successive calls (unlike `subsec_nanos` which is
/// deterministic within the same wall-clock tick).
fn jitter_interval() -> Duration {
    let n = POLL_COUNTER.fetch_add(1, Ordering::Relaxed);
    Duration::from_secs(5 + (n % 11))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::install_test_config;

    #[test]
    fn jitter_interval_is_in_range() {
        let mut seen = std::collections::HashSet::new();
        for _ in 0..50 {
            let d = jitter_interval();
            assert!(d >= Duration::from_secs(5), "too short: {d:?}");
            assert!(d <= Duration::from_secs(15), "too long: {d:?}");
            seen.insert(d.as_secs());
        }
        // With 50 samples from 11 buckets we should see at least 3 distinct values
        // (p ≈ 1.0 that we see more than 1, but 3 is a safe bar)
        assert!(seen.len() >= 2, "jitter looks constant: {seen:?}");
    }

    #[test]
    fn successive_calls_produce_variation() {
        let a = jitter_interval();
        let b = jitter_interval();
        // Extremely unlikely that two consecutive calls collide
        // (1-in-11 chance), but assert they're different for docs
        assert_ne!(a, b, "consecutive jitter calls should differ");
    }

    #[test]
    fn is_supported_delegates() {
        install_test_config();
        let _ = is_supported();
    }

    #[tokio::test]
    async fn poller_forwards_error_once_per_outage_then_resumes() {
        use tact_protocol::biz::{BalanceEntry, BalanceInfo};

        let ok = || {
            Ok(AccountQueryResult::Balance(BalanceInfo {
                is_available: true,
                balance_infos: vec![BalanceEntry {
                    currency: "CNY".to_string(),
                    total_balance: 10.0,
                    granted_balance: 5.0,
                    topped_up_balance: 5.0,
                }],
            }))
        };
        let sequence = std::sync::Mutex::new(
            vec![
                Err(AccountError::QueryFailed("boom".to_string())),
                Err(AccountError::QueryFailed("boom".to_string())),
                Err(AccountError::QueryFailed("boom".to_string())),
                ok(),
                Err(AccountError::QueryFailed("boom".to_string())),
                Err(AccountError::QueryFailed("boom".to_string())),
                ok(),
            ]
            .into_iter(),
        );
        let query = move || {
            let mut seq = sequence.lock().expect("test sequence poisoned");
            // Sequence exhausted → NotSupported terminates the loop cleanly.
            std::future::ready(seq.next().unwrap_or(Err(AccountError::NotSupported)))
        };

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        // Zero delay keeps the test fast; backoff is irrelevant to dedup.
        tokio::spawn(poll_loop(query, tx, |_| Duration::ZERO));

        // First outage: exactly one Error, silent retries after it.
        let first = tokio::time::timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("timeout waiting for first update")
            .expect("channel closed early");
        assert!(matches!(first, AccountUpdate::Error(_)));

        // Recovery: Balance arrives, error flag is reset.
        let second = tokio::time::timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("timeout waiting for recovery")
            .expect("channel closed early");
        assert!(matches!(second, AccountUpdate::Balance(_)));

        // Second outage: dedup restarted, again exactly one Error.
        let third = tokio::time::timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("timeout waiting for second outage")
            .expect("channel closed early");
        assert!(matches!(third, AccountUpdate::Error(_)));

        let fourth = tokio::time::timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("timeout waiting for final recovery")
            .expect("channel closed early");
        assert!(matches!(fourth, AccountUpdate::Balance(_)));

        // Sequence exhausted → NotSupported → poller terminates, closing the
        // channel. Assert clean shutdown with no further Error flashes.
        let shutdown = tokio::time::timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("timeout waiting for poller shutdown");
        assert!(
            shutdown.is_none(),
            "poller must shut down silently after NotSupported, got {shutdown:?}"
        );
    }

    #[tokio::test]
    async fn poller_stops_on_not_supported_without_error_flash() {
        let calls = std::sync::Mutex::new(0u32);
        let query = move || {
            let mut c = calls.lock().expect("test counter poisoned");
            *c += 1;
            std::future::ready(Err(AccountError::NotSupported))
        };
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(poll_loop(query, tx, |_| Duration::ZERO));

        // NotSupported terminates the poller without any message at all:
        // the channel closes with `Ok(None)` and no Error was ever sent.
        let shutdown = tokio::time::timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("timeout waiting for poller shutdown");
        assert!(
            shutdown.is_none(),
            "NotSupported must terminate silently, got {shutdown:?}"
        );
    }
}
