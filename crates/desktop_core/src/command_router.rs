//! Public, GPUI-free routing metadata for the desktop command surfaces.

use clap::CommandFactory;
use tact_catalog::{
    ArgumentValue, Availability, BuiltinCommand, ChoiceSource, CliEntry, CommandAction,
    CommandArguments, CommandCatalog, CommandDescriptor, CommandId, CommandSearchHit, SettingId,
};

use tact::config::CliArgs;

use crate::commands::route_cli;

/// Where a catalog action is executed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandTarget {
    /// Dispatch through [`crate::DesktopIntent::InvokeCommand`].
    Controller,
    /// Apply in the shell before controller dispatch.
    Shell,
    /// Close the window.
    Window,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CliRouteClass {
    Host,
    Process,
    Shell,
}

/// The kind of form a command needs before it can execute.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandForm {
    None,
    Text { name: String, required: bool },
    Choice { name: String, source: ChoiceSource },
}

/// Whether a setting applies immediately or only after restart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingMode {
    Live,
    StartupGlobal,
}

/// Runtime facts used to enable or disable catalog entries.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CommandAvailability {
    pub has_project: bool,
    pub has_session: bool,
    pub has_account: bool,
    pub has_installed_skill: bool,
}

/// The shell-facing plan for one command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandPlan {
    pub target: CommandTarget,
    pub form: CommandForm,
    pub setting_mode: Option<SettingMode>,
}

/// Route metadata without executing an action.
pub fn plan(command: &CommandId) -> CommandPlan {
    let target = match command {
        CommandId::Builtin(BuiltinCommand::Quit) => CommandTarget::Window,
        CommandId::Builtin(_) | CommandId::BuiltinPath { .. } | CommandId::DynamicSkill(_) => {
            CommandTarget::Controller
        }
        CommandId::Setting(_) | CommandId::Composer(_) | CommandId::Cli(_) => CommandTarget::Shell,
    };
    let form = form_for(command);
    let setting_mode = match command {
        CommandId::Setting(id) => Some(setting_mode(*id)),
        _ => None,
    };
    CommandPlan {
        target,
        form,
        setting_mode,
    }
}

/// Whether the catalog action reaches executable desktop behavior.
///
/// A route that needs arguments is executable because the shell opens its
/// declared form; a route that returns `Noop` without a form is the
/// visible-but-inert case the parity suite must reject.
pub fn command_is_executable(id: &CommandId) -> bool {
    // CLI catalog entries are surfaced through an executable desktop surface
    // even when they still need a flag or confirmation, such as `hooks forget
    // --all`. Their route class and Clap parity are checked separately.
    if let CommandId::Cli(entry) = id {
        return cli_entry_has_setting_route(*entry);
    }
    let route = crate::commands::route_command(id, &CommandArguments::default());
    match route {
        crate::commands::CommandRoute::Noop => false,
        // Missing arguments are only executable when the catalog declares a
        // form or child commands that the shell can open. An undeclared error is
        // visible-but-inert instead of pretending that routing metadata is action.
        crate::commands::CommandRoute::Error(_) => {
            form_for(id) != CommandForm::None
                || tact_catalog::builtins()
                    .get(id)
                    .is_some_and(|descriptor| !descriptor.children.is_empty())
        }
        _ => true,
    }
}

/// Returns whether a catalog entry can run in the current shell.
pub fn is_enabled(descriptor: &CommandDescriptor, state: CommandAvailability) -> bool {
    match descriptor.availability {
        Availability::Always => true,
        Availability::NeedsAccount => state.has_account,
        Availability::NeedsActiveSession => state.has_session,
        Availability::NeedsProject => state.has_project,
        Availability::NeedsInstalledSkill => state.has_installed_skill,
        Availability::Disabled(_) => false,
    }
}

/// The UI form inferred from the command's declared arguments.
pub fn form_for(command: &CommandId) -> CommandForm {
    if let CommandId::Setting(id) = command {
        return setting_form(*id);
    }
    let Some(descriptor) = tact_catalog::builtins().get(command) else {
        return CommandForm::None;
    };
    descriptor
        .args
        .first()
        .map_or(CommandForm::None, |argument| match argument.value {
            ArgumentValue::FreeText => CommandForm::Text {
                name: argument.name.to_string(),
                required: argument.required,
            },
            ArgumentValue::Choice(source) => CommandForm::Choice {
                name: argument.name.to_string(),
                source,
            },
        })
}

/// Settings that require a full app restart.
pub const fn setting_mode(id: SettingId) -> SettingMode {
    match id {
        SettingId::Provider
        | SettingId::ApiKey
        | SettingId::BaseUrl
        | SettingId::MaxTokens
        | SettingId::ModelContextWindow
        | SettingId::SnapshotMaxItems
        | SettingId::MicroCompact
        | SettingId::TokioConsole
        | SettingId::SkillBodyAutoInject
        | SettingId::ConfigPath => SettingMode::StartupGlobal,
        SettingId::Model
        | SettingId::SubagentModel
        | SettingId::ThinkingBudget
        | SettingId::ReasoningEffort
        | SettingId::PermissionMode
        | SettingId::Notifications
        | SettingId::HookOutput
        | SettingId::Theme
        | SettingId::Language => SettingMode::Live,
    }
}

fn setting_form(id: SettingId) -> CommandForm {
    let choice = match id {
        SettingId::PermissionMode => Some(ChoiceSource::Fixed(
            tact_catalog::FixedChoiceSet::PermissionMode,
        )),
        SettingId::ReasoningEffort => Some(ChoiceSource::Fixed(
            tact_catalog::FixedChoiceSet::ReasoningEffort,
        )),
        SettingId::Theme => Some(ChoiceSource::Fixed(tact_catalog::FixedChoiceSet::Theme)),
        SettingId::Language => Some(ChoiceSource::Fixed(tact_catalog::FixedChoiceSet::Language)),
        SettingId::Notifications
        | SettingId::MicroCompact
        | SettingId::TokioConsole
        | SettingId::SkillBodyAutoInject
        | SettingId::HookOutput => Some(ChoiceSource::Fixed(tact_catalog::FixedChoiceSet::Boolean)),
        _ => None,
    };
    let text = matches!(
        id,
        SettingId::Provider
            | SettingId::Model
            | SettingId::SubagentModel
            | SettingId::ApiKey
            | SettingId::BaseUrl
            | SettingId::MaxTokens
            | SettingId::ThinkingBudget
            | SettingId::ModelContextWindow
            | SettingId::SnapshotMaxItems
            | SettingId::ConfigPath
    );
    if let Some(source) = choice {
        CommandForm::Choice {
            name: id.as_str().to_string(),
            source,
        }
    } else if text {
        CommandForm::Text {
            name: id.as_str().to_string(),
            required: true,
        }
    } else {
        CommandForm::None
    }
}

/// Canonical identity used to deduplicate search hits.
pub fn canonical_key(action: &CommandAction) -> String {
    tact_catalog::canonical_action_key(action)
}

/// Search the catalog while returning one hit per canonical action.
pub fn search<'a>(
    catalog: &'a CommandCatalog,
    query: &str,
    locale: tact_catalog::CatalogLocale,
) -> Vec<&'a CommandDescriptor> {
    let mut seen = std::collections::HashSet::new();
    catalog
        .search(query, locale)
        .into_iter()
        .filter_map(|hit: CommandSearchHit| {
            let descriptor = catalog.get(&hit.id)?;
            seen.insert(canonical_key(&descriptor.action))
                .then_some(descriptor)
        })
        .collect()
}

/// Return every builtin and CLI action that must have a desktop route.
pub fn catalog_entries(catalog: &CommandCatalog) -> impl Iterator<Item = &CommandDescriptor> {
    catalog.entries()
}

/// Assertion helpers shared by shell tests without exposing executor internals.
pub fn cli_entry_has_setting_route(entry: CliEntry) -> bool {
    matches!(
        entry,
        CliEntry::PluginInstall
            | CliEntry::PluginUninstall
            | CliEntry::PluginUpdate
            | CliEntry::McpAdd
            | CliEntry::McpRemove
            | CliEntry::McpLogin
            | CliEntry::McpLogout
            | CliEntry::HooksTrust
            | CliEntry::HooksForget
    ) || plan(&CommandId::Cli(entry)).target == CommandTarget::Shell
}

pub fn cli_route_class(entry: CliEntry) -> CliRouteClass {
    match route_cli(entry, &CommandArguments::default()) {
        crate::commands::CommandRoute::User(_) | crate::commands::CommandRoute::Extension(_) => {
            CliRouteClass::Host
        }
        crate::commands::CommandRoute::CliProcess(_) => CliRouteClass::Process,
        _ => CliRouteClass::Shell,
    }
}

/// Validate the Clap CLI tree and flags against the shared desktop catalog.
///
/// The TUI's slash tree is already asserted against this catalog in its own
/// test, so this is the remaining source in the three-source parity contract.
pub fn cli_catalog_parity_errors() -> Vec<String> {
    use std::collections::{BTreeSet, HashMap};

    let command = CliArgs::command();
    let mut clap_paths = BTreeSet::new();
    collect_clap_paths(&command, &mut Vec::new(), &mut clap_paths);

    let mut errors = Vec::new();
    for entry in CliEntry::ALL {
        if matches!(entry, CliEntry::ListSessions | CliEntry::ResumeLast) {
            continue;
        }
        let path = match entry {
            CliEntry::UpgradeCheck | CliEntry::UpgradeInstall => "upgrade".to_string(),
            other => other.as_str().to_string(),
        };
        if !clap_paths.contains(&path) {
            errors.push(format!("CliEntry::{entry:?} has no Clap path `{path}`"));
        }
    }

    let expected_flags = [
        ("config", "config-path"),
        ("provider", "provider"),
        ("model", "model"),
        ("api-key", "api-key"),
        ("base-url", "base-url"),
        ("max-tokens", "max-tokens"),
        ("thinking-budget", "thinking-budget"),
        ("permission-mode", "permission-mode"),
        ("session", "navigation"),
        ("resume-last", "navigation"),
        ("list-sessions", "cli"),
        ("notifications", "notifications"),
        ("no-notifications", "notifications"),
        ("model-context-window", "model-context-window"),
        ("theme", "theme"),
        ("snapshot-max-items", "snapshot-max-items"),
        ("no-micro-compact", "micro-compact"),
        ("tokio-console", "tokio-console"),
        ("skill-body-auto-inject", "skill-body-auto-inject"),
    ];
    let expected: HashMap<_, _> = expected_flags.into_iter().collect();
    let actual = command
        .get_arguments()
        .filter_map(|argument| argument.get_long())
        .filter(|flag| !matches!(*flag, "help" | "version"))
        .collect::<BTreeSet<_>>();
    for flag in &actual {
        if !expected.contains_key(flag) {
            errors.push(format!("CLI flag `--{flag}` has no desktop route"));
        }
    }
    for flag in expected.keys() {
        if !actual.contains(flag) {
            errors.push(format!("catalog expects missing CLI flag `--{flag}`"));
        }
    }

    for setting in SettingId::ALL {
        if tact_catalog::builtins()
            .get(&CommandId::Setting(*setting))
            .is_none()
        {
            errors.push(format!("SettingId::{setting:?} has no catalog entry"));
        }
    }
    errors
}

fn collect_clap_paths(
    command: &clap::Command,
    prefix: &mut Vec<String>,
    paths: &mut std::collections::BTreeSet<String>,
) {
    for subcommand in command.get_subcommands() {
        prefix.push(subcommand.get_name().to_string());
        paths.insert(prefix.join(" "));
        for alias in subcommand.get_visible_aliases() {
            let mut alias_path = prefix.clone();
            alias_path.pop();
            alias_path.push(alias.to_string());
            paths.insert(alias_path.join(" "));
        }
        collect_clap_paths(subcommand, prefix, paths);
        prefix.pop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tact_catalog::{ComposerAction, builtins};
    use tact_protocol::UserCommand;

    #[test]
    fn every_catalog_entry_has_a_desktop_route() {
        for descriptor in catalog_entries(builtins()) {
            // `CommandTarget` has exactly these three variants, so matching on
            // them is a tautology. Ask the router instead: a `Noop` is the
            // visible-but-inert entry this test must reject.
            assert!(
                command_is_executable(&descriptor.id),
                "no executable desktop route for {:?}",
                descriptor.path
            );
            let plan = plan(&descriptor.id);
            assert!(
                matches!(
                    plan.target,
                    CommandTarget::Controller | CommandTarget::Shell | CommandTarget::Window
                ),
                "plan for {:?} has no target",
                descriptor.path
            );
        }
    }

    #[test]
    fn nested_slash_paths_complete_and_execute() {
        let catalog = builtins();
        let root = CommandId::Builtin(BuiltinCommand::Mcp);
        for child in catalog.children(&root) {
            let child_plan = plan(&child.id);
            assert_eq!(child_plan.target, CommandTarget::Controller);
            for grandchild in catalog.children(&child.id) {
                assert_eq!(plan(&grandchild.id).target, CommandTarget::Controller);
            }
        }
    }

    #[test]
    fn free_form_arguments_open_forms() {
        let id = CommandId::BuiltinPath {
            root: BuiltinCommand::Goal,
            path: vec!["goal", "create"],
        };
        assert!(matches!(form_for(&id), CommandForm::Text { .. }));
        assert_eq!(
            plan(&CommandId::Setting(SettingId::Model)).form,
            CommandForm::Text {
                name: "model".into(),
                required: true
            }
        );
    }

    #[test]
    fn command_availability_disables_unavailable_actions() {
        let descriptor = builtins()
            .get(&CommandId::Builtin(BuiltinCommand::Balance))
            .unwrap();
        assert!(!is_enabled(descriptor, CommandAvailability::default()));
        assert!(is_enabled(
            descriptor,
            CommandAvailability {
                has_account: true,
                ..Default::default()
            }
        ));

        for id in [
            CommandId::Builtin(BuiltinCommand::Compact),
            CommandId::Builtin(BuiltinCommand::Goal),
            CommandId::Cli(CliEntry::ListSessions),
        ] {
            assert!(
                !is_enabled(builtins().get(&id).unwrap(), CommandAvailability::default()),
                "{id:?}"
            );
        }
        assert!(!is_enabled(
            builtins()
                .get(&CommandId::Composer(ComposerAction::Voice))
                .unwrap(),
            CommandAvailability {
                has_project: true,
                has_session: true,
                has_account: true,
                has_installed_skill: true,
            }
        ));
    }

    #[test]
    fn every_cli_flag_has_a_setting_route() {
        let entries = [
            CliEntry::Headless,
            CliEntry::UpgradeCheck,
            CliEntry::UpgradeInstall,
            CliEntry::PluginList,
            CliEntry::PluginInstall,
            CliEntry::PluginUninstall,
            CliEntry::PluginUpdate,
            CliEntry::PluginReload,
            CliEntry::MarketplaceAdd,
            CliEntry::MarketplaceList,
            CliEntry::MarketplaceUpdate,
            CliEntry::MarketplaceRemove,
            CliEntry::McpList,
            CliEntry::McpGet,
            CliEntry::McpAdd,
            CliEntry::McpRemove,
            CliEntry::McpLogin,
            CliEntry::McpLogout,
            CliEntry::HooksList,
            CliEntry::HooksTrust,
            CliEntry::HooksForget,
            CliEntry::ListSessions,
            CliEntry::ResumeLast,
        ];
        for entry in entries {
            assert!(cli_entry_has_setting_route(entry), "{entry:?}");
        }
    }

    #[test]
    fn search_returns_one_hit_per_canonical_action() {
        let hits = search(builtins(), "mcp", tact_catalog::CatalogLocale::En);
        let mut keys = std::collections::HashSet::new();
        for descriptor in hits {
            assert!(keys.insert(canonical_key(&descriptor.action)));
        }
        assert!(!search(builtins(), "/", tact_catalog::CatalogLocale::En).is_empty());
        assert_eq!(
            plan(&CommandId::Composer(ComposerAction::Voice)).target,
            CommandTarget::Shell
        );
    }

    #[test]
    fn hooks_forget_search_exposes_the_executable_all_action() {
        let hits = search(
            tact_catalog::builtins(),
            "forget hook",
            tact_catalog::CatalogLocale::En,
        );
        assert!(hits.iter().any(|hit| {
            matches!(
                &hit.id,
                CommandId::BuiltinPath { path, .. }
                    if path.as_slice() == ["hooks", "forget", "--all"]
            )
        }));
    }

    #[test]
    fn upgrade_check_and_install_are_distinct_actions() {
        let check = builtins()
            .get(&CommandId::Cli(CliEntry::UpgradeCheck))
            .unwrap();
        let install = builtins()
            .get(&CommandId::Cli(CliEntry::UpgradeInstall))
            .unwrap();
        assert_ne!(canonical_key(&check.action), canonical_key(&install.action));
    }

    #[test]
    fn mcp_auth_aliases_share_one_canonical_action() {
        let auth = builtins().resolve_path(&["mcp", "auth"]).expect("auth");
        let login = builtins().resolve_path(&["mcp", "login"]).expect("login");
        assert_eq!(canonical_key(&auth.action), canonical_key(&login.action));
    }

    #[test]
    fn command_search_uses_the_requested_locale() {
        let hits = search(builtins(), "模型", tact_catalog::CatalogLocale::Zh);
        assert!(hits.iter().any(|entry| entry.path == ["model"]));
    }

    #[test]
    fn cli_commands_keep_arguments_and_are_executable() {
        let mcp = route_cli(
            CliEntry::McpGet,
            &CommandArguments {
                tokens: vec!["deepwiki".into()],
            },
        );
        assert!(matches!(
            mcp,
            crate::commands::CommandRoute::CliProcess(ref args)
                if args.as_slice() == ["mcp", "get", "deepwiki"]
        ));
        let headless = route_cli(
            CliEntry::Headless,
            &CommandArguments {
                tokens: vec!["summarize".into(), "repo".into()],
            },
        );
        assert!(matches!(
            headless,
            crate::commands::CommandRoute::CliProcess(ref args)
                if args.as_slice() == ["headless", "summarize repo"]
        ));
        let plugin = route_cli(
            CliEntry::PluginInstall,
            &CommandArguments {
                tokens: vec!["demo@official".into()],
            },
        );
        assert!(matches!(
            plugin,
            crate::commands::CommandRoute::Extension(
                tact_host::PluginAction::Install { spec }
            ) if spec == "demo@official"
        ));
        let hooks = route_cli(
            CliEntry::HooksTrust,
            &CommandArguments {
                tokens: vec!["source-a".into()],
            },
        );
        assert!(matches!(
            hooks,
            crate::commands::CommandRoute::User(UserCommand::HooksTrust {
                all: false,
                source: Some(ref source),
            }) if source == "source-a"
        ));
    }
}
