use tact_catalog::{CliEntry, CommandArguments, CommandId, ComposerAction, SettingId};
use tact_host::PluginAction;
use tact_protocol::{PermissionMode, ReasoningEffort, SurfaceQuery, UserCommand};

use crate::{ReaderRef, SettingValue};

#[derive(Debug)]
pub(crate) enum CommandRoute {
    User(UserCommand),
    Surface(SurfaceQuery),
    Extension(PluginAction),
    Reader(ReaderRef),
    Skill { name: String, args: String },
    CliProcess(Vec<String>),
    Error(String),
    Shell(CommandId),
    CloseWindow,
    Noop,
}

pub(crate) fn route_command(id: &CommandId, args: &CommandArguments) -> CommandRoute {
    match id {
        CommandId::Builtin(command) => route_builtin(*command, args),
        CommandId::BuiltinPath { root, path } => route_builtin_path(*root, path, args),
        CommandId::Cli(entry) => route_cli(*entry, args),
        CommandId::Setting(id) => CommandRoute::Shell(CommandId::Setting(*id)),
        CommandId::Composer(action) => route_composer(*action),
        CommandId::DynamicSkill(name) => CommandRoute::Skill {
            name: name.clone(),
            args: args.tokens.join(" "),
        },
    }
}

fn route_builtin(command: tact_catalog::BuiltinCommand, args: &CommandArguments) -> CommandRoute {
    use tact_catalog::BuiltinCommand as B;
    match command {
        B::Theme | B::Lang | B::Model | B::ModelSubagent | B::Permission => {
            CommandRoute::Shell(CommandId::Builtin(command))
        }
        B::ViewSystemPrompt => {
            CommandRoute::Surface(SurfaceQuery::SystemPrompt { assembled: true })
        }
        B::Save => CommandRoute::Shell(CommandId::Builtin(command)),
        B::Compact => CommandRoute::User(UserCommand::Compact),
        B::Cancel => CommandRoute::User(UserCommand::Cancel),
        B::SubagentCancel => CommandRoute::User(UserCommand::CancelSubagent {
            child_id: args.tokens.first().cloned().unwrap_or_default(),
        }),
        B::Quit => CommandRoute::CloseWindow,
        B::Help => CommandRoute::Reader(ReaderRef::Help),
        B::History => CommandRoute::Surface(SurfaceQuery::History),
        B::Skill => route_skill(args),
        B::Plugin => route_plugin(args),
        B::Mcp => route_mcp(args),
        B::Hooks => route_hooks(args),
        B::HookOutput => CommandRoute::Shell(CommandId::Builtin(command)),
        B::Balance => CommandRoute::User(UserCommand::QueryBalance),
        B::Stats => CommandRoute::Surface(SurfaceQuery::Stats),
        B::TasksDag => CommandRoute::Shell(CommandId::Builtin(command)),
        B::Background => CommandRoute::Shell(CommandId::Builtin(command)),
        B::Goal => route_goal(args),
    }
}

fn route_builtin_path(
    root: tact_catalog::BuiltinCommand,
    path: &[&'static str],
    args: &CommandArguments,
) -> CommandRoute {
    use tact_catalog::BuiltinCommand as B;
    let tail = path.strip_prefix(&[root.name()]).unwrap_or(path);
    match (root, tail) {
        (B::Mcp, ["auth" | "login", ..]) => CommandRoute::User(UserCommand::McpAuth {
            server: args.tokens.first().cloned().unwrap_or_default(),
        }),
        (B::Mcp, ["list"]) => CommandRoute::Surface(SurfaceQuery::McpServers),
        (B::Mcp, ["prompts"]) => CommandRoute::Surface(SurfaceQuery::McpPrompts {
            server: args.tokens.first().cloned(),
        }),
        (B::Mcp, ["prompt"]) => {
            let server = args.tokens.first().cloned().unwrap_or_default();
            let name = args.tokens.get(1).cloned().unwrap_or_default();
            if server.is_empty() || name.is_empty() {
                return CommandRoute::Error("mcp prompt requires a server and prompt name".into());
            }
            let mut arguments = std::collections::BTreeMap::new();
            for argument in args.tokens.get(2..).unwrap_or_default() {
                let Some((key, value)) = argument.split_once('=') else {
                    return CommandRoute::Error(format!(
                        "prompt argument `{argument}` must be key=value"
                    ));
                };
                if key.is_empty() {
                    return CommandRoute::Error(format!(
                        "prompt argument `{argument}` must have a name"
                    ));
                }
                arguments.insert(key.to_string(), value.to_string());
            }
            CommandRoute::User(UserCommand::RunMcpPrompt {
                server,
                name,
                arguments,
            })
        }
        (B::Plugin, ["marketplace", "add"]) => {
            CommandRoute::Extension(PluginAction::MarketplaceAdd {
                source: args.tokens.last().cloned().unwrap_or_default(),
            })
        }
        (B::Plugin, ["marketplace", "update"]) => {
            CommandRoute::Extension(PluginAction::MarketplaceUpdate {
                name: args.tokens.last().cloned().unwrap_or_default(),
            })
        }
        (B::Plugin, ["marketplace", "remove"]) => {
            CommandRoute::Extension(PluginAction::MarketplaceRemove {
                name: args.tokens.last().cloned().unwrap_or_default(),
            })
        }
        (B::Plugin, ["marketplace", "list"]) => CommandRoute::Shell(CommandId::BuiltinPath {
            root,
            path: path.to_vec(),
        }),
        (B::Plugin, ["reload"]) => CommandRoute::Extension(PluginAction::Reload),
        (B::Plugin, ["install", ..]) => {
            let spec = args.tokens.first().cloned().unwrap_or_default();
            CommandRoute::Extension(PluginAction::Install { spec })
        }
        (B::Plugin, ["uninstall", ..]) => CommandRoute::Extension(PluginAction::Uninstall {
            name: args.tokens.first().cloned().unwrap_or_default(),
        }),
        (B::Plugin, ["update", ..]) => CommandRoute::Extension(PluginAction::Update {
            name: args.tokens.first().cloned().unwrap_or_default(),
        }),
        (B::Plugin, ["list"]) => CommandRoute::Shell(CommandId::BuiltinPath {
            root,
            path: path.to_vec(),
        }),
        (B::Plugin, [..]) => route_plugin(args),
        (B::Hooks, ["list"]) => CommandRoute::Surface(SurfaceQuery::Hooks),
        (B::Hooks, ["trust", "--all"]) => CommandRoute::User(UserCommand::HooksTrust {
            all: true,
            source: None,
        }),
        (B::Hooks, ["trust", "--source", ..]) => {
            let source = (!args.tokens.is_empty()).then(|| args.tokens.join(" "));
            if source.is_none() {
                CommandRoute::Error("hooks trust --source requires a source label".into())
            } else {
                CommandRoute::User(UserCommand::HooksTrust { all: false, source })
            }
        }
        (B::Hooks, ["forget", "--all"]) => CommandRoute::User(UserCommand::HooksForget),
        (B::Hooks, ["forget", ..]) => CommandRoute::Error("hooks forget requires --all".into()),
        (B::Skill, ["list"]) | (B::Skill, ["reload"]) => {
            CommandRoute::Shell(CommandId::BuiltinPath {
                root,
                path: path.to_vec(),
            })
        }
        (B::Goal, ["list", "--all"]) => CommandRoute::User(UserCommand::ListGoals {
            include_archived: true,
        }),
        (B::Goal, [verb, ..]) => route_goal_verb(verb, args),
        _ => CommandRoute::Shell(CommandId::BuiltinPath {
            root,
            path: path.to_vec(),
        }),
    }
}

fn route_skill(args: &CommandArguments) -> CommandRoute {
    match args.tokens.first().map(String::as_str) {
        Some("list" | "reload" | "disable" | "enable") | None => {
            CommandRoute::Shell(CommandId::Builtin(tact_catalog::BuiltinCommand::Skill))
        }
        Some(name) => CommandRoute::Skill {
            name: name.to_string(),
            args: args.tokens[1..].join(" "),
        },
    }
}

fn route_plugin(args: &CommandArguments) -> CommandRoute {
    let action = match args.tokens.as_slice() {
        [verb] if verb == "reload" => PluginAction::Reload,
        [verb, name] if verb == "uninstall" => PluginAction::Uninstall { name: name.clone() },
        [verb, name] if verb == "update" => PluginAction::Update { name: name.clone() },
        [verb, source] if verb == "add" => PluginAction::MarketplaceAdd {
            source: source.clone(),
        },
        [verb, name] if verb == "remove" => PluginAction::MarketplaceRemove { name: name.clone() },
        _ => {
            return CommandRoute::Shell(CommandId::Builtin(tact_catalog::BuiltinCommand::Plugin));
        }
    };
    CommandRoute::Extension(action)
}

fn route_mcp(args: &CommandArguments) -> CommandRoute {
    match args.tokens.as_slice() {
        [] => CommandRoute::Surface(SurfaceQuery::McpServers),
        [verb, server] if verb == "auth" || verb == "login" => {
            CommandRoute::User(UserCommand::McpAuth {
                server: server.clone(),
            })
        }
        [verb] if verb == "list" => CommandRoute::Surface(SurfaceQuery::McpServers),
        [verb] if verb == "prompts" => {
            CommandRoute::Surface(SurfaceQuery::McpPrompts { server: None })
        }
        [verb, server] if verb == "prompts" => CommandRoute::Surface(SurfaceQuery::McpPrompts {
            server: Some(server.clone()),
        }),
        _ => CommandRoute::Shell(CommandId::Builtin(tact_catalog::BuiltinCommand::Mcp)),
    }
}

fn route_hooks(args: &CommandArguments) -> CommandRoute {
    match args.tokens.as_slice() {
        [] => CommandRoute::Surface(SurfaceQuery::Hooks),
        [verb] if verb == "list" => CommandRoute::Surface(SurfaceQuery::Hooks),
        [verb, flag] if verb == "trust" && flag == "--all" => {
            CommandRoute::User(UserCommand::HooksTrust {
                all: true,
                source: None,
            })
        }
        [verb, flag, source @ ..] if verb == "trust" && flag == "--source" => {
            if source.is_empty() {
                return CommandRoute::Error("hooks trust --source requires a source label".into());
            }
            CommandRoute::User(UserCommand::HooksTrust {
                all: false,
                source: Some(source.join(" ")),
            })
        }
        [verb, flag] if verb == "forget" && flag == "--all" => {
            CommandRoute::User(UserCommand::HooksForget)
        }
        [verb, ..] if verb == "forget" => CommandRoute::Error("hooks forget requires --all".into()),
        _ => CommandRoute::Shell(CommandId::Builtin(tact_catalog::BuiltinCommand::Hooks)),
    }
}

fn route_goal(args: &CommandArguments) -> CommandRoute {
    match args.tokens.first().map(String::as_str) {
        None => CommandRoute::Shell(CommandId::Builtin(tact_catalog::BuiltinCommand::Goal)),
        Some(verb) => route_goal_verb(
            verb,
            &CommandArguments {
                tokens: args.tokens[1..].to_vec(),
            },
        ),
    }
}

fn route_goal_verb(verb: &str, args: &CommandArguments) -> CommandRoute {
    let id = args.tokens.first().cloned().unwrap_or_default();
    let text = args.tokens.get(1..).unwrap_or_default().join(" ");
    CommandRoute::User(match verb {
        "create" => {
            let delimiter = args.tokens.iter().position(|token| token == "--");
            let objective_tokens = delimiter
                .map(|index| &args.tokens[..index])
                .unwrap_or(args.tokens.as_slice());
            let criteria = delimiter
                .map(|index| args.tokens[index + 1..].join(" "))
                .unwrap_or_default()
                .split(';')
                .map(str::trim)
                .filter(|criterion| !criterion.is_empty())
                .map(ToOwned::to_owned)
                .collect();
            let objective = objective_tokens.join(" ");
            UserCommand::CreateGoal {
                title: objective.clone(),
                objective,
                criteria,
            }
        }
        "list" => UserCommand::ListGoals {
            include_archived: args.tokens.iter().any(|arg| arg == "--all"),
        },
        "show" => UserCommand::ShowGoal { goal_id: id },
        "steer" => UserCommand::SteerGoal {
            goal_id: (!id.is_empty()).then_some(id),
            text,
        },
        "pause" => UserCommand::PauseGoal { goal_id: id },
        "resume" => UserCommand::ResumeGoal { goal_id: id },
        "cancel" => UserCommand::CancelGoal { goal_id: id },
        "accept" => UserCommand::AcceptGoal { goal_id: id, text },
        "archive" => UserCommand::SetGoalArchived {
            goal_id: id,
            archived: true,
        },
        "unarchive" => UserCommand::SetGoalArchived {
            goal_id: id,
            archived: false,
        },
        _ => return CommandRoute::Shell(CommandId::Builtin(tact_catalog::BuiltinCommand::Goal)),
    })
}

pub(crate) fn route_cli(entry: CliEntry, args: &CommandArguments) -> CommandRoute {
    match entry {
        CliEntry::Headless => {
            CommandRoute::CliProcess(vec!["headless".into(), args.tokens.join(" ")])
        }
        CliEntry::UpgradeCheck => {
            CommandRoute::CliProcess(vec!["upgrade".into(), "--check".into()])
        }
        CliEntry::UpgradeInstall => {
            CommandRoute::CliProcess(vec!["upgrade".into(), "--yes".into()])
        }
        CliEntry::ListSessions => CommandRoute::Shell(CommandId::Cli(entry)),
        CliEntry::ResumeLast => CommandRoute::Shell(CommandId::Cli(entry)),
        CliEntry::McpList => CommandRoute::Surface(SurfaceQuery::McpServers),
        CliEntry::McpGet => CommandRoute::CliProcess(vec![
            "mcp".into(),
            "get".into(),
            args.tokens.first().cloned().unwrap_or_default(),
        ]),
        CliEntry::McpAdd => {
            let mut command = vec![
                "mcp".into(),
                "add".into(),
                args.tokens.first().cloned().unwrap_or_default(),
            ];
            command.extend(args.tokens.iter().skip(1).cloned());
            CommandRoute::CliProcess(command)
        }
        CliEntry::McpRemove => {
            let mut command = vec![
                "mcp".into(),
                "remove".into(),
                args.tokens.first().cloned().unwrap_or_default(),
            ];
            command.extend(args.tokens.iter().skip(1).cloned());
            CommandRoute::CliProcess(command)
        }
        CliEntry::McpLogin | CliEntry::McpAuth => CommandRoute::User(UserCommand::McpAuth {
            server: args.tokens.first().cloned().unwrap_or_default(),
        }),
        CliEntry::McpLogout => CommandRoute::CliProcess(vec![
            "mcp".into(),
            "logout".into(),
            args.tokens.first().cloned().unwrap_or_default(),
        ]),
        CliEntry::HooksList => CommandRoute::Surface(SurfaceQuery::Hooks),
        CliEntry::HooksTrust => {
            if args.tokens.iter().any(|token| token == "--all") {
                CommandRoute::User(UserCommand::HooksTrust {
                    all: true,
                    source: None,
                })
            } else {
                let source = args
                    .tokens
                    .iter()
                    .position(|token| token == "--source")
                    .and_then(|index| args.tokens.get(index + 1..))
                    .filter(|tokens| !tokens.is_empty())
                    .map(|tokens| tokens.join(" "))
                    .or_else(|| (!args.tokens.is_empty()).then(|| args.tokens.join(" ")));
                let Some(source) = source else {
                    return CommandRoute::Error(
                        "hooks trust requires --all or a specific source".into(),
                    );
                };
                CommandRoute::User(UserCommand::HooksTrust {
                    all: false,
                    source: Some(source),
                })
            }
        }
        CliEntry::HooksForget => {
            if args.tokens.iter().any(|token| token == "--all") {
                CommandRoute::User(UserCommand::HooksForget)
            } else {
                CommandRoute::Error("hooks forget requires --all".into())
            }
        }
        CliEntry::PluginReload => CommandRoute::Extension(PluginAction::Reload),
        CliEntry::PluginInstall => {
            let spec = args.tokens.first().cloned().unwrap_or_default();
            CommandRoute::Extension(PluginAction::Install { spec })
        }
        CliEntry::PluginUninstall => CommandRoute::Extension(PluginAction::Uninstall {
            name: args.tokens.first().cloned().unwrap_or_default(),
        }),
        CliEntry::PluginUpdate => CommandRoute::Extension(PluginAction::Update {
            name: args.tokens.first().cloned().unwrap_or_default(),
        }),
        CliEntry::MarketplaceAdd => CommandRoute::Extension(PluginAction::MarketplaceAdd {
            source: args.tokens.first().cloned().unwrap_or_default(),
        }),
        CliEntry::MarketplaceUpdate => CommandRoute::Extension(PluginAction::MarketplaceUpdate {
            name: args.tokens.first().cloned().unwrap_or_default(),
        }),
        CliEntry::MarketplaceRemove => CommandRoute::Extension(PluginAction::MarketplaceRemove {
            name: args.tokens.first().cloned().unwrap_or_default(),
        }),
        CliEntry::PluginList | CliEntry::MarketplaceList => {
            CommandRoute::Shell(CommandId::Cli(entry))
        }
    }
}

fn route_composer(action: ComposerAction) -> CommandRoute {
    match action {
        ComposerAction::Stop => CommandRoute::User(UserCommand::Cancel),
        ComposerAction::Compact => CommandRoute::User(UserCommand::Compact),
        _ => CommandRoute::Shell(CommandId::Composer(action)),
    }
}

pub(crate) fn route_setting(id: SettingId, value: &SettingValue) -> CommandRoute {
    match (id, value) {
        (SettingId::Model, SettingValue::Text(model)) => {
            CommandRoute::User(UserCommand::SetModel(model.clone()))
        }
        (SettingId::ThinkingBudget, SettingValue::Text(value)) => {
            value.parse().map_or(CommandRoute::Noop, |budget| {
                CommandRoute::User(UserCommand::SetThinkingBudget(budget))
            })
        }
        (SettingId::ReasoningEffort, SettingValue::Choice(value)) => {
            let effort = match value.as_str() {
                "low" => Some(ReasoningEffort::Low),
                "medium" => Some(ReasoningEffort::Medium),
                "high" => Some(ReasoningEffort::High),
                "xhigh" => Some(ReasoningEffort::Xhigh),
                "max" => Some(ReasoningEffort::Max),
                _ => None,
            };
            CommandRoute::User(UserCommand::SetReasoningEffort(effort))
        }
        (SettingId::PermissionMode, SettingValue::Choice(value)) => {
            let mode = match value.as_str() {
                "plan" => PermissionMode::Plan,
                "auto" => PermissionMode::Auto,
                "default" => PermissionMode::Default,
                _ => return CommandRoute::Noop,
            };
            CommandRoute::User(UserCommand::SetPermissionMode(mode))
        }
        _ => CommandRoute::Shell(CommandId::Setting(id)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compiled(path: &[&'static str], tokens: &[&str]) -> CommandRoute {
        route_command(
            &CommandId::BuiltinPath {
                root: match path[0] {
                    "mcp" => tact_catalog::BuiltinCommand::Mcp,
                    "plugin" => tact_catalog::BuiltinCommand::Plugin,
                    "skill" => tact_catalog::BuiltinCommand::Skill,
                    "hooks" => tact_catalog::BuiltinCommand::Hooks,
                    "goal" => tact_catalog::BuiltinCommand::Goal,
                    other => panic!("unexpected root {other}"),
                },
                path: path.to_vec(),
            },
            &CommandArguments {
                tokens: tokens.iter().map(|token| (*token).to_string()).collect(),
            },
        )
    }

    #[test]
    fn compiled_subcommand_routes_strip_the_root_prefix() {
        assert!(matches!(
            compiled(&["goal", "show"], &["goal-1"]),
            CommandRoute::User(UserCommand::ShowGoal { goal_id }) if goal_id == "goal-1"
        ));
        assert!(matches!(
            compiled(&["mcp", "auth"], &["filesystem"]),
            CommandRoute::User(UserCommand::McpAuth { server }) if server == "filesystem"
        ));
        assert!(matches!(
            compiled(&["hooks", "trust", "--source"], &["repo"]),
            CommandRoute::User(UserCommand::HooksTrust {
                all: false,
                source: Some(source),
            }) if source == "repo"
        ));
        assert!(matches!(
            compiled(&["plugin", "reload"], &[]),
            CommandRoute::Extension(PluginAction::Reload)
        ));
        assert!(matches!(
            compiled(&["plugin", "uninstall"], &["example"]),
            CommandRoute::Extension(PluginAction::Uninstall { name }) if name == "example"
        ));
        assert!(matches!(
            compiled(&["mcp", "list"], &[]),
            CommandRoute::Surface(SurfaceQuery::McpServers)
        ));
        assert!(matches!(
            compiled(&["hooks", "list"], &[]),
            CommandRoute::Surface(SurfaceQuery::Hooks)
        ));
        assert!(matches!(
            compiled(&["goal", "list", "--all"], &[]),
            CommandRoute::User(UserCommand::ListGoals {
                include_archived: true
            })
        ));
    }

    #[test]
    fn dynamic_skills_route_to_registry_expansion() {
        let direct = route_command(
            &CommandId::DynamicSkill("demo".into()),
            &CommandArguments {
                tokens: vec!["fix".into(), "auth".into()],
            },
        );
        assert!(matches!(
            direct,
            CommandRoute::Skill { name, args } if name == "demo" && args == "fix auth"
        ));
        let with_args = route_command(
            &CommandId::Builtin(tact_catalog::BuiltinCommand::Skill),
            &CommandArguments {
                tokens: vec!["demo".into(), "fix auth".into()],
            },
        );
        assert!(matches!(
            with_args,
            CommandRoute::Skill { name, args } if name == "demo" && args == "fix auth"
        ));
    }

    #[test]
    fn mcp_remove_preserves_the_scope_flag() {
        assert!(matches!(
            route_cli(
                CliEntry::McpRemove,
                &CommandArguments {
                    tokens: vec!["analytics".into(), "--user".into()],
                },
            ),
            CommandRoute::CliProcess(args)
                if args == ["mcp", "remove", "analytics", "--user"]
        ));
    }

    #[test]
    fn hooks_forget_requires_explicit_all() {
        assert!(matches!(
            route_cli(CliEntry::HooksForget, &CommandArguments::default()),
            CommandRoute::Error(_)
        ));
        assert!(matches!(
            route_cli(
                CliEntry::HooksForget,
                &CommandArguments {
                    tokens: vec!["--all".into()],
                },
            ),
            CommandRoute::User(UserCommand::HooksForget)
        ));
    }

    #[test]
    fn hooks_trust_source_preserves_free_text_labels() {
        let route = compiled(&["hooks", "trust", "--source"], &["plugin", "ponytail"]);
        assert!(matches!(
            route,
            CommandRoute::User(UserCommand::HooksTrust {
                all: false,
                source: Some(source),
            }) if source == "plugin ponytail"
        ));
    }

    #[test]
    fn goal_objective_may_start_with_the_verb() {
        let route = compiled(&["goal", "create"], &["create", "release"]);
        assert!(matches!(
            route,
            CommandRoute::User(UserCommand::CreateGoal {
                title,
                objective,
                ..
            }) if title == "create release" && objective == "create release"
        ));
    }

    #[test]
    fn mcp_prompt_requires_both_server_and_name() {
        assert!(matches!(
            compiled(&["mcp", "prompt"], &["filesystem"]),
            CommandRoute::Error(_)
        ));
        assert!(matches!(
            compiled(&["mcp", "prompt"], &["filesystem", "summarize", "=bad"]),
            CommandRoute::Error(_)
        ));
    }

    #[test]
    fn goal_create_preserves_acceptance_criteria() {
        assert!(matches!(
            compiled(
                &["goal", "create"],
                &["Ship", "desktop", "--", "Windows;", "Linux", "and", "macOS"]
            ),
            CommandRoute::User(UserCommand::CreateGoal {
                title,
                objective,
                criteria,
            }) if title == "Ship desktop"
                && objective == "Ship desktop"
                && criteria == vec![
                    "Windows".to_string(),
                    "Linux and macOS".to_string()
                ]
        ));
    }
}
