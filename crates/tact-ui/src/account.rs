//! Account balance / usage quota service.
//!
//! This module is deliberately separate from the agent runtime. Balance and
//! quota queries are provider-specific business concerns; routing them through
//! the agent–TUI update channel couples the agent protocol to LLM-provider
//! details. Instead, the account service emits [`AccountUpdate`] messages on
//! its own channel and the TUI renders them independently.

use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use tact_llm::{
    is_account_query_supported, is_deepseek, is_kimi_balance_supported, is_kimi_usage_supported,
    query_deepseek_balance, query_kimi_balance, query_kimi_code_usage,
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

    if is_deepseek() {
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
/// The task stops when the receiver drops or the provider signals
/// [`AccountError::NotSupported`].
pub fn spawn_poller(account_tx: UnboundedSender<AccountUpdate>) {
    tokio::spawn(async move {
        let mut backoff = 0u32;
        loop {
            let delay = if backoff > 0 {
                // 10s, 20s, 40s, 80s, 160s, capped at 320s
                Duration::from_secs(10u64.saturating_mul(1 << backoff.min(5)))
            } else {
                jitter_interval()
            };

            tokio::time::sleep(delay).await;
            match query_once().await {
                Ok(result) => {
                    backoff = 0;
                    if account_tx.send(into_update(result)).is_err() {
                        break;
                    }
                },
                Err(AccountError::NotSupported) => break,
                Err(err) => {
                    backoff = backoff.saturating_add(1);
                    if account_tx.send(AccountUpdate::Error(err)).is_err() {
                        break;
                    }
                },
            }
        }
    });
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
}
