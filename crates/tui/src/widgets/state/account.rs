//! Cached account state for the TUI.
//!
//! Balance and usage quota updates arrive on a dedicated channel separate from
//! the agent runtime. This struct keeps the latest values in one place so the
//! bottom bar can render them without touching individual `Option` fields.

use tact_protocol::biz::{BalanceInfo, UsageQuotaInfo};

/// Latest account / subscription state fetched from the active provider.
#[derive(Default, Clone)]
pub(crate) struct AccountState {
    /// DeepSeek / Moonshot account balance info.
    pub balance: Option<BalanceInfo>,
    /// Kimi Code subscription quota.
    pub quota: Option<UsageQuotaInfo>,
}

impl AccountState {
    /// Whether the state currently holds either balance or quota data.
    pub(crate) fn is_populated(&self) -> bool {
        self.balance.is_some() || self.quota.is_some()
    }

    /// Replace any previous state with a balance result.
    pub(crate) fn set_balance(&mut self, info: BalanceInfo) {
        self.balance = Some(info);
        self.quota = None;
    }

    /// Replace any previous state with a usage quota result.
    pub(crate) fn set_quota(&mut self, info: UsageQuotaInfo) {
        self.quota = Some(info);
        self.balance = None;
    }

    /// Clear cached account state, e.g. after an error or unsupported query.
    pub(crate) fn clear(&mut self) {
        self.balance = None;
        self.quota = None;
    }
}
