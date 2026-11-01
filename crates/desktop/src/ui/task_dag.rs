use tact_desktop_core::TaskView;

use crate::i18n::{self, TextKey};

pub fn dependency_lines(tasks: &[TaskView], locale: tact_catalog::CatalogLocale) -> Vec<String> {
    tasks
        .iter()
        .map(|task| {
            let blocked_by = if task.blocked_by.is_empty() {
                i18n::text(locale, TextKey::Ready).to_string()
            } else {
                format!(
                    "{} {}",
                    i18n::text(locale, TextKey::BlockedBy),
                    task.blocked_by
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            };
            format!("#{} {} · {blocked_by}", task.id, task.subject)
        })
        .collect()
}
