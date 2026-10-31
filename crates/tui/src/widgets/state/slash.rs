//! Terminal adapter for the shared Tact command catalog.
//!
//! Command names, ordering, subcommand trees, argument requirements, and
//! localized descriptions live in `tact_catalog`. This module only maps the
//! terminal message bundle onto that shared metadata.

pub(crate) use tact_catalog::{BuiltinCommand as SlashCommand, Subcommand};

/// Map the terminal locale vocabulary to the frontend-neutral catalog locale.
pub(crate) fn catalog_locale(
    language: agent_tui_kit::i18n::Language,
) -> tact_catalog::CatalogLocale {
    match language {
        agent_tui_kit::i18n::Language::English => tact_catalog::CatalogLocale::En,
        agent_tui_kit::i18n::Language::Chinese => tact_catalog::CatalogLocale::Zh,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use strum::IntoEnumIterator;

    use super::*;
    use crate::handlers::execute_palette_command;
    use crate::test_fixtures::TestApp;
    use crate::widgets::state::Status;

    /// The wire names are user-facing and some are irregular, so pin the ones a
    /// derived-name refactor would silently change.
    #[test]
    fn names_are_the_strings_users_type() {
        assert_eq!(SlashCommand::ModelSubagent.name(), "model-subagent");
        assert_eq!(SlashCommand::SubagentCancel.name(), "subagent_cancel");
        assert_eq!(SlashCommand::ViewSystemPrompt.name(), "view-system-prompt");
        assert_eq!(SlashCommand::TasksDag.name(), "tasks-dag");
    }

    #[test]
    fn every_variant_is_listed_in_all() {
        let listed: HashSet<SlashCommand> = SlashCommand::ALL.iter().copied().collect();
        for cmd in SlashCommand::iter() {
            assert!(listed.contains(&cmd), "{cmd:?} is missing from ALL");
        }
        assert_eq!(
            SlashCommand::ALL.len(),
            SlashCommand::iter().count(),
            "ALL lists a variant twice"
        );
    }

    #[test]
    fn from_name_round_trips_every_variant() {
        for cmd in SlashCommand::iter() {
            assert_eq!(SlashCommand::from_name(cmd.name()), Some(cmd));
        }
    }

    #[test]
    fn from_name_rejects_a_non_command() {
        assert_eq!(SlashCommand::from_name("nope"), None);
        assert_eq!(SlashCommand::from_name(""), None);
        assert_eq!(SlashCommand::from_name("code-reviewer"), None);
    }

    #[test]
    fn every_command_has_a_real_description_in_both_languages() {
        use agent_tui_kit::i18n::{Language, Messages};

        for lang in Language::all() {
            let msgs = Messages::by_language(*lang);
            let locale = catalog_locale(*lang);
            for cmd in SlashCommand::iter() {
                let desc = cmd.description().get(locale);
                assert!(!desc.trim().is_empty(), "{cmd:?} has an empty description");
                assert_ne!(
                    desc,
                    cmd.name(),
                    "{cmd:?} falls back to its own name in {lang:?}"
                );
                assert_eq!(
                    desc,
                    terminal_cmd_desc(cmd, msgs),
                    "{cmd:?} drifted from the terminal message bundle in {lang:?}"
                );
            }
        }
    }

    #[test]
    fn descriptions_are_not_shared_between_languages() {
        use agent_tui_kit::i18n::{Language, Messages};

        let en = Messages::by_language(Language::English);
        let zh = Messages::by_language(Language::Chinese);
        for cmd in SlashCommand::iter() {
            let (en_desc, zh_desc) = (
                cmd.description().get(tact_catalog::CatalogLocale::En),
                cmd.description().get(tact_catalog::CatalogLocale::Zh),
            );
            assert_ne!(
                en_desc, zh_desc,
                "{cmd:?} has the same description in both languages: {en_desc:?}"
            );
            assert_eq!(en_desc, terminal_cmd_desc(cmd, en));
            assert_eq!(zh_desc, terminal_cmd_desc(cmd, zh));
        }
    }

    fn terminal_cmd_desc(cmd: SlashCommand, msgs: agent_tui_kit::i18n::Messages) -> &'static str {
        match cmd {
            SlashCommand::Theme => msgs.cmd_theme,
            SlashCommand::Model => msgs.cmd_model,
            SlashCommand::ModelSubagent => msgs.cmd_model_subagent,
            SlashCommand::Permission => msgs.cmd_permission,
            SlashCommand::ViewSystemPrompt => msgs.cmd_view_system_prompt,
            SlashCommand::Save => msgs.cmd_save,
            SlashCommand::Compact => msgs.cmd_compact,
            SlashCommand::Cancel => msgs.cmd_cancel,
            SlashCommand::SubagentCancel => msgs.cmd_subagent_cancel,
            SlashCommand::Quit => msgs.cmd_quit,
            SlashCommand::Help => msgs.cmd_help,
            SlashCommand::History => msgs.cmd_history,
            SlashCommand::Skill => msgs.cmd_skill,
            SlashCommand::Plugin => msgs.cmd_plugin,
            SlashCommand::Mcp => msgs.cmd_mcp,
            SlashCommand::Hooks => msgs.cmd_hooks,
            SlashCommand::HookOutput => msgs.cmd_hook_output,
            SlashCommand::Balance => msgs.cmd_balance,
            SlashCommand::Lang => msgs.cmd_lang,
            SlashCommand::Stats => msgs.cmd_stats,
            SlashCommand::TasksDag => msgs.cmd_tasks_dag,
            SlashCommand::Background => msgs.cmd_background,
            SlashCommand::Goal => msgs.cmd_goal,
        }
    }

    #[test]
    fn only_the_commands_that_take_arguments_say_so() {
        let with_args: Vec<&str> = SlashCommand::iter()
            .filter(|c| c.needs_args())
            .map(SlashCommand::name)
            .collect();
        assert_eq!(
            with_args,
            vec!["subagent_cancel", "skill", "plugin", "mcp", "hooks", "goal"]
        );
    }

    #[test]
    fn only_balance_needs_an_account() {
        let gated: Vec<&str> = SlashCommand::iter()
            .filter(|c| c.needs_account())
            .map(SlashCommand::name)
            .collect();
        assert_eq!(gated, vec!["balance"]);
    }

    #[test]
    fn language_maps_to_catalog_locale() {
        use agent_tui_kit::i18n::Language;

        assert_eq!(
            catalog_locale(Language::English),
            tact_catalog::CatalogLocale::En
        );
        assert_eq!(
            catalog_locale(Language::Chinese),
            tact_catalog::CatalogLocale::Zh
        );
    }

    #[test]
    fn tui_dispatch_is_exhaustive_over_builtin_catalog() {
        for command in SlashCommand::iter() {
            let (mut app, _rx) = TestApp::new().into_commands();
            let (_tx, account_rx) = tokio::sync::mpsc::unbounded_channel();
            app.account_rx = Some(account_rx);
            if command == SlashCommand::Cancel {
                app.status = Status::Planning;
            }
            let outcome = execute_palette_command(&mut app, command.name());
            assert!(
                outcome.handled,
                "catalog command `{}` has no TUI dispatch",
                command.name()
            );
        }
    }

    #[test]
    fn tui_subcommand_completion_matches_catalog_tree() {
        fn walk(
            catalog: &tact_catalog::CommandCatalog,
            parent: &tact_catalog::CommandId,
            declared: &[Subcommand],
        ) {
            let actual: Vec<_> = catalog.children(parent).collect();
            assert_eq!(actual.len(), declared.len());
            for (descriptor, child) in actual.into_iter().zip(declared) {
                assert_eq!(descriptor.path.last().map(String::as_str), Some(child.name));
                walk(catalog, &descriptor.id, child.children);
            }
        }

        let catalog = tact_catalog::builtins();
        for command in SlashCommand::iter() {
            walk(
                catalog,
                &tact_catalog::CommandId::Builtin(command),
                command.subcommands(),
            );
        }
    }
}
