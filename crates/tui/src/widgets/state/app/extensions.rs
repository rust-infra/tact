//! Tact-specific extension channels: account (balance/quota) and plugin events.
//!
//! These live on **separate channels** from the agent runtime so that
//! provider-specific account state and the plugin system do not leak into the
//! agent protocol (`AgentUpdate`). In the kit (Phase 3/4) this module becomes
//! the `BridgeExtension` implementation on the Tact app layer.

use ratatui::{
    style::Style,
    text::{Line, Span},
};
use tact::plugin::{PluginEvent, PluginOperation, PluginResult};
use tact_protocol::{AccountError, AccountUpdate};

use crate::{
    render::render_md::format_table_lines,
    widgets::state::{App, InputMode, LogItemKind, SystemMsgStyle},
};

pub(crate) const MAX_PLUGIN_FAILURE_DETAIL_CHARS: usize = 512;

fn sanitize_plugin_failure_detail(detail: &str) -> String {
    let mut sanitized: String = detail
        .chars()
        .map(|ch| if ch.is_control() { ' ' } else { ch })
        .take(MAX_PLUGIN_FAILURE_DETAIL_CHARS + 1)
        .collect();

    if sanitized.chars().count() > MAX_PLUGIN_FAILURE_DETAIL_CHARS {
        sanitized = sanitized
            .chars()
            .take(MAX_PLUGIN_FAILURE_DETAIL_CHARS)
            .collect();
        sanitized.push_str("...");
    }

    sanitized
}

fn replace_two(template: &str, first: &str, second: &str) -> String {
    template.replacen("{}", first, 1).replacen("{}", second, 1)
}

fn replace_three(template: &str, first: &str, second: &str, third: &str) -> String {
    template
        .replacen("{}", first, 1)
        .replacen("{}", second, 1)
        .replacen("{}", third, 1)
}

fn format_plugin_result(messages: &crate::i18n::Messages, result: &PluginResult) -> String {
    match result {
        PluginResult::Installed {
            plugin,
            marketplace,
        } => replace_two(messages.plugin_installed_tmpl, plugin, marketplace),
        PluginResult::Uninstalled { plugin } => {
            messages.plugin_uninstalled_tmpl.replace("{}", plugin)
        }
        PluginResult::Updated {
            plugin,
            marketplace,
            revision,
        } => replace_three(
            messages.plugin_updated_tmpl,
            plugin,
            marketplace,
            revision,
        ),
        PluginResult::UpToDate {
            plugin,
            marketplace,
            revision,
        } => replace_three(
            messages.plugin_up_to_date_tmpl,
            plugin,
            marketplace,
            revision,
        ),
        // Rendered as a titled table by `App::show_plugin_list`; plain fallback only.
        PluginResult::ListedInstalled { .. } => messages.plugin_list_empty.to_owned(),
        PluginResult::Reloaded { count } => messages
            .plugin_reloaded_tmpl
            .replace("{}", &count.to_string()),
        PluginResult::MarketplaceAdded { marketplace } => {
            messages.marketplace_added_tmpl.replace("{}", marketplace)
        }
        // Rendered as a titled table by `App::show_marketplace_list`; plain fallback only.
        PluginResult::ListedMarketplaces { .. } => messages.marketplace_list_empty.to_owned(),
        PluginResult::MarketplaceUpdated { marketplace, count } => replace_two(
            messages.marketplace_updated_tmpl,
            marketplace,
            &count.to_string(),
        ),
        PluginResult::MarketplaceRemoved { marketplace } => {
            messages.marketplace_removed_tmpl.replace("{}", marketplace)
        }
    }
}

fn plugin_operation_label(
    messages: &crate::i18n::Messages,
    operation: &PluginOperation,
) -> &'static str {
    match operation {
        PluginOperation::Install { .. } => messages.plugin_operation_install,
        PluginOperation::Uninstall { .. } => messages.plugin_operation_uninstall,
        PluginOperation::Update { .. } => messages.plugin_operation_update,
        PluginOperation::List => messages.plugin_operation_list,
        PluginOperation::Reload => messages.plugin_operation_reload,
        PluginOperation::MarketplaceAdd => messages.plugin_operation_marketplace_add,
        PluginOperation::MarketplaceList => messages.plugin_operation_marketplace_list,
        PluginOperation::MarketplaceUpdate { .. } => messages.plugin_operation_marketplace_update,
        PluginOperation::MarketplaceRemove { .. } => messages.plugin_operation_marketplace_remove,
    }
}

impl App {
    /// Provider-specific account state (balance / usage quota).
    ///
    /// These updates live on a separate channel from the agent runtime so that
    /// provider-specific account state does not leak into the agent protocol.
    pub(crate) fn handle_account_update(&mut self, update: AccountUpdate) {
        self.dirty = true;
        match update {
            AccountUpdate::Balance(info) => self.account.set_balance(info),
            AccountUpdate::UsageQuota(info) => self.account.set_quota(info),
            AccountUpdate::Error(err) => {
                // Only clear on permanent unsupported; keep last-known values
                // across transient poll / network failures.
                if matches!(err, AccountError::NotSupported) {
                    self.account.clear();
                }
                self.flash_msg = Some((err.to_string(), std::time::Instant::now()));
            }
        }
    }

    /// Renders `/plugin list` as a titled table block (same style as `/skills`).
    fn show_plugin_list(&mut self, plugins: &[tact::plugin::InstalledPlugin]) {
        self.add_new_line();

        let msgs = self.msgs();
        let title = msgs
            .plugin_list_title_tmpl
            .replace("{}", &plugins.len().to_string());
        self.append_msg(
            Line::from(Span::styled(
                title.clone(),
                Style::default().fg(self.theme.accent),
            )),
            title,
            LogItemKind::SystemPlain(SystemMsgStyle::Accent),
        );
        self.add_new_line();

        if plugins.is_empty() {
            let empty = msgs.plugin_list_empty;
            self.append_msg(
                Line::from(Span::styled(empty, Style::default().fg(self.theme.fg))),
                empty.to_string(),
                LogItemKind::SystemPlain(SystemMsgStyle::Default),
            );
        } else {
            let mut rows = vec![
                msgs.plugin_list_header.to_string(),
                "|---|---|---|".to_string(),
            ];
            rows.extend(plugins.iter().map(|plugin| {
                format!(
                    "| {} | {} | {} |",
                    plugin.id, plugin.marketplace, plugin.skill_count
                )
            }));
            let (styled, raw) =
                format_table_lines(&rows, &self.theme, Some(self.table_layout_width()));
            self.extend_msgs(
                styled,
                raw,
                LogItemKind::SystemPlain(SystemMsgStyle::Default),
            );
        }

        self.add_new_line();

        if self.input_mode == InputMode::Insert || self.input_mode == InputMode::Normal {
            self.scroll_log_to_bottom();
        }
    }

    /// Renders `/plugin marketplace list` as a titled table (one row per marketplace).
    ///
    /// Must not go through [`App::add_system_message`]: a single-newline list
    /// would be Markdown-soft-broken into one crowded line.
    fn show_marketplace_list(&mut self, marketplaces: &[tact::plugin::MarketplaceRecord]) {
        self.add_new_line();

        let msgs = self.msgs();
        let title = msgs
            .marketplace_list_title_tmpl
            .replace("{}", &marketplaces.len().to_string());
        self.append_msg(
            Line::from(Span::styled(
                title.clone(),
                Style::default().fg(self.theme.accent),
            )),
            title,
            LogItemKind::SystemPlain(SystemMsgStyle::Accent),
        );
        self.add_new_line();

        if marketplaces.is_empty() {
            let empty = msgs.marketplace_list_empty;
            self.append_msg(
                Line::from(Span::styled(empty, Style::default().fg(self.theme.fg))),
                empty.to_string(),
                LogItemKind::SystemPlain(SystemMsgStyle::Default),
            );
        } else {
            let mut rows = vec![
                msgs.marketplace_list_header.to_string(),
                "|---|---|".to_string(),
            ];
            rows.extend(marketplaces.iter().map(|marketplace| {
                format!(
                    "| {} | {} |",
                    marketplace.name,
                    marketplace.source.git_url()
                )
            }));
            let (styled, raw) =
                format_table_lines(&rows, &self.theme, Some(self.table_layout_width()));
            self.extend_msgs(
                styled,
                raw,
                LogItemKind::SystemPlain(SystemMsgStyle::Default),
            );
        }

        self.add_new_line();

        if self.input_mode == InputMode::Insert || self.input_mode == InputMode::Normal {
            self.scroll_log_to_bottom();
        }
    }

    /// Displays a completed plugin operation from the isolated worker.
    pub(crate) fn handle_plugin_event(&mut self, event: PluginEvent) {
        self.dirty = true;
        match event {
            PluginEvent::Succeeded {
                result,
                refresh_skills,
            } => {
                match &result {
                    PluginResult::ListedInstalled { plugins } => self.show_plugin_list(plugins),
                    PluginResult::ListedMarketplaces { marketplaces } => {
                        self.show_marketplace_list(marketplaces)
                    }
                    _ => self.add_system_message(format_plugin_result(&self.msgs(), &result)),
                }
                if refresh_skills && let Err(error) = crate::handlers::refresh_skills(self) {
                    self.add_system_message(
                        self.msgs().plugin_reload_failed_tmpl.replace("{}", &error),
                    );
                }
            }
            PluginEvent::Failed { operation, detail } => {
                let detail = sanitize_plugin_failure_detail(&detail);
                self.add_system_message(replace_two(
                    self.msgs().plugin_operation_failed_tmpl,
                    plugin_operation_label(&self.msgs(), &operation),
                    &detail,
                ));
            }
        }
    }
}
