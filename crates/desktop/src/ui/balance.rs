use std::sync::Arc;

use tact_desktop_core::AccountView;

use crate::i18n::{self, TextKey};

pub fn balance_markdown(
    account: Option<&AccountView>,
    locale: tact_catalog::CatalogLocale,
) -> String {
    match account {
        Some(AccountView::Balance { currency, total }) => {
            format!(
                "# {}\n\n**{total} {currency}**",
                i18n::text(locale, TextKey::Balance)
            )
        }
        Some(AccountView::UsageQuota {
            plan,
            used,
            limit,
            resets_at,
        }) => format!(
            "# {}\n\n- {}: {}\n- {}: {}\n- {}: {}\n- {}: {}",
            i18n::text(locale, TextKey::Balance),
            i18n::text(locale, TextKey::Plan),
            plan.as_deref().unwrap_or("—"),
            i18n::text(locale, TextKey::Used),
            used.map(|value| value.to_string())
                .unwrap_or_else(|| "—".into()),
            i18n::text(locale, TextKey::Limit),
            limit
                .map(|value| value.to_string())
                .unwrap_or_else(|| "—".into()),
            i18n::text(locale, TextKey::Resets),
            resets_at.as_deref().unwrap_or("—")
        ),
        Some(AccountView::Error(error)) => format!(
            "# {}\n\n{error}",
            i18n::text(locale, TextKey::BalanceUnavailable)
        ),
        None => format!(
            "# {}\n\n{}",
            i18n::text(locale, TextKey::Balance),
            i18n::text(locale, TextKey::QueryingAccount)
        ),
    }
}

pub fn currency_symbol(account: &AccountView) -> Option<Arc<str>> {
    match account {
        AccountView::Balance { currency, .. } => Some(currency.clone()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use gpui::TestAppContext;

    use super::*;

    #[gpui::test]
    fn balance_reader_renders_account_snapshot(_cx: &mut TestAppContext) {
        let account = AccountView::Balance {
            currency: Arc::from("USD"),
            total: "12.50".into(),
        };
        assert!(
            balance_markdown(Some(&account), tact_catalog::CatalogLocale::En).contains("12.50 USD")
        );
    }
}
