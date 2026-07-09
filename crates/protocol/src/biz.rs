//! Business-domain protocol types.
//!
//! Account balance and subscription quota structures returned by LLM provider
//! APIs (DeepSeek, Moonshot, Kimi Code). These are not agent-runtime messages
//! themselves, but they are carried inside [`AgentUpdate`](crate::agent::AgentUpdate)
//! and rendered by the TUI.

use serde::{Deserialize, Serialize};

/// A single currency entry in DeepSeek account balance info.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BalanceEntry {
    /// Currency type: CNY or USD
    pub currency: String,
    /// Total available balance (granted + topped up)
    pub total_balance: String,
    /// Unexpired granted balance
    pub granted_balance: String,
    /// Topped-up balance
    pub topped_up_balance: String,
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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageQuotaWindow {
    /// Short label, e.g. `week` or `5h`.
    pub label: String,
    pub limit: String,
    pub remaining: String,
    pub reset_time: Option<String>,
}

impl UsageQuotaWindow {
    /// Returns the percentage of quota already used.
    ///
    /// If the limit or remaining strings cannot be parsed, or if the limit is
    /// zero, this returns `None` so callers can fall back to raw text.
    pub fn usage_pct(&self) -> Option<f64> {
        let limit = self.limit.trim().parse::<f64>().ok()?;
        let remaining = self.remaining.trim().parse::<f64>().ok()?;
        if limit <= 0.0 {
            return None;
        }
        let used = (limit - remaining).max(0.0);
        Some((used / limit * 100.0).min(100.0))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageQuotaInfo {
    pub is_available: bool,
    pub windows: Vec<UsageQuotaWindow>,
    pub membership_level: Option<String>,
}
