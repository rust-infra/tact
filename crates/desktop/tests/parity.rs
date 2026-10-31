use tact_catalog::{
    BuiltinCommand, CliEntry, CommandId, ComposerAction, SettingId, Subcommand, builtins,
};
use tact_desktop_core::{
    CliRouteClass, cli_catalog_parity_errors, cli_entry_has_setting_route, cli_route_class,
    command_is_executable, search_commands,
};

const TUI_SUBCOMMAND_TREE: &str = include_str!("fixtures/tui-subcommands.txt");

fn assert_routed(id: CommandId) {
    assert!(
        command_is_executable(&id),
        "desktop route missing for {id:?}"
    );
}

fn walk_subcommands(
    root: BuiltinCommand,
    prefix: &mut Vec<&'static str>,
    commands: &'static [Subcommand],
) {
    for command in commands {
        prefix.push(command.name);
        let id = CommandId::BuiltinPath {
            root,
            path: prefix.clone(),
        };
        assert_routed(id);
        walk_subcommands(root, prefix, command.children);
        prefix.pop();
    }
}

#[test]
fn all_23_builtin_slash_commands_have_desktop_routes() {
    assert_eq!(BuiltinCommand::ALL.len(), 23);
    for command in BuiltinCommand::ALL {
        assert_routed(CommandId::Builtin(*command));
    }
}

#[test]
fn all_declared_subcommands_have_desktop_routes() {
    for root in BuiltinCommand::ALL {
        let mut prefix = vec![root.name()];
        walk_subcommands(*root, &mut prefix, root.subcommands());
    }
}

#[test]
fn all_cli_management_entries_have_desktop_routes() {
    for entry in CliEntry::ALL {
        assert!(cli_entry_has_setting_route(*entry), "{entry:?}");
        let class = cli_route_class(*entry);
        if matches!(
            *entry,
            CliEntry::Headless | CliEntry::UpgradeCheck | CliEntry::UpgradeInstall
        ) {
            assert_eq!(class, CliRouteClass::Process, "{entry:?}");
        }
    }
}

#[test]
fn all_settings_flags_have_desktop_routes() {
    for setting in SettingId::ALL {
        assert_routed(CommandId::Setting(*setting));
    }
}

#[test]
fn all_composer_entries_have_desktop_routes() {
    for action in ComposerAction::ALL {
        assert_routed(CommandId::Composer(*action));
    }
}

#[test]
fn executable_routes_do_not_treat_undeclared_errors_as_actions() {
    let undeclared = CommandId::BuiltinPath {
        root: BuiltinCommand::Hooks,
        path: vec!["hooks", "forget", "not-a-command"],
    };
    assert!(
        !command_is_executable(&undeclared),
        "an undeclared error must not be advertised as an executable action"
    );
}

#[test]
fn cli_tree_and_flags_match_command_catalog() {
    let errors = cli_catalog_parity_errors();
    assert!(errors.is_empty(), "{errors:#?}");
    let catalog = builtins();
    assert_eq!(
        catalog
            .entries()
            .filter(|entry| matches!(entry.id, CommandId::Builtin(BuiltinCommand::ModelSubagent)))
            .count(),
        1
    );
    assert_eq!(
        catalog
            .entries()
            .filter(|entry| matches!(entry.id, CommandId::Cli(CliEntry::ListSessions)))
            .count(),
        1
    );
    assert_eq!(
        catalog
            .entries()
            .filter(|entry| matches!(entry.id, CommandId::Setting(SettingId::ConfigPath)))
            .count(),
        1
    );
    assert_eq!(
        catalog
            .entries()
            .filter(|entry| matches!(entry.id, CommandId::Composer(ComposerAction::Voice)))
            .count(),
        1
    );
}

#[test]
fn nested_command_search_keeps_distinct_actions() {
    let hits = search_commands(builtins(), "goal", tact_catalog::CatalogLocale::En);
    let paths = hits
        .iter()
        .map(|entry| entry.path.join(" "))
        .collect::<Vec<_>>();
    assert!(paths.iter().any(|path| path == "goal create"));
    assert!(paths.iter().any(|path| path == "goal show"));
}

#[test]
fn goal_list_exposes_the_archived_scope_verb() {
    let catalog = builtins();
    let all = catalog
        .resolve_path(&["goal", "list", "--all"])
        .expect("goal list --all is part of the declared tree");
    assert_eq!(all.path.join(" "), "goal list --all");
}

#[test]
fn tui_tree_and_catalog_agree_without_sharing_the_oracle() {
    use std::collections::BTreeSet;

    // The fixture is a checked-in snapshot of the TUI completion tree taken
    // before the catalog migration. It is not generated from the catalog, so a
    // catalog-only rename or addition cannot silently rewrite the oracle. The
    // TUI's own `tui_subcommand_completion_matches_catalog_tree` test closes
    // the other half of the chain (TUI tree == catalog).

    let catalog = builtins()
        .entries()
        .filter(|entry| matches!(entry.id, CommandId::BuiltinPath { .. }))
        .map(|entry| entry.path.join(" "))
        .collect::<BTreeSet<_>>();
    let tui_lines = TUI_SUBCOMMAND_TREE
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    assert_eq!(
        tui_lines.len(),
        tui_lines.iter().collect::<BTreeSet<_>>().len(),
        "the TUI fixture contains a duplicate subcommand path"
    );
    let tui = tui_lines
        .into_iter()
        .map(str::to_string)
        .collect::<BTreeSet<_>>();
    assert_eq!(catalog, tui);
}
