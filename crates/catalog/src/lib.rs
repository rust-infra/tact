//! Frontend-neutral command metadata for Tact.

mod builtin;
mod catalog;
mod cli;
mod fuzzy;
mod localized;
mod settings;

pub use builtin::{BuiltinCommand, Subcommand};
pub use catalog::{
    CommandCatalog, CommandDescriptor, CommandId, CommandSearchHit, builtins, canonical_action_key,
};
pub use cli::{
    ArgumentSpec, ArgumentValue, Availability, ChoiceSource, CliEntry, CommandAction,
    CommandArguments, CommandGroup, DisabledReason, FixedChoiceSet, Shortcut,
};
pub use fuzzy::fuzzy_score;
pub use localized::{CatalogLocale, LocalizedText};
pub use settings::{ComposerAction, SettingId};
