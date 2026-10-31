use tact_catalog::{CommandDescriptor, CommandId};
use tact_desktop_core::{CommandTarget, canonical_key, plan_command, search_commands};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandCenterItem {
    pub id: CommandId,
    pub title: String,
    pub subtitle: String,
    pub target: CommandTarget,
}

pub fn items(query: &str) -> Vec<CommandCenterItem> {
    search_commands(
        tact_catalog::builtins(),
        query,
        tact_catalog::CatalogLocale::En,
    )
    .into_iter()
    .map(center_item)
    .collect()
}

pub fn recent(items: &[CommandDescriptor]) -> Vec<CommandCenterItem> {
    items.iter().take(8).map(center_item).collect()
}

fn center_item(entry: &CommandDescriptor) -> CommandCenterItem {
    CommandCenterItem {
        id: entry.id.clone(),
        title: entry.path.join(" "),
        subtitle: entry
            .description
            .get(tact_catalog::CatalogLocale::En)
            .to_string(),
        target: plan_command(&entry.id).target,
    }
}

pub fn canonical_ids(items: &[CommandCenterItem]) -> Vec<String> {
    items
        .iter()
        .map(|item| {
            tact_catalog::builtins()
                .get(&item.id)
                .map(|entry| canonical_key(&entry.action))
                .unwrap_or_else(|| format!("{:?}", item.id))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use gpui::TestAppContext;
    use tact_catalog::BuiltinCommand;

    use super::*;

    #[gpui::test]
    fn help_and_save_entries_are_executable(_cx: &mut TestAppContext) {
        for command in [BuiltinCommand::Help, BuiltinCommand::Save] {
            let plan = plan_command(&CommandId::Builtin(command));
            assert_eq!(plan.target, CommandTarget::Controller);
        }
    }
}
