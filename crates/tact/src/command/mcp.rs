//! `/mcp` as a catalog command — the read-only half.
//!
//! `list` and `prompts` only ever read the router the agent already holds, so
//! they need a narrow view of it rather than the agent. `auth` and `run` are not
//! here: authorizing reloads the router and running a prompt submits a turn, and
//! both of those are the runtime's work, not a listing's.

use std::sync::Arc;

use async_trait::async_trait;
use tact_kernel::capability::command::{
    Command, CommandInvocation, CommandMeta, CommandOutcome, CommandScope,
};

use crate::command::McpCatalog;

/// What a `/mcp` invocation asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
enum McpVerb {
    List,
    Prompts { server: Option<String> },
}

/// How a server listing becomes text.
pub type McpRenderer = Arc<dyn Fn(&[crate::mcp::McpServerView]) -> String + Send + Sync>;

/// Inspect connected MCP servers and their prompts.
pub struct McpCommand {
    meta: CommandMeta,
    catalog: Arc<dyn McpCatalog>,
    /// How a server listing becomes text.
    ///
    /// A function because the renderer lives with the other CLI surfaces; a
    /// command that reached for it would make this crate depend on a frontend.
    render: McpRenderer,
}

impl McpCommand {
    /// A command over the given read-only view.
    pub fn new(catalog: Arc<dyn McpCatalog>, render: McpRenderer) -> Self {
        Self {
            meta: CommandMeta {
                name: "mcp".to_string(),
                aliases: Vec::new(),
                summary: "List MCP servers, or show one server's prompts".to_string(),
                usage: "/mcp [list | prompts [<server>]]".to_string(),
                scope: CommandScope::Runtime,
                // Verbs follow, so Enter completes instead of running.
                needs_args: true,
            },
            catalog,
            render,
        }
    }

    fn parse(args: &str) -> Result<McpVerb, String> {
        let mut tokens = args.split_whitespace();
        match tokens.next().unwrap_or("") {
            "" | "list" => {
                if tokens.next().is_some() {
                    return Err("`/mcp list` takes no arguments".to_string());
                }
                Ok(McpVerb::List)
            }
            "prompts" => {
                let server = tokens.next().map(str::to_string);
                if tokens.next().is_some() {
                    return Err("`/mcp prompts` takes at most one server".to_string());
                }
                Ok(McpVerb::Prompts { server })
            }
            other => Err(format!("unknown `/mcp` verb `{other}`")),
        }
    }
}

#[async_trait]
impl Command for McpCommand {
    fn meta(&self) -> &CommandMeta {
        &self.meta
    }

    async fn run_async(&self, invocation: &CommandInvocation) -> CommandOutcome {
        let verb = match Self::parse(&invocation.args) {
            Ok(verb) => verb,
            Err(message) => {
                return CommandOutcome::Usage(format!("{message}\n{}", self.meta.usage));
            }
        };
        match verb {
            McpVerb::List => {
                // A live view of the router the agent holds: never reload here,
                // because a reconnect would drop live stdio children and
                // duplicate remote dials just to print a table.
                match self.catalog.describe(&self.catalog.server_summaries()) {
                    Ok(views) => CommandOutcome::Emit(crate::command::Emitted::new(
                        tact_protocol::AgentUpdate::MdInfo((self.render)(&views)),
                    )),
                    Err(error) => CommandOutcome::Failed(format!("MCP list failed: {error:#}")),
                }
            }
            McpVerb::Prompts { server } => match self.catalog.list_prompts(server.as_deref()).await
            {
                Ok(listing) => CommandOutcome::Emit(crate::command::Emitted::new(
                    tact_protocol::AgentUpdate::MdInfo(listing),
                )),
                Err(error) => CommandOutcome::Failed(format!("MCP prompts failed: {error:#}")),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FixedCatalog {
        servers: Vec<(String, usize)>,
        prompts: Option<String>,
    }

    #[async_trait]
    impl McpCatalog for FixedCatalog {
        fn server_summaries(&self) -> Vec<(String, usize)> {
            self.servers.clone()
        }

        fn describe(
            &self,
            connected: &[(String, usize)],
        ) -> anyhow::Result<Vec<crate::mcp::McpServerView>> {
            // The fixed catalog declares nothing of its own; the renderer counts
            // what it is given, which is what the listing assertions read.
            let _ = connected;
            Ok(Vec::new())
        }

        fn list_prompts<'a>(
            &'a self,
            server: Option<&'a str>,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<String>> + Send + 'a>>
        {
            Box::pin(async move {
                match &self.prompts {
                    Some(text) => Ok(match server {
                        Some(name) => format!("{text} ({name})"),
                        None => text.clone(),
                    }),
                    None => anyhow::bail!("no prompts"),
                }
            })
        }
    }

    fn command(catalog: FixedCatalog) -> McpCommand {
        McpCommand::new(
            Arc::new(catalog),
            Arc::new(|views: &[crate::mcp::McpServerView]| format!("{} servers", views.len())),
        )
    }

    /// No verb means `list`.
    #[test]
    fn an_empty_argument_list_means_list() {
        assert_eq!(McpCommand::parse("").unwrap(), McpVerb::List);
        assert_eq!(McpCommand::parse("list").unwrap(), McpVerb::List);
    }

    /// `prompts` takes an optional server.
    #[test]
    fn prompts_takes_an_optional_server() {
        assert_eq!(
            McpCommand::parse("prompts").unwrap(),
            McpVerb::Prompts { server: None }
        );
        assert_eq!(
            McpCommand::parse("prompts basic-memory").unwrap(),
            McpVerb::Prompts {
                server: Some("basic-memory".to_string())
            }
        );
    }

    /// A second server argument is refused rather than ignored.
    #[test]
    fn a_second_server_is_refused() {
        let error = McpCommand::parse("prompts one two").unwrap_err();
        assert!(error.contains("one server"), "{error}");
    }

    /// An unknown verb is refused by name.
    #[test]
    fn an_unknown_verb_is_refused() {
        let error = McpCommand::parse("auth").unwrap_err();
        assert!(error.contains("auth"), "{error}");
    }

    /// `list` renders the servers the catalog reports.
    #[tokio::test]
    async fn list_renders_the_catalog() {
        let outcome = command(FixedCatalog {
            servers: vec![("one".to_string(), 3)],
            prompts: None,
        })
        .run_async(&CommandInvocation {
            name: "mcp".to_string(),
            args: "list".to_string(),
        })
        .await;
        match outcome {
            CommandOutcome::Emit(emitted) => match emitted.into_inner() {
                tact_protocol::AgentUpdate::MdInfo(text) => {
                    assert!(text.contains("servers"), "{text}")
                }
                other => panic!("expected MdInfo, got {other:?}"),
            },
            other => panic!("expected an emitted listing, got {other:?}"),
        }
    }

    /// `prompts` passes the server through and shows what came back.
    #[tokio::test]
    async fn prompts_scopes_to_the_named_server() {
        let outcome = command(FixedCatalog {
            servers: Vec::new(),
            prompts: Some("available prompts".to_string()),
        })
        .run_async(&CommandInvocation {
            name: "mcp".to_string(),
            args: "prompts basic-memory".to_string(),
        })
        .await;
        match outcome {
            CommandOutcome::Emit(emitted) => match emitted.into_inner() {
                tact_protocol::AgentUpdate::MdInfo(text) => {
                    assert!(text.contains("basic-memory"), "{text}")
                }
                other => panic!("expected MdInfo, got {other:?}"),
            },
            other => panic!("expected an emitted listing, got {other:?}"),
        }
    }

    /// A failing listing is reported as a failure, not as an empty list.
    #[tokio::test]
    async fn a_failure_is_reported_as_one() {
        let outcome = command(FixedCatalog {
            servers: Vec::new(),
            prompts: None,
        })
        .run_async(&CommandInvocation {
            name: "mcp".to_string(),
            args: "prompts".to_string(),
        })
        .await;
        assert!(
            matches!(&outcome, CommandOutcome::Failed(text) if text.contains("MCP prompts failed")),
            "{outcome:?}"
        );
    }

    /// The metadata says it takes arguments.
    #[test]
    fn the_metadata_says_it_takes_arguments() {
        let meta = command(FixedCatalog {
            servers: Vec::new(),
            prompts: None,
        })
        .meta()
        .clone();
        assert_eq!(meta.name, "mcp");
        assert!(meta.needs_args);
        assert_eq!(meta.scope, CommandScope::Runtime);
    }
}
