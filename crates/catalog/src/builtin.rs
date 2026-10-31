use strum::{EnumIter, EnumString, IntoStaticStr};

use crate::LocalizedText;

/// A built-in Tact command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EnumIter, EnumString, IntoStaticStr)]
pub enum BuiltinCommand {
    #[strum(serialize = "theme")]
    Theme,
    #[strum(serialize = "model")]
    Model,
    #[strum(serialize = "model-subagent")]
    ModelSubagent,
    #[strum(serialize = "permission")]
    Permission,
    #[strum(serialize = "view-system-prompt")]
    ViewSystemPrompt,
    #[strum(serialize = "save")]
    Save,
    #[strum(serialize = "compact")]
    Compact,
    #[strum(serialize = "cancel")]
    Cancel,
    #[strum(serialize = "subagent_cancel")]
    SubagentCancel,
    #[strum(serialize = "quit")]
    Quit,
    #[strum(serialize = "help")]
    Help,
    #[strum(serialize = "history")]
    History,
    #[strum(serialize = "skill")]
    Skill,
    #[strum(serialize = "plugin")]
    Plugin,
    #[strum(serialize = "mcp")]
    Mcp,
    #[strum(serialize = "hooks")]
    Hooks,
    #[strum(serialize = "hook-output")]
    HookOutput,
    #[strum(serialize = "balance")]
    Balance,
    #[strum(serialize = "lang")]
    Lang,
    #[strum(serialize = "stats")]
    Stats,
    #[strum(serialize = "tasks-dag")]
    TasksDag,
    #[strum(serialize = "background")]
    Background,
    #[strum(serialize = "goal")]
    Goal,
}

/// One declared subcommand, recursive to arbitrary depth.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Subcommand {
    pub name: &'static str,
    pub hint: &'static str,
    pub children: &'static [Subcommand],
    pub takes_value: bool,
}

const fn sub(
    name: &'static str,
    hint: &'static str,
    children: &'static [Subcommand],
    takes_value: bool,
) -> Subcommand {
    Subcommand {
        name,
        hint,
        children,
        takes_value,
    }
}

const SKILL_SUBCOMMANDS: &[Subcommand] = &[
    sub("list", "", &[], false),
    sub("reload", "", &[], false),
    sub("disable", "<name>", &[], true),
    sub("enable", "<name>", &[], true),
];

const MCP_SUBCOMMANDS: &[Subcommand] = &[
    sub("auth", "<server>", &[], true),
    sub("login", "<server>", &[], true),
    sub("list", "", &[], false),
    sub("prompts", "[server]", &[], false),
    sub("prompt", "<server> <name> [key=value ...]", &[], true),
];

const PLUGIN_MARKETPLACE_SUBCOMMANDS: &[Subcommand] = &[
    sub("list", "", &[], false),
    sub("add", "<source>", &[], true),
    sub("update", "<name>", &[], true),
    sub("remove", "<name>", &[], true),
];

const PLUGIN_SUBCOMMANDS: &[Subcommand] = &[
    sub("install", "<plugin[@marketplace]>", &[], true),
    sub("list", "", &[], false),
    sub("reload", "", &[], false),
    sub("uninstall", "<name>", &[], true),
    sub("update", "<name>", &[], true),
    sub("marketplace", "", PLUGIN_MARKETPLACE_SUBCOMMANDS, false),
];

const HOOK_TRUST_SUBCOMMANDS: &[Subcommand] = &[
    sub("--all", "", &[], false),
    sub("--source", "<label>", &[], true),
];

const HOOK_FORGET_SUBCOMMANDS: &[Subcommand] = &[sub("--all", "", &[], false)];

const HOOKS_SUBCOMMANDS: &[Subcommand] = &[
    sub("list", "", &[], false),
    sub("trust", "", HOOK_TRUST_SUBCOMMANDS, false),
    sub("forget", "", HOOK_FORGET_SUBCOMMANDS, false),
];

const GOAL_SUBCOMMANDS: &[Subcommand] = &[
    sub("create", "<objective>", &[], true),
    sub("list", "[--all]", &[sub("--all", "", &[], false)], false),
    sub("show", "<goal-id>", &[], true),
    sub("steer", "<goal-id> <text>", &[], true),
    sub("pause", "<goal-id>", &[], true),
    sub("resume", "<goal-id>", &[], true),
    sub("cancel", "<goal-id>", &[], true),
    sub("accept", "<goal-id> <text>", &[], true),
    sub("archive", "<goal-id>", &[], true),
    sub("unarchive", "<goal-id>", &[], true),
];

impl BuiltinCommand {
    pub const ALL: &'static [Self] = &[
        Self::Theme,
        Self::Model,
        Self::ModelSubagent,
        Self::Permission,
        Self::ViewSystemPrompt,
        Self::Save,
        Self::Compact,
        Self::Cancel,
        Self::SubagentCancel,
        Self::Quit,
        Self::Help,
        Self::History,
        Self::Skill,
        Self::Plugin,
        Self::Mcp,
        Self::Hooks,
        Self::HookOutput,
        Self::Balance,
        Self::Lang,
        Self::Stats,
        Self::TasksDag,
        Self::Background,
        Self::Goal,
    ];

    pub const fn name(self) -> &'static str {
        self.as_ref()
    }

    pub const fn subcommands(self) -> &'static [Subcommand] {
        match self {
            Self::Skill => SKILL_SUBCOMMANDS,
            Self::Mcp => MCP_SUBCOMMANDS,
            Self::Plugin => PLUGIN_SUBCOMMANDS,
            Self::Hooks => HOOKS_SUBCOMMANDS,
            Self::Goal => GOAL_SUBCOMMANDS,
            _ => &[],
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        name.parse().ok()
    }

    pub const fn needs_args(self) -> bool {
        matches!(
            self,
            Self::Skill
                | Self::Plugin
                | Self::Mcp
                | Self::Hooks
                | Self::Goal
                | Self::SubagentCancel
        )
    }

    pub const fn needs_account(self) -> bool {
        matches!(self, Self::Balance)
    }

    /// The localized one-line description used by every frontend.
    pub const fn description(self) -> LocalizedText {
        match self {
            Self::Theme => LocalizedText::new(
                "Choose a color theme (Ctrl+T cycles)",
                "选择颜色主题（Ctrl+T 循环）",
            ),
            Self::Model => LocalizedText::new(
                "Switch model for current provider",
                "切换当前 provider 的模型",
            ),
            Self::ModelSubagent => {
                LocalizedText::new("Switch subagent model", "切换 Subagent 模型")
            }
            Self::Permission => LocalizedText::new(
                "Set permission mode (Default/Plan/Auto)",
                "设置权限模式 (默认/只读/自动)",
            ),
            Self::ViewSystemPrompt => LocalizedText::new("View system prompt", "查看系统提示"),
            Self::Save => LocalizedText::new("Save log to file", "保存日志到文件"),
            Self::Compact => LocalizedText::new("Compact conversation history", "压缩对话历史"),
            Self::Cancel => LocalizedText::new("Cancel current task", "取消当前任务"),
            Self::SubagentCancel => LocalizedText::new(
                "Cancel a running subagent (usage: /subagent_cancel <child-id>)",
                "取消运行中的子代理（用法：/subagent_cancel <child-id>）",
            ),
            Self::Quit => LocalizedText::new("Quit application", "退出应用"),
            Self::Help => LocalizedText::new("Show help panel", "显示帮助面板"),
            Self::History => LocalizedText::new("Show task history", "显示任务历史"),
            Self::Skill => LocalizedText::new(
                "List, reload or run a skill (usage: /skill list | /skill reload | /skill <name>)",
                "列出、重新加载或运行技能（用法：/skill list | /skill reload | /skill <名称>）",
            ),
            Self::Plugin => LocalizedText::new("Manage plugins and marketplaces", "管理插件和市场"),
            Self::Mcp => LocalizedText::new(
                "Manage MCP servers (usage: /mcp auth <server> | /mcp list)",
                "管理 MCP server（用法：/mcp auth <server> | /mcp list）",
            ),
            Self::Hooks => LocalizedText::new(
                "Review command hooks (usage: /hooks list | /hooks trust --all | /hooks forget --all)",
                "审阅 command hook（用法：/hooks list | /hooks trust --all | /hooks forget --all）",
            ),
            Self::HookOutput => LocalizedText::new(
                "Show or hide hook output in the log",
                "显示或隐藏日志里的 hook 输出",
            ),
            Self::Balance => LocalizedText::new(
                "Query account balance (DeepSeek/Kimi)",
                "查询账户余额 (DeepSeek/Kimi)",
            ),
            Self::Lang => LocalizedText::new("Toggle language (EN/中文)", "切换语言 (EN/中文)"),
            Self::Stats => LocalizedText::new("Show session statistics", "显示会话统计"),
            Self::TasksDag => LocalizedText::new("Show task dependency DAG", "显示任务依赖 DAG"),
            Self::Background => {
                LocalizedText::new("Check background task status", "查看后台任务状态")
            }
            Self::Goal => LocalizedText::new(
                "Create, inspect, and control durable Goals",
                "创建、查看和控制持久 Goal",
            ),
        }
    }

    const fn as_ref(self) -> &'static str {
        match self {
            Self::Theme => "theme",
            Self::Model => "model",
            Self::ModelSubagent => "model-subagent",
            Self::Permission => "permission",
            Self::ViewSystemPrompt => "view-system-prompt",
            Self::Save => "save",
            Self::Compact => "compact",
            Self::Cancel => "cancel",
            Self::SubagentCancel => "subagent_cancel",
            Self::Quit => "quit",
            Self::Help => "help",
            Self::History => "history",
            Self::Skill => "skill",
            Self::Plugin => "plugin",
            Self::Mcp => "mcp",
            Self::Hooks => "hooks",
            Self::HookOutput => "hook-output",
            Self::Balance => "balance",
            Self::Lang => "lang",
            Self::Stats => "stats",
            Self::TasksDag => "tasks-dag",
            Self::Background => "background",
            Self::Goal => "goal",
        }
    }
}

#[cfg(test)]
mod tests {
    use strum::IntoEnumIterator;

    use super::*;

    #[test]
    fn every_builtin_variant_has_one_catalog_spec() {
        assert_eq!(BuiltinCommand::iter().count(), 23);
        assert_eq!(BuiltinCommand::ALL.len(), 23);
        assert_eq!(BuiltinCommand::ModelSubagent.name(), "model-subagent");
        assert_eq!(BuiltinCommand::SubagentCancel.name(), "subagent_cancel");
        assert_eq!(
            BuiltinCommand::ViewSystemPrompt.name(),
            "view-system-prompt"
        );
        assert_eq!(BuiltinCommand::TasksDag.name(), "tasks-dag");
    }

    #[test]
    fn slash_subcommands_form_a_tree_without_duplicate_siblings() {
        fn walk(commands: &[Subcommand]) {
            let mut seen = std::collections::HashSet::new();
            for command in commands {
                assert!(seen.insert(command.name), "duplicate {}", command.name);
                walk(command.children);
            }
        }

        for command in BuiltinCommand::ALL {
            walk(command.subcommands());
        }
    }
}
