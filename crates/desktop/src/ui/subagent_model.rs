use tact_catalog::SettingId;

pub const SUBAGENT_MODEL_SETTING: SettingId = SettingId::SubagentModel;

pub fn apply_scope() -> &'static str {
    "Applies to the next spawned child"
}
