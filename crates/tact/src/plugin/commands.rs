//! Slash commands installed from marketplace packages.
//!
//! A package may ship `commands/<name>.md`. The front matter carries a
//! `description`; the body *is* the prompt the command expands to, which is how
//! Claude-Code-style custom commands work. Those files were already installed and
//! counted — `InstalledPlugin::command_count` — but nothing read them, so an
//! installed command was silently inert.
//!
//! They reach the runtime as a [`CommandSource`] because that is what they are:
//! discovered at runtime, refreshed when the installed set changes, and filtered by
//! the same per-command rule as a built-in.

use std::sync::Arc;

use async_trait::async_trait;
use tact_kernel::capability::command::{
    Command, CommandInvocation, CommandMeta, CommandOutcome, CommandScope, CommandSource,
};

/// What a package's command file declares.
#[derive(Debug, Default, serde::Deserialize)]
struct CommandFrontmatter {
    #[serde(default)]
    description: String,
}

/// One installed command, ready to be run.
pub struct InstalledCommand {
    meta: CommandMeta,
    /// The file body: the prompt this command expands to.
    body: String,
    submit: Arc<dyn Fn(String) + Send + Sync>,
}

impl Command for InstalledCommand {
    fn meta(&self) -> &CommandMeta {
        &self.meta
    }

    fn run(&self, _invocation: &CommandInvocation) -> CommandOutcome {
        // Expanded, not executed: the body becomes the user's turn, which is why
        // this hands it to the queue the driver drains instead of returning it as
        // text the user would have to copy.
        (self.submit)(self.body.clone());
        CommandOutcome::Message(format!("/{} queued", self.meta.name))
    }
}

/// A discovered command file, before it becomes a runnable command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandFile {
    pub name: String,
    pub description: String,
    pub body: String,
}

/// Split a command file into its front matter and body.
///
/// Missing front matter is not an error: a file that is only a body is a valid
/// command with no description, which is what the skill and memory parsers do with
/// the same shape.
fn parse(text: &str) -> (String, String) {
    let text = text.replace("\r\n", "\n");
    let Some(rest) = text.strip_prefix("---\n") else {
        return (String::new(), text.trim().to_string());
    };
    let Some((frontmatter, body)) = rest.split_once("\n---\n") else {
        return (String::new(), text.trim().to_string());
    };
    let meta: CommandFrontmatter = serde_yaml::from_str(frontmatter).unwrap_or_default();
    (meta.description.trim().to_string(), body.trim().to_string())
}

/// Every `commands/<name>.md` under `root`.
///
/// Sorted by name so two runs list the palette identically, and a file that cannot
/// be read is skipped rather than failing the whole source: one broken package must
/// not hide the commands of the others.
pub fn discover(root: &std::path::Path) -> Vec<CommandFile> {
    let dir = root.join("commands");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut found: Vec<CommandFile> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("md") {
                return None;
            }
            let name = path.file_stem()?.to_str()?.to_string();
            let text = std::fs::read_to_string(&path).ok()?;
            let (description, body) = parse(&text);
            if body.is_empty() {
                // A command with no body expands to nothing; listing it would offer
                // the user a button that submits an empty turn.
                return None;
            }
            Some(CommandFile {
                name,
                description,
                body,
            })
        })
        .collect();
    found.sort_by(|left, right| left.name.cmp(&right.name));
    found
}

/// The commands every installed package ships.
pub struct PluginCommandSource {
    files: Vec<CommandFile>,
    submit: Arc<dyn Fn(String) + Send + Sync>,
}

impl PluginCommandSource {
    /// Build a source over the plugins `store` reports as installed.
    pub fn discover(
        store: &crate::plugin::store::PluginStore,
        submit: Arc<dyn Fn(String) + Send + Sync>,
    ) -> Self {
        let mut files = Vec::new();
        if let Ok(roots) = store.installed_plugin_roots() {
            for root in roots {
                files.extend(discover(&root.root));
            }
        }
        files.sort_by(|left, right| left.name.cmp(&right.name));
        // Two packages shipping `commit.md` cannot both be `/commit`: the first
        // wins by name order, and the duplicate is dropped rather than shadowing a
        // built-in or making the listing depend on filesystem order.
        files.dedup_by(|later, earlier| later.name == earlier.name);
        Self { files, submit }
    }
}

#[async_trait]
impl CommandSource for PluginCommandSource {
    fn id(&self) -> &str {
        "marketplace"
    }

    async fn refresh(&self) -> anyhow::Result<bool> {
        // The installed set only changes through an install or a removal, and the
        // driver rebuilds this source after either; there is nothing to poll.
        Ok(false)
    }

    fn commands(&self) -> Vec<Arc<dyn Command>> {
        self.files
            .iter()
            .map(|file| {
                Arc::new(InstalledCommand {
                    meta: CommandMeta {
                        name: file.name.clone(),
                        aliases: Vec::new(),
                        summary: file.description.clone(),
                        usage: String::new(),
                        scope: CommandScope::Runtime,
                        needs_args: false,
                    },
                    body: file.body.clone(),
                    submit: self.submit.clone(),
                }) as Arc<dyn Command>
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &std::path::Path, name: &str, text: &str) {
        let commands = dir.join("commands");
        std::fs::create_dir_all(&commands).unwrap();
        std::fs::write(commands.join(name), text).unwrap();
    }

    #[test]
    fn a_command_file_splits_frontmatter_from_body() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "commit.md",
            "---\ndescription: Commit helper\n---\nWrite a commit message.\n",
        );
        let found = discover(dir.path());
        assert_eq!(
            found,
            vec![CommandFile {
                name: "commit".to_string(),
                description: "Commit helper".to_string(),
                body: "Write a commit message.".to_string(),
            }]
        );
    }

    #[test]
    fn a_body_without_frontmatter_is_still_a_command() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "review.md", "Review the diff.\n");
        let found = discover(dir.path());
        assert_eq!(found[0].description, "");
        assert_eq!(found[0].body, "Review the diff.");
    }

    #[test]
    fn a_command_with_no_body_is_not_offered() {
        // Otherwise the palette would list a button that submits an empty turn.
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "empty.md", "---\ndescription: nothing\n---\n");
        assert!(discover(dir.path()).is_empty());
    }

    #[test]
    fn discovery_is_sorted_and_ignores_non_markdown() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "zeta.md", "z");
        write(dir.path(), "alpha.md", "a");
        write(dir.path(), "notes.txt", "not a command");
        let names: Vec<String> = discover(dir.path()).into_iter().map(|f| f.name).collect();
        assert_eq!(names, vec!["alpha".to_string(), "zeta".to_string()]);
    }

    #[tokio::test]
    async fn running_a_command_submits_its_body() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "commit.md",
            "---\ndescription: d\n---\nWrite a message.",
        );
        let sent = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let sink = sent.clone();
        let source = {
            let mut files = discover(dir.path());
            files.sort_by(|l, r| l.name.cmp(&r.name));
            PluginCommandSource {
                files,
                submit: Arc::new(move |body: String| {
                    sink.lock().unwrap().push(body);
                }),
            }
        };

        let commands = source.commands();
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].meta().name, "commit");
        let outcome = commands[0].run(&CommandInvocation {
            name: "commit".to_string(),
            args: String::new(),
        });
        assert!(matches!(outcome, CommandOutcome::Message(_)));
        assert_eq!(
            sent.lock().unwrap().as_slice(),
            ["Write a message.".to_string()],
            "the body is what the model must receive"
        );
    }
}
