use std::sync::OnceLock;

use strum::IntoEnumIterator;

use crate::cli::ArgumentValue;
use crate::{
    BuiltinCommand, CatalogLocale, ChoiceSource, CliEntry, ComposerAction, LocalizedText,
    SettingId, Subcommand, fuzzy_score,
};

use super::cli::{
    ArgumentSpec, Availability, CommandAction, CommandGroup, DisabledReason, Shortcut,
};

/// Stable identity for a catalog entry.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum CommandId {
    Builtin(BuiltinCommand),
    BuiltinPath {
        root: BuiltinCommand,
        path: Vec<&'static str>,
    },
    Cli(CliEntry),
    Setting(SettingId),
    Composer(ComposerAction),
    DynamicSkill(String),
}

/// One catalog entry.
#[derive(Debug, Clone)]
pub struct CommandDescriptor {
    pub id: CommandId,
    pub parent: Option<CommandId>,
    pub children: Vec<CommandId>,
    pub group: CommandGroup,
    pub path: Vec<String>,
    pub label: LocalizedText,
    pub description: LocalizedText,
    pub aliases: Vec<String>,
    pub shortcut: Option<Shortcut>,
    pub args: Vec<ArgumentSpec>,
    pub availability: Availability,
    pub action: CommandAction,
}

/// A relevance-ranked search result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandSearchHit {
    pub id: CommandId,
    pub score: i32,
    pub title: LocalizedText,
    pub subtitle: LocalizedText,
}

/// The command catalog.
#[derive(Debug, Default)]
pub struct CommandCatalog {
    entries: Vec<CommandDescriptor>,
}

impl CommandCatalog {
    pub fn get(&self, id: &CommandId) -> Option<&CommandDescriptor> {
        self.entries.iter().find(|entry| &entry.id == id)
    }

    pub fn resolve_path(&self, path: &[&str]) -> Option<&CommandDescriptor> {
        self.entries.iter().find(|entry| {
            entry
                .path
                .iter()
                .map(String::as_str)
                .eq(path.iter().copied())
        })
    }

    pub fn children(&self, parent: &CommandId) -> impl Iterator<Item = &CommandDescriptor> {
        self.entries
            .iter()
            .filter(move |entry| entry.parent.as_ref() == Some(parent))
    }

    /// Every descriptor in declaration order.
    pub fn entries(&self) -> impl Iterator<Item = &CommandDescriptor> {
        self.entries.iter()
    }

    pub fn search(&self, query: &str, locale: CatalogLocale) -> Vec<CommandSearchHit> {
        let mut seen = std::collections::HashSet::new();
        let mut hits: Vec<_> = self
            .entries
            .iter()
            .filter_map(|entry| {
                let title = entry.label.get(locale);
                let subtitle = entry.description.get(locale);
                let path = entry.path.join(" ");
                let score = fuzzy_score(query, title)
                    .or_else(|| fuzzy_score(query, &path))
                    .or_else(|| fuzzy_score(query, subtitle))?;
                Some(CommandSearchHit {
                    id: entry.id.clone(),
                    score,
                    title: entry.label,
                    subtitle: entry.description,
                })
            })
            .collect();
        hits.sort_by_key(|hit| std::cmp::Reverse(hit.score));
        hits.retain(|hit| {
            self.get(&hit.id)
                .is_some_and(|entry| seen.insert(canonical_action_key(&entry.action)))
        });
        for hit in &mut hits {
            let Some(entry) = self.get(&hit.id) else {
                continue;
            };
            let key = canonical_action_key(&entry.action);
            if let Some(canonical) = self
                .entries
                .iter()
                .filter(|candidate| {
                    matches!(
                        candidate.id,
                        CommandId::BuiltinPath { .. } | CommandId::Builtin(_)
                    ) && canonical_action_key(&candidate.action) == key
                })
                .max_by_key(|candidate| match &candidate.id {
                    CommandId::BuiltinPath { path, .. } => path.len(),
                    _ => 0,
                })
            {
                hit.id = canonical.id.clone();
                hit.title = canonical.label;
                hit.subtitle = canonical.description;
            }
        }
        hits
    }

    fn push(&mut self, descriptor: CommandDescriptor) -> usize {
        let index = self.entries.len();
        self.entries.push(descriptor);
        index
    }
}

/// The process-wide built-in catalog.
pub fn builtins() -> &'static CommandCatalog {
    static CATALOG: OnceLock<CommandCatalog> = OnceLock::new();
    CATALOG.get_or_init(build_builtins)
}

fn build_builtins() -> CommandCatalog {
    let mut catalog = CommandCatalog::default();
    for command in BuiltinCommand::iter() {
        let path = vec![command.name().to_string()];
        let id = CommandId::Builtin(command);
        let root_index = catalog.push(CommandDescriptor {
            id: id.clone(),
            parent: None,
            children: Vec::new(),
            group: group_for(command),
            path,
            label: LocalizedText::new(command.name(), command.name()),
            description: command.description(),
            aliases: Vec::new(),
            shortcut: None,
            args: builtin_args(command),
            availability: availability_for(command),
            action: CommandAction::Builtin(command),
        });
        let children = add_subcommands(
            &mut catalog,
            &id,
            command,
            &mut vec![command.name()],
            command.subcommands(),
        );
        catalog.entries[root_index].children = children;
    }
    for entry in CliEntry::iter() {
        let text = entry.as_str();
        catalog.push(CommandDescriptor {
            id: CommandId::Cli(entry),
            parent: None,
            children: Vec::new(),
            group: CommandGroup::Cli,
            path: text.split_whitespace().map(str::to_string).collect(),
            label: LocalizedText::new(text, text),
            description: entry.description(),
            aliases: vec![text.to_string()],
            shortcut: None,
            args: cli_args(entry),
            availability: cli_availability(entry),
            action: CommandAction::Cli(entry),
        });
    }
    for entry in SettingId::iter() {
        let text = entry.as_str();
        catalog.push(CommandDescriptor {
            id: CommandId::Setting(entry),
            parent: None,
            children: Vec::new(),
            group: CommandGroup::Appearance,
            path: vec![text.to_string()],
            label: entry.label(),
            description: entry.description(),
            aliases: vec![text.to_string()],
            shortcut: None,
            args: Vec::new(),
            availability: Availability::Always,
            action: CommandAction::Setting(entry),
        });
    }
    for entry in ComposerAction::iter() {
        let text = entry.as_str();
        catalog.push(CommandDescriptor {
            id: CommandId::Composer(entry),
            parent: None,
            children: Vec::new(),
            group: CommandGroup::Composer,
            path: vec![text.to_string()],
            label: LocalizedText::new(text, text),
            description: entry.description(),
            aliases: vec![text.to_string()],
            shortcut: None,
            args: Vec::new(),
            availability: composer_availability(entry),
            action: CommandAction::Composer(entry),
        });
    }
    catalog
}

/// One identity for aliases and composer/builtin spellings that execute the
/// same action.
pub fn canonical_action_key(action: &CommandAction) -> String {
    match action {
        CommandAction::Builtin(command) => match command {
            BuiltinCommand::Cancel => "builtin:cancel".into(),
            BuiltinCommand::Compact => "builtin:compact".into(),
            _ => format!("builtin:{}", command.name()),
        },
        CommandAction::BuiltinPath { path, .. } => match path.as_slice() {
            ["mcp", "auth" | "login"] => "command:mcp:auth".into(),
            ["hooks", "forget"] | ["hooks", "forget", "--all"] => {
                "command:hooks:forget:--all".into()
            }
            _ => format!("command:{}", path.join(":")),
        },
        CommandAction::Cli(CliEntry::HooksForget) => "command:hooks:forget:--all".into(),
        CommandAction::Cli(entry) => format!("command:{}", entry.key()),
        CommandAction::Setting(id) => match id {
            SettingId::Model => "builtin:model".into(),
            SettingId::SubagentModel => "builtin:model-subagent".into(),
            SettingId::PermissionMode => "builtin:permission".into(),
            SettingId::Theme => "builtin:theme".into(),
            SettingId::Language => "builtin:lang".into(),
            SettingId::HookOutput => "builtin:hook-output".into(),
            _ => format!("setting:{}", id.as_str()),
        },
        CommandAction::Composer(ComposerAction::Stop) => "builtin:cancel".into(),
        CommandAction::Composer(ComposerAction::Compact) => "builtin:compact".into(),
        CommandAction::Composer(action) => format!("composer:{}", action.as_str()),
        CommandAction::DynamicSkill(name) => format!("skill:{name}"),
    }
}

fn add_subcommands(
    catalog: &mut CommandCatalog,
    parent: &CommandId,
    root: BuiltinCommand,
    prefix: &mut Vec<&'static str>,
    subcommands: &'static [Subcommand],
) -> Vec<CommandId> {
    let mut child_ids = Vec::with_capacity(subcommands.len());
    for subcommand in subcommands {
        prefix.push(subcommand.name);
        let path = prefix.to_vec();
        let id = CommandId::BuiltinPath {
            root,
            path: path.clone(),
        };
        let index = catalog.push(CommandDescriptor {
            id: id.clone(),
            parent: Some(parent.clone()),
            children: Vec::new(),
            group: group_for(root),
            path: path.iter().map(|part| (*part).to_string()).collect(),
            label: LocalizedText::new(subcommand.name, subcommand.name),
            description: subcommand_description(root, &path, subcommand.hint),
            aliases: vec![prefix.join(" ")],
            shortcut: None,
            args: subcommand_args(root, &path, subcommand.takes_value),
            availability: availability_for(root),
            action: CommandAction::BuiltinPath {
                root,
                path: path.clone(),
            },
        });
        let children = add_subcommands(catalog, &id, root, prefix, subcommand.children);
        catalog.entries[index].children = children;
        child_ids.push(id);
        prefix.pop();
    }
    child_ids
}

fn subcommand_description(
    root: BuiltinCommand,
    path: &[&str],
    hint: &'static str,
) -> LocalizedText {
    let alias = match (root, path) {
        (BuiltinCommand::Plugin, ["plugin", "list"]) => Some(CliEntry::PluginList),
        (BuiltinCommand::Plugin, ["plugin", "reload"]) => Some(CliEntry::PluginReload),
        (BuiltinCommand::Plugin, ["plugin", "marketplace", "list"]) => {
            Some(CliEntry::MarketplaceList)
        }
        (BuiltinCommand::Mcp, ["mcp", "list"]) => Some(CliEntry::McpList),
        (BuiltinCommand::Hooks, ["hooks", "list"]) => Some(CliEntry::HooksList),
        (BuiltinCommand::Hooks, ["hooks", "trust"]) => Some(CliEntry::HooksTrust),
        (BuiltinCommand::Hooks, ["hooks", "forget"]) => Some(CliEntry::HooksForget),
        _ => None,
    };
    alias.map(CliEntry::description).unwrap_or_else(|| {
        if hint.is_empty() {
            LocalizedText::new("Run this command", "运行此命令")
        } else {
            LocalizedText::new(hint, hint)
        }
    })
}

fn subcommand_args(root: BuiltinCommand, path: &[&str], takes_value: bool) -> Vec<ArgumentSpec> {
    if matches!(root, BuiltinCommand::Mcp) && matches!(path, ["mcp", "prompt"]) {
        return vec![ArgumentSpec {
            name: "server name key=value...",
            required: true,
            value: ArgumentValue::FreeText,
        }];
    }
    if matches!(root, BuiltinCommand::Goal) && matches!(path, ["goal", "steer" | "accept"]) {
        return vec![
            ArgumentSpec {
                name: "goal",
                required: true,
                value: ArgumentValue::Choice(ChoiceSource::Goals),
            },
            ArgumentSpec {
                name: "text",
                required: true,
                value: ArgumentValue::FreeText,
            },
        ];
    }
    if takes_value {
        vec![ArgumentSpec {
            name: "value",
            required: true,
            value: subcommand_choice(root, path),
        }]
    } else {
        Vec::new()
    }
}

fn subcommand_choice(root: BuiltinCommand, path: &[&str]) -> ArgumentValue {
    let value = match (root, path) {
        (BuiltinCommand::Skill, ["skill", "disable" | "enable"]) => ChoiceSource::Skills,
        (BuiltinCommand::Plugin, ["plugin", "uninstall" | "update"]) => ChoiceSource::Plugins,
        (BuiltinCommand::Mcp, ["mcp", "auth" | "login" | "get" | "remove" | "logout"]) => {
            ChoiceSource::McpServers
        }
        (BuiltinCommand::Hooks, ["hooks", "trust", "--source"]) => ChoiceSource::HookSources,
        (
            BuiltinCommand::Goal,
            [
                "goal",
                "show" | "steer" | "pause" | "resume" | "cancel" | "accept" | "archive"
                | "unarchive",
            ],
        ) => ChoiceSource::Goals,
        _ => return ArgumentValue::FreeText,
    };
    ArgumentValue::Choice(value)
}

fn builtin_args(command: BuiltinCommand) -> Vec<ArgumentSpec> {
    let value = match command {
        BuiltinCommand::Model | BuiltinCommand::ModelSubagent | BuiltinCommand::Permission => {
            Some(false)
        }
        BuiltinCommand::SubagentCancel => Some(true),
        _ => None,
    };
    value.map_or_else(Vec::new, |required| {
        vec![ArgumentSpec {
            name: "value",
            required,
            value: ArgumentValue::FreeText,
        }]
    })
}

fn cli_args(entry: CliEntry) -> Vec<ArgumentSpec> {
    let (name, required, value) = match entry {
        CliEntry::Headless => ("prompt", true, ArgumentValue::FreeText),
        CliEntry::PluginInstall => ("plugin", true, ArgumentValue::FreeText),
        CliEntry::PluginUninstall | CliEntry::PluginUpdate => (
            "plugin",
            true,
            ArgumentValue::Choice(super::cli::ChoiceSource::Plugins),
        ),
        CliEntry::MarketplaceAdd => ("marketplace", true, ArgumentValue::FreeText),
        CliEntry::MarketplaceUpdate | CliEntry::MarketplaceRemove => {
            ("marketplace", true, ArgumentValue::FreeText)
        }
        CliEntry::McpGet | CliEntry::McpRemove | CliEntry::McpLogout => (
            "server",
            true,
            ArgumentValue::Choice(super::cli::ChoiceSource::McpServers),
        ),
        CliEntry::McpAdd => ("server", true, ArgumentValue::FreeText),
        CliEntry::McpLogin | CliEntry::McpAuth => (
            "server",
            true,
            ArgumentValue::Choice(super::cli::ChoiceSource::McpServers),
        ),
        CliEntry::HooksTrust => (
            "source",
            false,
            ArgumentValue::Choice(super::cli::ChoiceSource::HookSources),
        ),
        _ => return Vec::new(),
    };
    vec![ArgumentSpec {
        name,
        required,
        value,
    }]
}

const fn group_for(command: BuiltinCommand) -> CommandGroup {
    match command {
        BuiltinCommand::Model
        | BuiltinCommand::ModelSubagent
        | BuiltinCommand::Permission
        | BuiltinCommand::Compact
        | BuiltinCommand::Cancel
        | BuiltinCommand::SubagentCancel
        | BuiltinCommand::Balance => CommandGroup::Run,
        BuiltinCommand::Theme | BuiltinCommand::Lang => CommandGroup::Appearance,
        BuiltinCommand::Skill
        | BuiltinCommand::Plugin
        | BuiltinCommand::Mcp
        | BuiltinCommand::Hooks
        | BuiltinCommand::HookOutput => CommandGroup::Extensions,
        BuiltinCommand::Goal => CommandGroup::Goals,
        BuiltinCommand::Stats
        | BuiltinCommand::TasksDag
        | BuiltinCommand::Background
        | BuiltinCommand::Help
        | BuiltinCommand::History => CommandGroup::Readers,
        _ => CommandGroup::Session,
    }
}

const fn availability_for(command: BuiltinCommand) -> Availability {
    match command {
        BuiltinCommand::Balance => Availability::NeedsAccount,
        BuiltinCommand::Skill
        | BuiltinCommand::Plugin
        | BuiltinCommand::Mcp
        | BuiltinCommand::Hooks => Availability::NeedsProject,
        BuiltinCommand::ViewSystemPrompt
        | BuiltinCommand::Save
        | BuiltinCommand::Compact
        | BuiltinCommand::Cancel
        | BuiltinCommand::SubagentCancel
        | BuiltinCommand::History
        | BuiltinCommand::Stats
        | BuiltinCommand::TasksDag
        | BuiltinCommand::Background
        | BuiltinCommand::Goal => Availability::NeedsActiveSession,
        _ => Availability::Always,
    }
}

const fn cli_availability(entry: CliEntry) -> Availability {
    match entry {
        CliEntry::Headless | CliEntry::UpgradeCheck | CliEntry::UpgradeInstall => {
            Availability::Always
        }
        _ => Availability::NeedsProject,
    }
}

const fn composer_availability(action: ComposerAction) -> Availability {
    match action {
        ComposerAction::Voice => Availability::Disabled(DisabledReason::Unavailable),
        ComposerAction::Stop | ComposerAction::Compact | ComposerAction::Queue => {
            Availability::NeedsActiveSession
        }
        ComposerAction::SessionActions => Availability::NeedsProject,
        ComposerAction::MentionFile | ComposerAction::AttachImage => Availability::Always,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_cli_entry_has_one_catalog_spec() {
        let catalog = builtins();
        for entry in CliEntry::iter() {
            assert!(catalog.get(&CommandId::Cli(entry)).is_some());
        }
    }

    #[test]
    fn every_setting_has_one_catalog_spec() {
        let catalog = builtins();
        for entry in SettingId::iter() {
            assert!(catalog.get(&CommandId::Setting(entry)).is_some());
        }
    }

    #[test]
    fn every_setting_has_localized_labels_and_descriptions() {
        for entry in SettingId::iter() {
            assert_ne!(
                entry.label().get(CatalogLocale::En),
                entry.label().get(CatalogLocale::Zh)
            );
            assert_ne!(
                entry.description().get(CatalogLocale::En),
                entry.description().get(CatalogLocale::Zh)
            );
        }
    }

    #[test]
    fn every_builtin_has_a_root_and_declared_subcommands() {
        let catalog = builtins();
        for command in BuiltinCommand::iter() {
            let root = CommandId::Builtin(command);
            assert!(catalog.get(&root).is_some());
            for subcommand in command.subcommands() {
                assert!(
                    catalog
                        .resolve_path(&[command.name(), subcommand.name])
                        .is_some(),
                    "missing {} {}",
                    command.name(),
                    subcommand.name
                );
            }
        }
    }

    #[test]
    fn goal_text_commands_declare_both_required_inputs() {
        let catalog = builtins();
        for verb in ["steer", "accept"] {
            let descriptor = catalog.resolve_path(&["goal", verb]).expect("goal verb");
            assert_eq!(
                descriptor
                    .args
                    .iter()
                    .filter(|argument| argument.required)
                    .count(),
                2,
                "{verb}"
            );
        }
    }

    #[test]
    fn descriptor_children_match_the_declared_subcommand_tree() {
        fn walk(catalog: &CommandCatalog, parent: &CommandId, declared: &[Subcommand]) {
            let actual: Vec<_> = catalog.children(parent).collect();
            assert_eq!(actual.len(), declared.len());
            for (descriptor, subcommand) in actual.into_iter().zip(declared) {
                assert_eq!(
                    descriptor.path.last().map(String::as_str),
                    Some(subcommand.name)
                );
                walk(catalog, &descriptor.id, subcommand.children);
            }
        }

        let catalog = builtins();
        for command in BuiltinCommand::iter() {
            walk(catalog, &CommandId::Builtin(command), command.subcommands());
        }
    }

    #[test]
    fn setting_aliases_dedupe_to_the_builtin_action() {
        let catalog = builtins();
        for (query, expected, alias) in [
            (
                "model",
                CommandId::Builtin(BuiltinCommand::Model),
                SettingId::Model,
            ),
            (
                "model-subagent",
                CommandId::Builtin(BuiltinCommand::ModelSubagent),
                SettingId::SubagentModel,
            ),
            (
                "permission",
                CommandId::Builtin(BuiltinCommand::Permission),
                SettingId::PermissionMode,
            ),
            (
                "theme",
                CommandId::Builtin(BuiltinCommand::Theme),
                SettingId::Theme,
            ),
            (
                "lang",
                CommandId::Builtin(BuiltinCommand::Lang),
                SettingId::Language,
            ),
        ] {
            let hits = catalog.search(query, CatalogLocale::En);
            assert_eq!(hits.iter().filter(|hit| hit.id == expected).count(), 1);
            assert!(
                !hits.iter().any(|hit| hit.id == CommandId::Setting(alias)),
                "setting alias leaked for {query}: {hits:?}"
            );
        }
    }
}
