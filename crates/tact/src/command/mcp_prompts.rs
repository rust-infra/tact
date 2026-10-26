//! MCP prompts as catalog commands.
//!
//! The design lists `CommandSource` as an MCP contribution: a prompt a server
//! advertises should be runnable as a slash command, not only through
//! `/mcp prompts`. Prompts are discovered at runtime, which is exactly what a
//! `CommandSource` is for.
//!
//! Listing is async and `CommandSource::commands` is not, so the source holds a
//! snapshot the cursor refreshes. The backend reads client handles rather than the
//! agent, so a refresh never touches the agent slot — the lock the command loop
//! holds while it runs a command, and the one a self-refresh would deadlock on.

use std::sync::Arc;

use async_trait::async_trait;
use tact_kernel::capability::CapabilityId;
use tact_kernel::capability::CapabilityKind;
use tact_kernel::capability::command::{
    Command, CommandInvocation, CommandMeta, CommandOutcome, CommandScope, CommandSource,
};

/// What running a prompt needs: which one, and its arguments.
///
/// A trait alias rather than a bare `Fn` so the signature the driver implements and
/// the source calls cannot drift: adding an argument here is a compile error at both
/// ends instead of at one.
pub trait McpPromptSubmit:
    Fn(String, String, std::collections::BTreeMap<String, String>) + Send + Sync
{
}
impl<T> McpPromptSubmit for T where
    T: Fn(String, String, std::collections::BTreeMap<String, String>) + Send + Sync
{
}

/// One prompt a server advertises.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpPromptEntry {
    pub server: String,
    pub name: String,
    pub description: String,
}

/// Where the entries come from. Separate from the source so a host can drive it
/// from the live router in production and from a fixture in a test.
#[async_trait]
pub trait McpPromptBackend: Send + Sync {
    async fn list(&self) -> anyhow::Result<Vec<McpPromptEntry>>;
}

/// A backend over an immutable snapshot of client handles.
///
/// A handle is an `Arc<McpClient>` clone, so listing never holds a lock across an
/// await and a refresh cannot contend with the loop.
pub struct SnapshotPromptBackend {
    handles: Vec<crate::mcp::McpCallHandle>,
}

impl SnapshotPromptBackend {
    #[must_use]
    pub fn new(handles: Vec<crate::mcp::McpCallHandle>) -> Self {
        Self { handles }
    }
}

#[async_trait]
impl McpPromptBackend for SnapshotPromptBackend {
    async fn list(&self) -> anyhow::Result<Vec<McpPromptEntry>> {
        let mut entries = Vec::new();
        for handle in &self.handles {
            // One unreachable server must not hide the prompts of the others, and
            // a refresh that fails entirely would leave the previous snapshot in
            // place — which is the better failure: stale commands still work.
            let Ok(prompts) = handle.prompts().await else {
                continue;
            };
            for (name, description) in prompts {
                entries.push(McpPromptEntry {
                    server: handle.server().to_string(),
                    name,
                    description,
                });
            }
        }
        entries
            .sort_by(|left, right| (&left.server, &left.name).cmp(&(&right.server, &right.name)));
        entries.dedup_by(|later, earlier| {
            later.server == earlier.server && later.name == earlier.name
        });
        Ok(entries)
    }
}

/// The command name a prompt gets.
///
/// The same shape an MCP tool has (`mcp__<server>__<tool>`), so a user who has
/// learned one has learned both, and a prompt cannot collide with a built-in.
#[must_use]
pub fn command_name(server: &str, prompt: &str) -> String {
    format!("mcp__{server}__{prompt}")
}

/// One prompt, as a runnable command.
pub struct McpPromptCommand {
    meta: CommandMeta,
    server: String,
    prompt: String,
    /// `None` when nothing is serving commands, which the run reports rather than
    /// dropping the request silently.
    submit: Option<Arc<dyn McpPromptSubmit>>,
}

impl Command for McpPromptCommand {
    fn meta(&self) -> &CommandMeta {
        &self.meta
    }

    fn run(&self, invocation: &CommandInvocation) -> CommandOutcome {
        // A prompt is a *starting message*: it has to be fetched and rendered
        // against the live router, which a command cannot reach. Queue it and let
        // the driver run the same path `/mcp prompt` uses.
        let Some(submit) = &self.submit else {
            return CommandOutcome::Failed(
                "no catalog is serving commands, so this prompt cannot be run".to_string(),
            );
        };
        // Prompts take arguments, and dropping them would run the prompt with the
        // placeholders unfilled — worse than refusing, because it looks like it
        // worked. The grammar is the one `/mcp prompt` already uses.
        let mut arguments = std::collections::BTreeMap::new();
        for token in invocation.args.split_whitespace() {
            match token.split_once('=') {
                Some((key, value)) if !key.is_empty() => {
                    arguments.insert(key.to_string(), value.to_string());
                }
                _ => {
                    return CommandOutcome::Usage(format!(
                        "/{} takes key=value arguments; `{token}` is not one",
                        self.meta.name
                    ));
                }
            }
        }
        submit(self.server.clone(), self.prompt.clone(), arguments);
        CommandOutcome::Message(format!(
            "running prompt {} from {}",
            self.prompt, self.server
        ))
    }
}

/// Every prompt the connected servers advertise.
///
/// One instance per agent, mutated in place: `set_backend` after an MCP reload and
/// `set_submit` once by whoever serves commands. Replacing the instance instead
/// would leave every catalog already holding the old one frozen at the prompt set
/// it was built with.
pub struct McpPromptSource {
    entries: std::sync::RwLock<Vec<McpPromptEntry>>,
    backend: std::sync::RwLock<Arc<dyn McpPromptBackend>>,
    submit: std::sync::OnceLock<Arc<dyn McpPromptSubmit>>,
}

impl McpPromptSource {
    pub fn new(backend: Arc<dyn McpPromptBackend>) -> Self {
        Self {
            entries: std::sync::RwLock::new(Vec::new()),
            backend: std::sync::RwLock::new(backend),
            submit: std::sync::OnceLock::new(),
        }
    }

    /// Point the source at a fresh snapshot of client handles.
    ///
    /// Called after an MCP reload: a snapshot backend holds the handles it was
    /// built with, so without this a reconnected server's prompts would never
    /// appear.
    pub fn set_backend(&self, backend: Arc<dyn McpPromptBackend>) {
        *self
            .backend
            .write()
            .unwrap_or_else(|error| error.into_inner()) = backend;
    }

    /// Wire the queue a running command hands its prompt to.
    ///
    /// Set once, by whichever catalog serves commands first (the loop). A later
    /// caller cannot replace it, because a second queue would be a receiver nobody
    /// drains — the prompt would vanish.
    pub fn set_submit(&self, submit: Arc<dyn McpPromptSubmit>) {
        let _ = self.submit.set(submit);
    }

    /// The prompts currently known, for a caller that wants to report a count.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries
            .read()
            .unwrap_or_else(|error| error.into_inner())
            .len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[async_trait]
impl CommandSource for McpPromptSource {
    fn id(&self) -> &str {
        "mcp"
    }

    async fn refresh(&self) -> anyhow::Result<bool> {
        // Cloned out of the lock: `list` is async and a reload may swap it.
        let backend = self
            .backend
            .read()
            .unwrap_or_else(|error| error.into_inner())
            .clone();
        let entries = backend.list().await?;
        let mut held = self
            .entries
            .write()
            .unwrap_or_else(|error| error.into_inner());
        let changed = *held != entries;
        *held = entries;
        Ok(changed)
    }

    fn commands(&self) -> Vec<Arc<dyn Command>> {
        let entries = self
            .entries
            .read()
            .unwrap_or_else(|error| error.into_inner())
            .clone();
        entries
            .into_iter()
            .map(|entry| {
                Arc::new(McpPromptCommand {
                    meta: CommandMeta {
                        name: command_name(&entry.server, &entry.name),
                        aliases: Vec::new(),
                        summary: if entry.description.is_empty() {
                            format!("Prompt {} on {}", entry.name, entry.server)
                        } else {
                            entry.description.clone()
                        },
                        usage: String::new(),
                        scope: CommandScope::Runtime,
                        needs_args: false,
                    },
                    server: entry.server,
                    prompt: entry.name,
                    submit: self.submit.get().cloned(),
                }) as Arc<dyn Command>
            })
            .collect()
    }
}

/// The shared source for one router.
pub fn shared_source(router: &crate::mcp::MCPToolRouter) -> Arc<McpPromptSource> {
    Arc::new(McpPromptSource::new(Arc::new(SnapshotPromptBackend::new(
        router.call_handles(),
    ))))
}

/// Point an existing source at `router`'s current connections.
pub fn rebind(source: &McpPromptSource, router: &crate::mcp::MCPToolRouter) {
    source.set_backend(Arc::new(SnapshotPromptBackend::new(router.call_handles())));
}

/// Add `source` to `catalog` and return it, so the caller can refresh and report.
pub fn attach(
    catalog: &tact_kernel::catalog::CommandCatalog,
    source: Arc<McpPromptSource>,
) -> Arc<McpPromptSource> {
    catalog.add_source(source.clone());
    source
}

/// Whether a capability id names a command this source contributes.
///
/// Used by the filter tests and by a caller that wants to explain a refusal.
#[must_use]
pub fn capability_for(name: &str) -> CapabilityId {
    CapabilityId::new(CapabilityKind::Command, name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FixedBackend(Vec<McpPromptEntry>);

    #[async_trait]
    impl McpPromptBackend for FixedBackend {
        async fn list(&self) -> anyhow::Result<Vec<McpPromptEntry>> {
            Ok(self.0.clone())
        }
    }

    fn entry(server: &str, name: &str) -> McpPromptEntry {
        McpPromptEntry {
            server: server.to_string(),
            name: name.to_string(),
            description: format!("{name} prompt"),
        }
    }

    #[tokio::test]
    async fn a_prompt_becomes_a_command_after_a_refresh() {
        let source = McpPromptSource::new(Arc::new(FixedBackend(vec![entry("docs", "summarise")])));
        assert!(
            source.commands().is_empty(),
            "nothing is known before the first refresh"
        );

        assert!(
            source.refresh().await.unwrap(),
            "the first refresh changed it"
        );
        let commands = source.commands();
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].meta().name, "mcp__docs__summarise");
        assert_eq!(commands[0].meta().summary, "summarise prompt");
    }

    #[tokio::test]
    async fn a_refresh_that_lists_the_same_prompts_reports_no_change() {
        let source = McpPromptSource::new(Arc::new(FixedBackend(vec![entry("docs", "summarise")])));
        assert!(source.refresh().await.unwrap());
        // The generation must not move for an unchanged set, or a host comparing
        // generations would believe a stale list had just been rebuilt.
        assert!(!source.refresh().await.unwrap());
    }

    #[tokio::test]
    async fn running_a_prompt_queues_it_rather_than_answering_inline() {
        let sent = Arc::new(std::sync::Mutex::new(Vec::<(
            String,
            String,
            std::collections::BTreeMap<String, String>,
        )>::new()));
        let sink = sent.clone();
        let source = McpPromptSource::new(Arc::new(FixedBackend(vec![entry("docs", "summarise")])));
        source.set_submit(Arc::new(move |server, prompt, arguments| {
            sink.lock().unwrap().push((server, prompt, arguments));
        }));
        source.refresh().await.unwrap();
        let commands = source.commands();
        let outcome = commands[0].run(&CommandInvocation {
            name: "mcp__docs__summarise".to_string(),
            args: String::new(),
        });
        assert!(matches!(outcome, CommandOutcome::Message(_)));
        assert_eq!(
            sent.lock().unwrap().len(),
            1,
            "the driver needs the triple to fetch the prompt"
        );
        let (server, prompt, arguments) = &sent.lock().unwrap()[0];
        assert_eq!((server.as_str(), prompt.as_str()), ("docs", "summarise"));
        assert!(arguments.is_empty());
    }

    /// A reload swaps the handles inside the same source.
    ///
    /// Replacing the instance instead would leave every catalog that already holds
    /// it — the palette outlives the driver's — frozen at the old prompt set.
    #[tokio::test]
    async fn rebinding_the_backend_changes_what_the_source_offers() {
        let source = McpPromptSource::new(Arc::new(FixedBackend(vec![entry("old", "gone")])));
        assert!(source.refresh().await.unwrap());
        assert_eq!(source.commands()[0].meta().name, "mcp__old__gone");

        source.set_backend(Arc::new(FixedBackend(vec![entry("new", "live")])));
        // The entries are still the old ones until something lists them again.
        assert_eq!(source.commands()[0].meta().name, "mcp__old__gone");

        assert!(
            source.refresh().await.unwrap(),
            "a different set is a change"
        );
        let names: Vec<String> = source
            .commands()
            .into_iter()
            .map(|command| command.meta().name.clone())
            .collect();
        assert_eq!(names, vec!["mcp__new__live".to_string()]);
    }

    /// Arguments reach the driver, because a prompt that takes them cannot be run
    /// without them.
    ///
    /// Dropping them would fetch the prompt with its placeholders unfilled: the run
    /// would look successful and produce the wrong message, which is worse than
    /// refusing.
    #[tokio::test]
    async fn arguments_are_passed_through() {
        let sent = Arc::new(std::sync::Mutex::new(Vec::<(
            String,
            String,
            std::collections::BTreeMap<String, String>,
        )>::new()));
        let sink = sent.clone();

        let source = McpPromptSource::new(Arc::new(FixedBackend(vec![entry("docs", "search")])));
        source.set_submit(Arc::new(move |server, prompt, arguments| {
            sink.lock().unwrap().push((server, prompt, arguments));
        }));
        source.refresh().await.unwrap();

        let outcome = source.commands()[0].run(&CommandInvocation {
            name: "mcp__docs__search".to_string(),
            args: "query=notes depth=2".to_string(),
        });
        assert!(matches!(outcome, CommandOutcome::Message(_)));
        let (_, _, arguments) = &sent.lock().unwrap()[0];
        assert_eq!(arguments.get("query").map(String::as_str), Some("notes"));
        assert_eq!(arguments.get("depth").map(String::as_str), Some("2"));
    }

    /// A token that is not `key=value` is refused by name rather than ignored.
    #[tokio::test]
    async fn a_malformed_argument_names_itself() {
        let source = McpPromptSource::new(Arc::new(FixedBackend(vec![entry("docs", "search")])));
        source.set_submit(Arc::new(|_, _, _| {}));
        source.refresh().await.unwrap();

        let outcome = source.commands()[0].run(&CommandInvocation {
            name: "mcp__docs__search".to_string(),
            args: "query=notes depth".to_string(),
        });
        match outcome {
            CommandOutcome::Usage(message) => {
                assert!(
                    message.contains("depth") && message.contains("key=value"),
                    "the line must name the token and the expected form, got: {message}"
                );
            }
            other => panic!("expected a usage line, got {other:?}"),
        }
    }

    /// The first `set_submit` wins.
    ///
    /// A second queue would have no drainer, so the prompt would be accepted by a
    /// receiver nobody reads — the same as losing it, only harder to see.
    #[tokio::test]
    async fn a_second_submit_is_ignored() {
        let first = Arc::new(std::sync::Mutex::new(0usize));
        let second = Arc::new(std::sync::Mutex::new(0usize));

        let source = McpPromptSource::new(Arc::new(FixedBackend(vec![entry("docs", "one")])));
        let sink = first.clone();
        source.set_submit(Arc::new(move |_, _, _| {
            *sink.lock().unwrap() += 1;
        }));
        let other = second.clone();
        source.set_submit(Arc::new(move |_, _, _| {
            *other.lock().unwrap() += 1;
        }));
        source.refresh().await.unwrap();

        source.commands()[0].run(&CommandInvocation {
            name: "mcp__docs__one".to_string(),
            args: String::new(),
        });
        assert_eq!(*first.lock().unwrap(), 1, "the first queue gets it");
        assert_eq!(
            *second.lock().unwrap(),
            0,
            "the second queue must not receive a prompt nothing drains"
        );
    }

    /// A command with no serving catalog reports instead of doing nothing.
    #[tokio::test]
    async fn a_command_with_no_queue_fails_loudly() {
        let source = McpPromptSource::new(Arc::new(FixedBackend(vec![entry("docs", "one")])));
        source.refresh().await.unwrap();
        let outcome = source.commands()[0].run(&CommandInvocation {
            name: "mcp__docs__one".to_string(),
            args: String::new(),
        });
        assert!(
            matches!(outcome, CommandOutcome::Failed(_)),
            "silently dropping the request is the failure mode being avoided"
        );
    }

    #[test]
    fn a_prompt_name_cannot_collide_with_a_builtin() {
        // The prefix is what makes that true, so it is the thing worth asserting.
        assert_eq!(command_name("stats", "list"), "mcp__stats__list");
        assert_ne!(command_name("stats", "list"), "stats");
    }
}
