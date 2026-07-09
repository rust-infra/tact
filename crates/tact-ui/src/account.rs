//! Account balance / usage quota service.
//!
//! This module is deliberately separate from the agent runtime. Balance and
//! quota queries are provider-specific business concerns; routing them through
//! the agent–TUI update channel couples the agent protocol to LLM-provider
//! details. Instead, the account service emits [`AccountUpdate`] messages on
//! its own channel and the TUI renders them independently.

use std::time::Duration;

use tact_llm::{
    is_account_query_supported, is_deepseek, is_kimi_balance_supported, is_kimi_usage_supported,
    query_deepseek_balance, query_kimi_balance, query_kimi_code_usage,
};
use tact_protocol::{
    AccountError, AccountUpdate,
    biz::{BalanceInfo, UsageQuotaInfo},
};
use tokio::sync::mpsc::UnboundedSender;

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
/// The interval is randomized between 5–15 seconds to avoid provider
/// rate-limiting. The task stops automatically when the receiver drops.
pub fn spawn_poller(account_tx: UnboundedSender<AccountUpdate>) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(random_interval()).await;
            match query_once().await {
                Ok(result) => {
                    if account_tx.send(into_update(result)).is_err() {
                        break;
                    }
                }
                Err(AccountError::NotSupported) => break,
                Err(err) => {
                    if account_tx.send(AccountUpdate::Error(err)).is_err() {
                        break;
                    }
                }
            }
        }
    });
}

fn random_interval() -> Duration {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    Duration::from_secs(5 + (nanos % 11) as u64)
}
