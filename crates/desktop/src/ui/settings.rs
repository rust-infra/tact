use tact_catalog::SettingId;
use tact_desktop_core::SettingMode;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SettingRow {
    pub id: SettingId,
    pub mode: SettingMode,
}

pub fn label(row: SettingRow) -> &'static str {
    match (row.id, row.mode) {
        (SettingId::SubagentModel, _) => "Next child",
        (_, SettingMode::Live) => "Live",
        (_, SettingMode::StartupGlobal) => "Requires restart",
    }
}

pub fn rows(query: &str) -> impl Iterator<Item = SettingRow> + '_ {
    let query = query.to_ascii_lowercase();
    SettingId::ALL
        .iter()
        .copied()
        .map(|id| SettingRow {
            id,
            mode: tact_desktop_core::setting_mode(id),
        })
        .filter(move |row| {
            query.is_empty() || row.id.as_str().to_ascii_lowercase().contains(&query)
        })
}
