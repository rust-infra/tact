//! The built-in slash commands, as one enum.
//!
//! Before this module the same 22 names were listed in five parallel places,
//! and two of them had already drifted:
//!
//! | Where | What it held | State |
//! |---|---|---|
//! | `PALETTE_COMMANDS` | 22 × (name, English description) | the description column was **dead** — no caller read it |
//! | `localize_cmd_desc` | the live description, per language | **6 commands missing**, so they displayed their own name as the description in *both* languages |
//! | `execute_palette_command` | the handler | complete, but a missing arm fell into `_ => handled: false` silently |
//! | `command_needs_args` | 4 names | a fourth list |
//! | `i18n::Messages` | the descriptions themselves | 17 `cmd_*` fields for 22 commands |
//!
//! The enum removes the drift by making each of those an exhaustive match over
//! the variants. Adding a command now fails to compile until it has a name, a
//! description in both languages, and a handler.
//!
//! Deliberately **not** here: argument and subcommand parsing. `/mcp auth <server>`,
//! `/plugin marketplace list` and `/hooks trust --all` still parse strings in
//! their own handler modules; that is a separate abstraction with an
//! independent payoff.

use agent_tui_kit::i18n::Messages;
use strum::{EnumIter, EnumString, IntoStaticStr};

/// A built-in slash command.
///
/// The `serialize` string is the name the user types and the popup shows — it
/// is spelled out per variant rather than derived, because the set is not
/// uniform: `model-subagent` uses hyphens and `subagent_cancel` uses an
/// underscore, and deriving either from the variant name would silently rename
/// a command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EnumIter, EnumString, IntoStaticStr)]
pub(crate) enum SlashCommand {
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
    #[strum(serialize = "skills")]
    Skills,
    #[strum(serialize = "skill-reload")]
    SkillReload,
    #[strum(serialize = "plugin")]
    Plugin,
    #[strum(serialize = "mcp")]
    Mcp,
    #[strum(serialize = "hooks")]
    Hooks,
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
}

impl SlashCommand {
    /// Popup order, which is the order the user sees and the tie-break order
    /// for equal fuzzy scores. `every_variant_is_listed_in_all` keeps it
    /// complete; dispatch does not depend on it (a name resolves through
    /// `from_name` whether or not it is listed), so a forgotten entry costs
    /// discoverability rather than function — which is exactly the kind of
    /// quiet failure the test is there to prevent.
    pub(crate) const ALL: &'static [Self] = &[
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
        Self::Skills,
        Self::SkillReload,
        Self::Plugin,
        Self::Mcp,
        Self::Hooks,
        Self::Balance,
        Self::Lang,
        Self::Stats,
        Self::TasksDag,
        Self::Background,
    ];

    /// The name the user types, without the leading `/`.
    #[must_use]
    pub(crate) fn name(self) -> &'static str {
        self.into()
    }

    /// Resolve a user-typed name (no leading `/`).
    ///
    /// Case-sensitive and exact, which is what the input box produces; it is
    /// the same comparison `is_builtin_palette_command` used to do against the
    /// table.
    #[must_use]
    pub(crate) fn from_name(name: &str) -> Option<Self> {
        name.parse().ok()
    }

    /// Whether Enter should autocomplete `/{name} ` instead of executing, for
    /// commands that take a subcommand or an argument.
    #[must_use]
    pub(crate) fn needs_args(self) -> bool {
        matches!(
            self,
            Self::Plugin | Self::Mcp | Self::Hooks | Self::SubagentCancel
        )
    }

    /// Whether the command is only useful with an account channel — `/balance`
    /// queries the provider's balance endpoint and is hidden without one.
    #[must_use]
    pub(crate) fn needs_account(self) -> bool {
        matches!(self, Self::Balance)
    }

    /// The localized one-line description shown in the popup and the help panel.
    ///
    /// Exhaustive on purpose: this match had a `_ =>` arm before, and that is
    /// how six commands came to display their own name as their description in
    /// both languages.
    #[must_use]
    pub(crate) fn desc(self, msgs: Messages) -> &'static str {
        match self {
            Self::Theme => msgs.cmd_theme,
            Self::Model => msgs.cmd_model,
            Self::ModelSubagent => msgs.cmd_model_subagent,
            Self::Permission => msgs.cmd_permission,
            Self::ViewSystemPrompt => msgs.cmd_view_system_prompt,
            Self::Save => msgs.cmd_save,
            Self::Compact => msgs.cmd_compact,
            Self::Cancel => msgs.cmd_cancel,
            Self::SubagentCancel => msgs.cmd_subagent_cancel,
            Self::Quit => msgs.cmd_quit,
            Self::Help => msgs.cmd_help,
            Self::History => msgs.cmd_history,
            Self::Skills => msgs.cmd_skills,
            Self::SkillReload => msgs.cmd_skill_reload,
            Self::Plugin => msgs.cmd_plugin,
            Self::Mcp => msgs.cmd_mcp,
            Self::Hooks => msgs.cmd_hooks,
            Self::Balance => msgs.cmd_balance,
            Self::Lang => msgs.cmd_lang,
            Self::Stats => msgs.cmd_stats,
            Self::TasksDag => msgs.cmd_tasks_dag,
            Self::Background => msgs.cmd_background,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use strum::IntoEnumIterator;

    use super::*;

    /// The wire names are user-facing and some are irregular, so pin the ones a
    /// derived-name refactor would silently change.
    #[test]
    fn names_are_the_strings_users_type() {
        assert_eq!(SlashCommand::ModelSubagent.name(), "model-subagent");
        assert_eq!(SlashCommand::SubagentCancel.name(), "subagent_cancel");
        assert_eq!(SlashCommand::ViewSystemPrompt.name(), "view-system-prompt");
        assert_eq!(SlashCommand::SkillReload.name(), "skill-reload");
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
        // Not a skill name, not a builtin.
        assert_eq!(SlashCommand::from_name("code-reviewer"), None);
    }

    /// The bug this refactor came from: `localize_cmd_desc` had a `_ =>` arm
    /// that returned the command name, so six commands displayed `permission`
    /// as the description of `/permission` — in *both* languages, with nothing
    /// to catch it. An exhaustive `desc` makes a missing translation a compile
    /// error instead; this asserts the shape that survived.
    #[test]
    fn every_command_has_a_real_description_in_both_languages() {
        use agent_tui_kit::i18n::{Language, Messages};

        for lang in Language::all() {
            let msgs = Messages::by_language(*lang);
            for cmd in SlashCommand::iter() {
                let desc = cmd.desc(msgs);
                assert!(!desc.trim().is_empty(), "{cmd:?} has an empty description");
                assert_ne!(
                    desc,
                    cmd.name(),
                    "{cmd:?} falls back to its own name in {lang:?}"
                );
            }
        }
    }

    /// The descriptions are the popup's second column, so a Chinese and an
    /// English description being identical means one of them was never
    /// translated for the commands whose words differ.
    #[test]
    fn descriptions_are_not_shared_between_languages() {
        use agent_tui_kit::i18n::{Language, Messages};

        let en = Messages::by_language(Language::English);
        let zh = Messages::by_language(Language::Chinese);
        for cmd in SlashCommand::iter() {
            let (en, zh) = (cmd.desc(en), cmd.desc(zh));
            assert_ne!(
                en, zh,
                "{cmd:?} has the same description in both languages: {en:?}"
            );
        }
    }

    #[test]
    fn only_the_commands_that_take_arguments_say_so() {
        let with_args: Vec<&str> = SlashCommand::iter()
            .filter(|c| c.needs_args())
            .map(SlashCommand::name)
            .collect();
        assert_eq!(with_args, vec!["subagent_cancel", "plugin", "mcp", "hooks"]);
    }

    /// `/balance` is the only command gated on the account channel; it was a
    /// string comparison inside `palette_commands` before.
    #[test]
    fn only_balance_needs_an_account() {
        let gated: Vec<&str> = SlashCommand::iter()
            .filter(|c| c.needs_account())
            .map(SlashCommand::name)
            .collect();
        assert_eq!(gated, vec!["balance"]);
    }
}
