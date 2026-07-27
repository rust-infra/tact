//! Business-domain protocol types.
//!
//! Account balance and subscription quota structures returned by LLM provider
//! APIs (DeepSeek, Moonshot, Kimi Code). These are not agent-runtime messages
//! themselves, but they are carried inside [`AgentUpdate`](crate::agent::AgentUpdate)
//! and rendered by the TUI.

use serde::{Deserialize, Serialize};

/// A single currency entry in DeepSeek account balance info.
///
/// Amounts are parsed into numbers at the provider API boundary; rendering
/// (decimal places, currency symbols) is up to the UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BalanceEntry {
    /// Currency type: CNY or USD
    pub currency: String,
    /// Total available balance (granted + topped up)
    pub total_balance: f64,
    /// Unexpired granted balance
    pub granted_balance: f64,
    /// Topped-up balance
    pub topped_up_balance: f64,
}

/// DeepSeek / Moonshot account balance query result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BalanceInfo {
    /// Whether the account has available balance
    pub is_available: bool,
    /// Per-currency balance details
    pub balance_infos: Vec<BalanceEntry>,
}

/// A single quota window from Kimi Code `GET /usages`.
///
/// `limit` / `remaining` are `None` when the provider reports a non-numeric
/// value (e.g. unlimited) — callers should treat `None` as "no cap".
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageQuotaWindow {
    /// Short label, e.g. `week` or `5h`.
    pub label: String,
    pub limit: Option<f64>,
    pub remaining: Option<f64>,
    pub reset_time: Option<String>,
}

impl UsageQuotaWindow {
    /// Returns the percentage of quota already used.
    ///
    /// Returns `None` when the window has no numeric limit/remaining or the
    /// limit is zero, so callers can fall back to a textual display.
    pub fn usage_pct(&self) -> Option<f64> {
        let limit = self.limit?;
        let remaining = self.remaining?;
        if limit <= 0.0 {
            return None;
        }
        let used = (limit - remaining).max(0.0);
        Some((used / limit * 100.0).min(100.0))
    }

    /// Whether this window still has quota left.
    ///
    /// A window without a numeric `remaining` value is treated as available.
    pub fn has_remaining(&self) -> bool {
        self.remaining.is_none_or(|v| v > 0.0)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageQuotaInfo {
    pub is_available: bool,
    pub windows: Vec<UsageQuotaWindow>,
    pub membership_level: Option<String>,
}

/// Account-query error, separate from [`AgentErrorKind`](crate::agent::AgentErrorKind)
/// so that balance failures do not have to flow through the agent runtime.
#[derive(Debug, Clone)]
pub enum AccountError {
    /// The active provider does not support balance / usage queries.
    NotSupported,
    /// Network or API error while fetching account information.
    QueryFailed(String),
}

impl std::fmt::Display for AccountError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AccountError::NotSupported => {
                f.write_str("Account query not supported for this provider")
            }
            AccountError::QueryFailed(err) => write!(f, "Account query failed: {err}"),
        }
    }
}

impl std::error::Error for AccountError {}

/// Update messages produced by the account service (balance / usage quota).
///
/// These travel on their own channel and are handled by the TUI independently
/// of the agent runtime.
#[derive(Debug, Clone)]
pub enum AccountUpdate {
    /// DeepSeek / Moonshot account balance info.
    Balance(BalanceInfo),
    /// Kimi Code subscription quota.
    UsageQuota(UsageQuotaInfo),
    /// Account query failed.
    Error(AccountError),
}
