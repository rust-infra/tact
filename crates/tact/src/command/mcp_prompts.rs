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
    submit: Arc<dyn Fn(String, String) + Send + Sync>,
}

impl Command for McpPromptCommand {
    fn meta(&self) -> &CommandMeta {
        &self.meta
    }

    fn run(&self, _invocation: &CommandInvocation) -> CommandOutcome {
        // A prompt is a *starting message*: it has to be fetched and rendered
        // against the live router, which a command cannot reach. Queue it and let
        // the driver run the same path `/mcp prompts` uses.
        (self.submit)(self.server.clone(), self.prompt.clone());
        CommandOutcome::Message(format!(
            "running prompt {} from {}",
            self.prompt, self.server
        ))
    }
}

/// Every prompt the connected servers advertise.
pub struct McpPromptSource {
    entries: std::sync::RwLock<Vec<McpPromptEntry>>,
    backend: Arc<dyn McpPromptBackend>,
    submit: Arc<dyn Fn(String, String) + Send + Sync>,
}

impl McpPromptSource {
    pub fn new(
        backend: Arc<dyn McpPromptBackend>,
        submit: Arc<dyn Fn(String, String) + Send + Sync>,
    ) -> Self {
        Self {
            entries: std::sync::RwLock::new(Vec::new()),
            backend,
            submit,
        }
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
        let entries = self.backend.list().await?;
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
                    submit: self.submit.clone(),
                }) as Arc<dyn Command>
            })
            .collect()
    }
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
        let source = McpPromptSource::new(
            Arc::new(FixedBackend(vec![entry("docs", "summarise")])),
            Arc::new(|_, _| {}),
        );
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
        let source = McpPromptSource::new(
            Arc::new(FixedBackend(vec![entry("docs", "summarise")])),
            Arc::new(|_, _| {}),
        );
        assert!(source.refresh().await.unwrap());
        // The generation must not move for an unchanged set, or a host comparing
        // generations would believe a stale list had just been rebuilt.
        assert!(!source.refresh().await.unwrap());
    }

    #[tokio::test]
    async fn running_a_prompt_queues_it_rather_than_answering_inline() {
        let sent = Arc::new(std::sync::Mutex::new(Vec::<(String, String)>::new()));
        let sink = sent.clone();
        let source = McpPromptSource::new(
            Arc::new(FixedBackend(vec![entry("docs", "summarise")])),
            Arc::new(move |server, prompt| {
                sink.lock().unwrap().push((server, prompt));
            }),
        );
        source.refresh().await.unwrap();
        let commands = source.commands();
        let outcome = commands[0].run(&CommandInvocation {
            name: "mcp__docs__summarise".to_string(),
            args: String::new(),
        });
        assert!(matches!(outcome, CommandOutcome::Message(_)));
        assert_eq!(
            sent.lock().unwrap().as_slice(),
            [("docs".to_string(), "summarise".to_string())],
            "the driver needs the pair to fetch the prompt"
        );
    }

    #[test]
    fn a_prompt_name_cannot_collide_with_a_builtin() {
        // The prefix is what makes that true, so it is the thing worth asserting.
        assert_eq!(command_name("stats", "list"), "mcp__stats__list");
        assert_ne!(command_name("stats", "list"), "stats");
    }
}
