use strum::EnumIter;

use crate::LocalizedText;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EnumIter)]
pub enum CliEntry {
    Headless,
    UpgradeCheck,
    UpgradeInstall,
    PluginList,
    PluginInstall,
    PluginUninstall,
    PluginUpdate,
    PluginReload,
    MarketplaceAdd,
    MarketplaceList,
    MarketplaceUpdate,
    MarketplaceRemove,
    McpList,
    McpGet,
    McpAdd,
    McpRemove,
    McpLogin,
    McpAuth,
    McpLogout,
    HooksList,
    HooksTrust,
    HooksForget,
    ListSessions,
    ResumeLast,
}

impl CliEntry {
    pub const ALL: &'static [Self] = &[
        Self::Headless,
        Self::UpgradeCheck,
        Self::UpgradeInstall,
        Self::PluginList,
        Self::PluginInstall,
        Self::PluginUninstall,
        Self::PluginUpdate,
        Self::PluginReload,
        Self::MarketplaceAdd,
        Self::MarketplaceList,
        Self::MarketplaceUpdate,
        Self::MarketplaceRemove,
        Self::McpList,
        Self::McpGet,
        Self::McpAdd,
        Self::McpRemove,
        Self::McpLogin,
        Self::McpAuth,
        Self::McpLogout,
        Self::HooksList,
        Self::HooksTrust,
        Self::HooksForget,
        Self::ListSessions,
        Self::ResumeLast,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Headless => "headless",
            Self::UpgradeCheck => "upgrade --check",
            Self::UpgradeInstall => "upgrade",
            Self::PluginList => "plugin list",
            Self::PluginInstall => "plugin install",
            Self::PluginUninstall => "plugin uninstall",
            Self::PluginUpdate => "plugin update",
            Self::PluginReload => "plugin reload",
            Self::MarketplaceAdd => "plugin marketplace add",
            Self::MarketplaceList => "plugin marketplace list",
            Self::MarketplaceUpdate => "plugin marketplace update",
            Self::MarketplaceRemove => "plugin marketplace remove",
            Self::McpList => "mcp list",
            Self::McpGet => "mcp get",
            Self::McpAdd => "mcp add",
            Self::McpRemove => "mcp remove",
            Self::McpLogin => "mcp login",
            Self::McpAuth => "mcp auth",
            Self::McpLogout => "mcp logout",
            Self::HooksList => "hooks list",
            Self::HooksTrust => "hooks trust",
            Self::HooksForget => "hooks forget",
            Self::ListSessions => "--list-sessions",
            Self::ResumeLast => "--resume-last",
        }
    }

    pub const fn key(self) -> &'static str {
        match self {
            Self::Headless => "headless",
            Self::UpgradeCheck => "upgrade:check",
            Self::UpgradeInstall => "upgrade:install",
            Self::PluginList => "plugin:list",
            Self::PluginInstall => "plugin:install",
            Self::PluginUninstall => "plugin:uninstall",
            Self::PluginUpdate => "plugin:update",
            Self::PluginReload => "plugin:reload",
            Self::MarketplaceAdd => "plugin:marketplace:add",
            Self::MarketplaceList => "plugin:marketplace:list",
            Self::MarketplaceUpdate => "plugin:marketplace:update",
            Self::MarketplaceRemove => "plugin:marketplace:remove",
            Self::McpList => "mcp:list",
            Self::McpGet => "mcp:get",
            Self::McpAdd => "mcp:add",
            Self::McpRemove => "mcp:remove",
            Self::McpLogin | Self::McpAuth => "mcp:auth",
            Self::McpLogout => "mcp:logout",
            Self::HooksList => "hooks:list",
            Self::HooksTrust => "hooks:trust",
            Self::HooksForget => "hooks:forget",
            Self::ListSessions => "sessions:list",
            Self::ResumeLast => "sessions:resume-last",
        }
    }

    pub const fn description(self) -> LocalizedText {
        match self {
            Self::Headless => LocalizedText::new("Run a headless task", "运行无头任务"),
            Self::UpgradeCheck => LocalizedText::new("Check for updates", "检查更新"),
            Self::UpgradeInstall => LocalizedText::new("Install an update", "安装更新"),
            Self::PluginList => LocalizedText::new("List plugins", "列出插件"),
            Self::PluginInstall => LocalizedText::new("Install a plugin", "安装插件"),
            Self::PluginUninstall => LocalizedText::new("Uninstall a plugin", "卸载插件"),
            Self::PluginUpdate => LocalizedText::new("Update a plugin", "更新插件"),
            Self::PluginReload => LocalizedText::new("Reload plugins", "重新加载插件"),
            Self::MarketplaceAdd => LocalizedText::new("Add a marketplace", "添加市场"),
            Self::MarketplaceList => LocalizedText::new("List marketplaces", "列出市场"),
            Self::MarketplaceUpdate => LocalizedText::new("Update a marketplace", "更新市场"),
            Self::MarketplaceRemove => LocalizedText::new("Remove a marketplace", "移除市场"),
            Self::McpList => LocalizedText::new("List MCP servers", "列出 MCP 服务"),
            Self::McpGet => LocalizedText::new("Show one MCP server", "查看 MCP 服务"),
            Self::McpAdd => LocalizedText::new("Add an MCP server", "添加 MCP 服务"),
            Self::McpRemove => LocalizedText::new("Remove an MCP server", "移除 MCP 服务"),
            Self::McpLogin | Self::McpAuth => {
                LocalizedText::new("Authorize an MCP server", "授权 MCP 服务")
            }
            Self::McpLogout => LocalizedText::new("Log out of an MCP server", "退出 MCP 授权"),
            Self::HooksList => LocalizedText::new("List hooks", "列出 Hook"),
            Self::HooksTrust => LocalizedText::new("Trust hooks", "信任 Hook"),
            Self::HooksForget => LocalizedText::new("Forget hook trust", "忘记 Hook 信任"),
            Self::ListSessions => LocalizedText::new("List sessions", "列出会话"),
            Self::ResumeLast => LocalizedText::new("Resume the last session", "继续上一个会话"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EnumIter)]
pub enum SettingId {
    Provider,
    Model,
    SubagentModel,
    ApiKey,
    BaseUrl,
    MaxTokens,
    ThinkingBudget,
    ReasoningEffort,
    PermissionMode,
    Notifications,
    HookOutput,
    ModelContextWindow,
    Theme,
    Language,
    SnapshotMaxItems,
    MicroCompact,
    TokioConsole,
    SkillBodyAutoInject,
    ConfigPath,
}

impl SettingId {
    pub const ALL: &'static [Self] = &[
        Self::Provider,
        Self::Model,
        Self::SubagentModel,
        Self::ApiKey,
        Self::BaseUrl,
        Self::MaxTokens,
        Self::ThinkingBudget,
        Self::ReasoningEffort,
        Self::PermissionMode,
        Self::Notifications,
        Self::HookOutput,
        Self::ModelContextWindow,
        Self::Theme,
        Self::Language,
        Self::SnapshotMaxItems,
        Self::MicroCompact,
        Self::TokioConsole,
        Self::SkillBodyAutoInject,
        Self::ConfigPath,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Provider => "provider",
            Self::Model => "model",
            Self::SubagentModel => "model-subagent",
            Self::ApiKey => "api-key",
            Self::BaseUrl => "base-url",
            Self::MaxTokens => "max-tokens",
            Self::ThinkingBudget => "thinking-budget",
            Self::ReasoningEffort => "reasoning-effort",
            Self::PermissionMode => "permission-mode",
            Self::Notifications => "notifications",
            Self::HookOutput => "hook-output",
            Self::ModelContextWindow => "model-context-window",
            Self::Theme => "theme",
            Self::Language => "language",
            Self::SnapshotMaxItems => "snapshot-max-items",
            Self::MicroCompact => "micro-compact",
            Self::TokioConsole => "tokio-console",
            Self::SkillBodyAutoInject => "skill-body-auto-inject",
            Self::ConfigPath => "config",
        }
    }

    pub const fn label(self) -> LocalizedText {
        match self {
            Self::Provider => LocalizedText::new("Provider", "提供商"),
            Self::Model => LocalizedText::new("Model", "模型"),
            Self::SubagentModel => LocalizedText::new("Subagent model", "子代理模型"),
            Self::ApiKey => LocalizedText::new("API key", "API 密钥"),
            Self::BaseUrl => LocalizedText::new("Base URL", "基础 URL"),
            Self::MaxTokens => LocalizedText::new("Max output tokens", "最大输出 Token"),
            Self::ThinkingBudget => LocalizedText::new("Thinking budget", "思考预算"),
            Self::ReasoningEffort => LocalizedText::new("Reasoning effort", "推理强度"),
            Self::PermissionMode => LocalizedText::new("Permission mode", "权限模式"),
            Self::Notifications => LocalizedText::new("Notifications", "通知"),
            Self::HookOutput => LocalizedText::new("Hook output", "Hook 输出"),
            Self::ModelContextWindow => {
                LocalizedText::new("Model context window", "模型上下文窗口")
            }
            Self::Theme => LocalizedText::new("Theme", "主题"),
            Self::Language => LocalizedText::new("Language", "语言"),
            Self::SnapshotMaxItems => LocalizedText::new("Snapshot max items", "快照最大条目数"),
            Self::MicroCompact => LocalizedText::new("Micro compaction", "微型压缩"),
            Self::TokioConsole => LocalizedText::new("Tokio console", "Tokio 诊断"),
            Self::SkillBodyAutoInject => LocalizedText::new("Skill body injection", "技能正文注入"),
            Self::ConfigPath => LocalizedText::new("Config path", "配置路径"),
        }
    }

    pub const fn description(self) -> LocalizedText {
        match self {
            Self::Provider
            | Self::ApiKey
            | Self::BaseUrl
            | Self::MaxTokens
            | Self::ModelContextWindow
            | Self::SnapshotMaxItems
            | Self::MicroCompact
            | Self::TokioConsole
            | Self::SkillBodyAutoInject
            | Self::ConfigPath => LocalizedText::new("Startup-global setting", "启动级设置"),
            Self::Model
            | Self::SubagentModel
            | Self::ThinkingBudget
            | Self::ReasoningEffort
            | Self::PermissionMode
            | Self::Notifications
            | Self::HookOutput
            | Self::Theme
            | Self::Language => LocalizedText::new("Live setting", "实时设置"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EnumIter)]
pub enum ComposerAction {
    MentionFile,
    AttachImage,
    Voice,
    Queue,
    Stop,
    Compact,
    SessionActions,
}

impl ComposerAction {
    pub const ALL: &'static [Self] = &[
        Self::MentionFile,
        Self::AttachImage,
        Self::Voice,
        Self::Queue,
        Self::Stop,
        Self::Compact,
        Self::SessionActions,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MentionFile => "mention-file",
            Self::AttachImage => "attach-image",
            Self::Voice => "voice",
            Self::Queue => "queue",
            Self::Stop => "stop",
            Self::Compact => "compact",
            Self::SessionActions => "session-actions",
        }
    }

    pub const fn description(self) -> LocalizedText {
        match self {
            Self::MentionFile => LocalizedText::new("Mention a workspace file", "引用工作区文件"),
            Self::AttachImage => LocalizedText::new("Attach an image", "附加图片"),
            Self::Voice => LocalizedText::new("Transcribe voice input", "转写语音输入"),
            Self::Queue => LocalizedText::new("Show queued prompts", "查看排队提示"),
            Self::Stop => LocalizedText::new("Cancel the active run", "取消当前运行"),
            Self::Compact => LocalizedText::new("Compact conversation history", "压缩对话历史"),
            Self::SessionActions => LocalizedText::new("Session actions", "会话操作"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandGroup {
    Session,
    Run,
    Appearance,
    Account,
    Goals,
    Extensions,
    Readers,
    Composer,
    Cli,
}

impl CommandGroup {
    pub const fn label(self) -> LocalizedText {
        match self {
            Self::Session => LocalizedText::new("Session", "会话"),
            Self::Run => LocalizedText::new("Run", "运行"),
            Self::Appearance => LocalizedText::new("Appearance", "外观"),
            Self::Account => LocalizedText::new("Account", "账户"),
            Self::Goals => LocalizedText::new("Goals", "目标"),
            Self::Extensions => LocalizedText::new("Extensions", "扩展"),
            Self::Readers => LocalizedText::new("Readers", "阅读器"),
            Self::Composer => LocalizedText::new("Composer", "编辑器"),
            Self::Cli => LocalizedText::new("CLI", "CLI"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shortcut {
    pub modifiers: &'static str,
    pub key: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArgumentSpec {
    pub name: &'static str,
    pub required: bool,
    pub value: ArgumentValue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArgumentValue {
    FreeText,
    Choice(ChoiceSource),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChoiceSource {
    Skills,
    Plugins,
    McpServers,
    HookSources,
    Goals,
    FilesAndImages,
    Fixed(FixedChoiceSet),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FixedChoiceSet {
    ReasoningEffort,
    PermissionMode,
    Theme,
    Language,
    Boolean,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Availability {
    Always,
    NeedsAccount,
    NeedsActiveSession,
    NeedsProject,
    NeedsInstalledSkill,
    Disabled(DisabledReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisabledReason {
    MissingAccount,
    NoActiveSession,
    NoProject,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandAction {
    Builtin(super::BuiltinCommand),
    BuiltinPath {
        root: super::BuiltinCommand,
        path: Vec<&'static str>,
    },
    Cli(CliEntry),
    Setting(SettingId),
    Composer(ComposerAction),
    DynamicSkill(String),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommandArguments {
    pub tokens: Vec<String>,
}
