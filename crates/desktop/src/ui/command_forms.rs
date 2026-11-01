use tact_catalog::{CommandId, SettingId};
use tact_desktop_core::{CommandForm, SettingMode, form_for, plan_command, setting_mode};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormRoute {
    pub form: CommandForm,
    pub application: Option<SettingMode>,
    pub restart_required: bool,
}

pub fn route(id: &CommandId) -> FormRoute {
    let application = match id {
        CommandId::Setting(setting) => Some(setting_mode(*setting)),
        _ => plan_command(id).setting_mode,
    };
    FormRoute {
        form: form_for(id),
        application,
        restart_required: application == Some(SettingMode::StartupGlobal),
    }
}

pub fn values(setting: SettingId) -> Vec<(&'static str, &'static str)> {
    match setting {
        SettingId::ReasoningEffort => vec![
            ("Low", "low"),
            ("Medium", "medium"),
            ("High", "high"),
            ("Extra high", "xhigh"),
            ("Max", "max"),
        ],
        SettingId::PermissionMode => {
            vec![("Default", "default"), ("Plan", "plan"), ("Auto", "auto")]
        }
        SettingId::Theme => vec![("Light", "light"), ("Dark", "dark")],
        SettingId::Language => vec![("English", "en"), ("中文", "zh")],
        SettingId::Notifications
        | SettingId::HookOutput
        | SettingId::MicroCompact
        | SettingId::TokioConsole
        | SettingId::SkillBodyAutoInject => vec![("On", "true"), ("Off", "false")],
        _ => Vec::new(),
    }
}
