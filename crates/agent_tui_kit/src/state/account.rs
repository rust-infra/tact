//! Cached account state for the bottom bar.
//!
//! Balance and usage quota updates arrive on a dedicated channel separate from
//! the agent runtime. This struct keeps the latest values in one place so the
//! bottom bar can render them without touching individual `Option` fields.

use tact_protocol::biz::{BalanceInfo, UsageQuotaInfo};

/// Latest account / subscription state fetched from the active provider.
#[derive(Default, Clone)]
pub struct AccountState {
    /// DeepSeek / Moonshot account balance info.
    pub balance: Option<BalanceInfo>,
    /// Kimi Code subscription quota.
    pub quota: Option<UsageQuotaInfo>,
}

impl AccountState {
    /// Replace any previous state with a balance result.
    pub fn set_balance(&mut self, info: BalanceInfo) {
        self.balance = Some(info);
        self.quota = None;
    }

    /// Replace any previous state with a usage quota result.
    pub fn set_quota(&mut self, info: UsageQuotaInfo) {
        self.quota = Some(info);
        self.balance = None;
    }

    /// Clear cached account state, e.g. when the provider permanently does not
    /// support account queries.
    pub fn clear(&mut self) {
        self.balance = None;
        self.quota = None;
    }
}
