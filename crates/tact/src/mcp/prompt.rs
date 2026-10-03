//! MCP **prompts**, exposed to the agent as two tools.
//!
//! The third and last MCP primitive. A server publishes *prompts* — named
//! templates that compose messages, with optional arguments — and until now
//! Tact implemented `tools/*` and `resources/*` and never asked for
//! `prompts/list`. The gap was recorded as deliberate: a prompt is "a string of
//! messages the server wrote, not a tool call", so Tact's turn structure had no
//! consumer for one.
//!
//! That reasoning was about *pushing* prompts at the model, which is still not
//! something Tact does. It does not apply to a tool result: `get_mcp_prompt`
//! returns the composed messages as text, and the model reads them the way it
//! reads a resource. The consumer problem disappears at the same moment the
//! primitive becomes a tool the model can call.
//!
//! The cost of leaving it out is not neutral, either. Basic Memory advertises
//! four prompts and declares `prompts` in its capabilities; a model told to
//! "start with `getting_started`" had no way to fetch one, exactly as it had no
//! way to read `memory://ai_assistant_guide` before resources were wired.
//!
//! The two names follow the resource pair's convention (`list_mcp_resources` /
//! `read_mcp_resource`) rather than inventing a new shape.
//!
//! Like the resource tools, these are resolved in `agent::tool_dispatch` against
//! the live router and only exist while a server is connected.

use anyhow::{Context as _, Result, bail};
use rmcp::model::{
    GetPromptRequestParams, GetPromptResult, JsonObject, Prompt, PromptMessageContent,
    PromptMessageRole,
};
use serde_json::json;

use super::{MCP_FETCH_TIMEOUT, MCPToolRouter, McpClient};
use crate::{ToolSpec, permission::CapabilityRisk};

/// List the prompts connected servers expose.
pub const LIST_PROMPTS_TOOL: &str = "list_mcp_prompts";
/// Fetch one prompt, with its arguments filled in.
pub const GET_PROMPT_TOOL: &str = "get_mcp_prompt";

/// Which prompt tool a resolved tool call is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpPromptTool {
    List,
    Get,
}

impl McpPromptTool {
    /// Every tool, in a stable order: the listing before the get, so the name a
    /// listing prints is adjacent to the call that consumes it.
    pub const ALL: [Self; 2] = [Self::List, Self::Get];

    /// Resolves a tool name, or `None` when it is not a prompt tool.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            LIST_PROMPTS_TOOL => Some(Self::List),
            GET_PROMPT_TOOL => Some(Self::Get),
            _ => None,
        }
    }

    /// The exact name the agent must call.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::List => LIST_PROMPTS_TOOL,
            Self::Get => GET_PROMPT_TOOL,
        }
    }

    /// The agent-facing schema.
    #[must_use]
    pub fn spec(self) -> ToolSpec {
        match self {
            Self::List => ToolSpec {
                name: LIST_PROMPTS_TOOL.to_string(),
                description: Some(
                    "List the prompt templates the connected MCP servers publish. Pass `server` \
                     to inspect just one. A prompt is a named, reusable instruction the server \
                     wrote; fetch one with `get_mcp_prompt`, filling in the arguments the \
                     listing marks as required."
                        .to_string(),
                ),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "server": {
                            "type": "string",
                            "description": "Only list this MCP server's prompts."
                        }
                    }
                }),
            },
            Self::Get => ToolSpec {
                name: GET_PROMPT_TOOL.to_string(),
                description: Some(
                    "Fetch one MCP prompt template (see `list_mcp_prompts`). Returns the messages \
                     the server composed, in order, labelled by role. Pass `arguments` for the \
                     placeholders the listing showed."
                        .to_string(),
                ),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "server": {
                            "type": "string",
                            "description": "The MCP server that owns the prompt."
                        },
                        "name": {
                            "type": "string",
                            "description": "The prompt name, exactly as `list_mcp_prompts` printed it."
                        },
                        "arguments": {
                            "type": "object",
                            "description": "Values for the prompt's arguments, keyed by name.",
                            "additionalProperties": { "type": "string" }
                        }
                    },
                    "required": ["server", "name"]
                }),
            },
        }
    }
}

/// The risk of one of Tact's own prompt tools.
///
/// Same reasoning as [`super::resource_tool_risk`]: these names belong to Tact,
/// so no entry's `tools.<name>.risk` can address them and `[mcp]` needs a key of
/// its own. Listing and getting are separate keys because the acts differ — a
/// listing returns metadata, a get returns server-authored content. Both default
/// to `High`, and so does an unresolvable config, so the unconfigured path is the
/// restrictive one.
#[must_use]
pub fn prompt_tool_risk(tool: McpPromptTool) -> CapabilityRisk {
    match crate::config::try_settings() {
        Some(settings) => prompt_tool_risk_with(tool, &settings.mcp),
        None => CapabilityRisk::High,
    }
}

/// [`prompt_tool_risk`] against given settings.
///
/// Split out so the tool-to-key mapping is testable without installing a
/// process-global config.
#[must_use]
pub fn prompt_tool_risk_with(
    tool: McpPromptTool,
    mcp: &crate::config::McpSettings,
) -> CapabilityRisk {
    match tool {
        McpPromptTool::List => mcp.prompt_list_risk,
        McpPromptTool::Get => mcp.prompt_get_risk,
    }
    .unwrap_or(CapabilityRisk::High)
}

impl McpClient {
    /// `prompts/list` for this server.
    pub async fn list_prompts(&self) -> Result<Vec<Prompt>> {
        tokio::time::timeout(MCP_FETCH_TIMEOUT, self.service.list_prompts())
            .await
            .with_context(|| {
                format!(
                    "MCP server {} did not list its prompts within {}s",
                    self.server_name,
                    MCP_FETCH_TIMEOUT.as_secs()
                )
            })?
            .with_context(|| format!("failed to list prompts from {}", self.server_name))
    }

    /// `prompts/get` for one prompt on this server.
    pub async fn get_prompt(
        &self,
        name: &str,
        arguments: Option<JsonObject>,
    ) -> Result<GetPromptResult> {
        tokio::time::timeout(
            MCP_FETCH_TIMEOUT,
            self.service.get_prompt(get_prompt_params(name, arguments)),
        )
        .await
        .with_context(|| {
            format!(
                "MCP server {} did not return prompt {name} within {}s",
                self.server_name,
                MCP_FETCH_TIMEOUT.as_secs()
            )
        })?
        .with_context(|| format!("failed to get prompt {name} from {}", self.server_name))
    }
}

impl MCPToolRouter {
    /// The prompt tools, or none when no server is connected.
    ///
    /// Same rule as [`Self::resource_tool_specs`]: advertising them with an
    /// empty router would hand the model a tool whose only answer is "no MCP
    /// servers are connected".
    #[must_use]
    pub fn prompt_tool_specs(&self) -> Vec<ToolSpec> {
        if self.clients.is_empty() {
            return Vec::new();
        }
        McpPromptTool::ALL.iter().map(|tool| tool.spec()).collect()
    }

    /// Every prompt of every connected server, or of one named server.
    ///
    /// Returns a rendered listing rather than the raw models: this is a tool
    /// result the model reads, and the prompt names plus their argument
    /// vocabulary are what it needs to copy.
    pub async fn list_prompts(&self, server: Option<&str>) -> Result<String> {
        let names = self.known_servers();
        if names.is_empty() {
            bail!("no MCP servers are connected");
        }

        let selected: Vec<&str> = match server {
            Some(server) => {
                let client = self.clients.get(server).with_context(|| {
                    format!(
                        "unknown MCP server {server} (connected: {})",
                        names.join(", ")
                    )
                })?;
                vec![client.server_name.as_str()]
            }
            None => names.iter().map(String::as_str).collect(),
        };

        let mut results = Vec::with_capacity(selected.len());
        for name in selected {
            let client = &self.clients[name];
            results.push((name.to_string(), client.list_prompts().await));
        }
        Ok(super::render_server_results(
            "prompts/list",
            results,
            render_prompt_listing,
        ))
    }

    /// Fetches one prompt and renders its messages for the model.
    ///
    /// Scoped to the server it names, like [`Self::read_resource`]: a get is one
    /// server's work, while a listing spans all of them.
    pub async fn get_prompt(
        &self,
        server: &str,
        name: &str,
        arguments: Option<JsonObject>,
    ) -> Result<String> {
        let client = self.clients.get(server).with_context(|| {
            format!(
                "unknown MCP server {server} (connected: {})",
                self.known_servers().join(", ")
            )
        })?;
        let result = client.get_prompt(name, arguments).await?;
        Ok(render_prompt_messages(name, &result))
    }
}

/// Renders `list_mcp_prompts`'s result.
///
/// One `## <server>` section per server, in the order given (already sorted by
/// the caller). Each prompt carries its arguments and which of them are
/// required: a template whose placeholders are invisible reads as one that
/// takes none, and the model then calls `get_mcp_prompt` without them.
#[must_use]
pub fn render_prompt_listing(listings: &[(String, Vec<Prompt>)]) -> String {
    let total: usize = listings.iter().map(|(_, prompts)| prompts.len()).sum();
    if total == 0 {
        return format!(
            "No prompts: {} connected server(s) expose none through `prompts/list`.",
            listings.len()
        );
    }

    let mut out = format!("{total} prompt(s) across {} server(s):", listings.len());
    for (server, prompts) in listings {
        out.push_str(&format!("\n\n## {server}"));
        for prompt in prompts {
            out.push_str("\n- ");
            out.push_str(&prompt.name);
            if let Some(title) = &prompt.title {
                out.push_str(&format!(" ({title})"));
            }
            if let Some(description) = &prompt.description {
                out.push_str(&format!("\n  {}", description.replace('\n', " ")));
            }
            match prompt.arguments.as_deref() {
                Some([]) | None => {}
                Some(arguments) => {
                    let rendered: Vec<String> = arguments
                        .iter()
                        .map(|argument| {
                            if argument.required.unwrap_or(false) {
                                format!("{} (required)", argument.name)
                            } else {
                                argument.name.clone()
                            }
                        })
                        .collect();
                    out.push_str(&format!("\n  arguments: {}", rendered.join(", ")));
                }
            }
        }
    }
    out
}

/// Renders `get_mcp_prompt`'s result.
///
/// One `## <role>` section per message, in the order the server composed them:
/// the order *is* the instruction, so it is never re-sorted or merged.
#[must_use]
pub fn render_prompt_messages(name: &str, result: &GetPromptResult) -> String {
    if result.messages.is_empty() {
        return format!("Prompt {name} returned no messages.");
    }

    let mut out = format!("{name}: {} message(s)", result.messages.len());
    if let Some(description) = &result.description {
        out.push_str(&format!(" — {}", description.replace('\n', " ")));
    }
    for message in &result.messages {
        let role = match message.role {
            PromptMessageRole::User => "user",
            PromptMessageRole::Assistant => "assistant",
        };
        out.push_str(&format!(
            "\n\n## {role}\n\n{}",
            render_prompt_content(&message.content)
        ));
    }
    out
}

/// One prompt message's content, as text the model can read.
///
/// Images and embedded blobs are reported by size rather than inlined, for the
/// same reason `read_mcp_resource` does it: a base64 payload is unreadable to the
/// model and expensive to carry.
fn render_prompt_content(content: &PromptMessageContent) -> String {
    match content {
        PromptMessageContent::Text { text } => text.clone(),
        PromptMessageContent::Image { image } => format!(
            "[image content ({}), {} base64 characters — not inlined]",
            image.mime_type,
            image.data.len()
        ),
        PromptMessageContent::Resource { resource } => {
            let contents = std::slice::from_ref(&resource.resource);
            let uri = match &resource.resource {
                rmcp::model::ResourceContents::TextResourceContents { uri, .. }
                | rmcp::model::ResourceContents::BlobResourceContents { uri, .. } => uri.clone(),
            };
            super::render_resource_contents(&uri, contents)
        }
        PromptMessageContent::ResourceLink { link } => {
            format!("[resource link: {} — {}]", link.uri, link.name)
        }
    }
}

/// Builds the request params `prompts/get` needs.
#[must_use]
pub fn get_prompt_params(name: &str, arguments: Option<JsonObject>) -> GetPromptRequestParams {
    GetPromptRequestParams {
        meta: None,
        name: name.to_string(),
        arguments,
    }
}

/// A caller's `key=value` pairs as the protocol's `arguments` map.
///
/// The TUI's `/mcp prompt` syntax produces string pairs; converting them *here*
/// keeps the protocol type out of the UI crates, which do not depend on
/// `serde_json`. An empty map becomes `None`: "no arguments" and "an empty
/// arguments object" are the same request.
#[must_use]
pub fn prompt_arguments_from_pairs(
    pairs: std::collections::BTreeMap<String, String>,
) -> Option<JsonObject> {
    (!pairs.is_empty()).then(|| {
        pairs
            .into_iter()
            .map(|(key, value)| (key, serde_json::Value::String(value)))
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use rmcp::model::{PromptArgument, PromptMessage};

    use super::*;
    use crate::mcp::{MCPToolRouter, McpClient, MockMcpService};

    /// One fixture prompt: name, description, `(argument, required)` pairs.
    type PromptFixture<'a> = (&'a str, &'a str, &'a [(&'a str, bool)]);

    /// A router whose one server publishes `prompts`.
    fn router_with(prompts: &[PromptFixture<'_>]) -> MCPToolRouter {
        let service = prompts.iter().fold(
            MockMcpService::new(Vec::new(), |_| {
                Ok(rmcp::model::CallToolResult::success(Vec::new()))
            }),
            |service, (name, description, arguments)| {
                service.with_prompt(name, description, arguments)
            },
        );
        let mut router = MCPToolRouter::new();
        router.register_client(McpClient::with_service(
            "basic-memory",
            Vec::new(),
            Arc::new(service),
        ));
        router
    }

    #[test]
    fn the_prompt_tools_exist_only_while_a_server_is_connected() {
        // With no servers the names are unresolvable, so advertising them would
        // hand the model a tool whose only possible answer is "nothing".
        assert!(MCPToolRouter::new().prompt_tool_specs().is_empty());

        let specs = router_with(&[]).prompt_tool_specs();
        let names: Vec<&str> = specs.iter().map(|spec| spec.name.as_str()).collect();
        assert_eq!(names, [LIST_PROMPTS_TOOL, GET_PROMPT_TOOL]);
    }

    #[test]
    fn getting_declares_the_arguments_it_cannot_infer() {
        let spec = McpPromptTool::Get.spec();
        assert_eq!(
            spec.input_schema["required"],
            serde_json::json!(["server", "name"])
        );
    }

    #[test]
    fn a_tool_name_round_trips() {
        for tool in McpPromptTool::ALL {
            assert_eq!(McpPromptTool::from_name(tool.name()), Some(tool));
        }
        assert_eq!(McpPromptTool::from_name("mcp__x__y"), None);
        assert_eq!(McpPromptTool::from_name("read_mcp_resource"), None);
    }

    #[test]
    fn a_listing_shows_the_argument_vocabulary() {
        let prompt = Prompt::new(
            "continue_conversation",
            Some("Continue a previous conversation"),
            Some(vec![
                PromptArgument {
                    name: "topic".to_string(),
                    title: None,
                    description: Some("What to continue".to_string()),
                    required: Some(true),
                },
                PromptArgument {
                    name: "project".to_string(),
                    title: None,
                    description: None,
                    required: None,
                },
            ]),
        );
        let listing = vec![("basic-memory".to_string(), vec![prompt])];

        let text = render_prompt_listing(&listing);
        assert!(text.contains("1 prompt(s) across 1 server(s)"), "{text}");
        assert!(text.contains("## basic-memory"), "{text}");
        assert!(text.contains("- continue_conversation"), "{text}");
        // A placeholder the model cannot see is one it will not fill.
        assert!(
            text.contains("arguments: topic (required), project"),
            "{text}"
        );
    }

    #[test]
    fn a_prompt_without_arguments_gains_no_argument_line() {
        let prompt = Prompt::new("getting_started", Some("Introduce Basic Memory"), None);
        let listing = vec![("basic-memory".to_string(), vec![prompt])];

        let text = render_prompt_listing(&listing);
        assert!(!text.contains("arguments:"), "{text}");
    }

    #[test]
    fn an_empty_listing_says_so_instead_of_printing_nothing() {
        let text = render_prompt_listing(&[("basic-memory".to_string(), Vec::new())]);
        assert!(text.contains("No prompts"), "{text}");
        assert!(text.contains("1 connected server(s)"), "{text}");
    }

    #[test]
    fn messages_are_rendered_in_order_with_their_roles() {
        let result = GetPromptResult {
            description: Some("Two turns\nacross lines".to_string()),
            messages: vec![
                PromptMessage::new_text(PromptMessageRole::User, "Summarise the notes."),
                PromptMessage::new_text(PromptMessageRole::Assistant, "Which project?"),
            ],
        };

        let text = render_prompt_messages("continue_conversation", &result);
        assert!(
            text.contains("continue_conversation: 2 message(s)"),
            "{text}"
        );
        assert!(text.contains("Two turns across lines"), "{text}");
        assert!(text.contains("## user\n\nSummarise the notes."), "{text}");
        assert!(text.contains("## assistant\n\nWhich project?"), "{text}");
        // Order is the instruction, so the user turn must come first.
        assert!(
            text.find("## user").unwrap() < text.find("## assistant").unwrap(),
            "{text}"
        );
    }

    #[test]
    fn a_payload_is_reported_by_size_rather_than_inlined() {
        let result = GetPromptResult {
            description: None,
            messages: vec![PromptMessage {
                role: PromptMessageRole::User,
                content: PromptMessageContent::Image {
                    image: rmcp::model::Annotated {
                        raw: rmcp::model::RawImageContent {
                            data: "AAAA".to_string(),
                            mime_type: "image/png".to_string(),
                            meta: None,
                        },
                        annotations: None,
                    },
                },
            }],
        };

        let text = render_prompt_messages("with_image", &result);
        assert!(text.contains("image/png"), "{text}");
        assert!(text.contains("4 base64 characters"), "{text}");
        assert!(
            !text.contains("AAAA"),
            "the payload must not be inlined: {text}"
        );
    }

    #[test]
    fn key_value_pairs_become_the_protocol_argument_map() {
        let pairs = std::collections::BTreeMap::from([("query".to_string(), "notes".to_string())]);
        let map = prompt_arguments_from_pairs(pairs).expect("a non-empty map is arguments");
        assert_eq!(map.get("query"), Some(&json!("notes")));

        // No arguments and an empty object are the same request.
        assert_eq!(
            prompt_arguments_from_pairs(std::collections::BTreeMap::new()),
            None
        );
    }

    #[test]
    fn empty_messages_are_stated_not_rendered_blank() {
        let result = GetPromptResult {
            description: None,
            messages: Vec::new(),
        };
        let text = render_prompt_messages("empty", &result);
        assert!(text.contains("empty returned no messages"), "{text}");
    }

    #[tokio::test]
    async fn the_router_lists_and_gets_a_real_prompt() {
        let service = MockMcpService::new(Vec::new(), |_| {
            Ok(rmcp::model::CallToolResult::success(Vec::new()))
        })
        .with_prompt("getting_started", "Introduce Basic Memory", &[])
        .with_prompt_messages(
            "getting_started",
            vec![PromptMessage::new_text(
                PromptMessageRole::User,
                "Show me around.",
            )],
        );
        let mut router = MCPToolRouter::new();
        router.register_client(McpClient::with_service(
            "basic-memory",
            Vec::new(),
            Arc::new(service),
        ));

        let listing = router.list_prompts(None).await.unwrap();
        assert!(listing.contains("getting_started"), "{listing}");

        let content = router
            .get_prompt("basic-memory", "getting_started", None)
            .await
            .unwrap();
        assert!(content.contains("Show me around."), "{content}");
    }

    #[tokio::test]
    async fn the_arguments_reach_the_server() {
        // The model's `arguments` object has to survive the trip: a prompt
        // fetched without its placeholders is a template, not an instruction.
        let service = Arc::new(
            MockMcpService::new(Vec::new(), |_| {
                Ok(rmcp::model::CallToolResult::success(Vec::new()))
            })
            .with_prompt("search_knowledge_base", "Search", &[("query", true)]),
        );
        // An `Arc` clone, not a service clone: the router takes ownership, and
        // the recorded calls are the assertion.
        let handle = service.clone();
        let mut router = MCPToolRouter::new();
        router.register_client(McpClient::with_service("basic-memory", Vec::new(), service));

        let mut arguments = JsonObject::new();
        arguments.insert("query".to_string(), json!("deployment notes"));
        router
            .get_prompt("basic-memory", "search_knowledge_base", Some(arguments))
            .await
            .unwrap();

        let calls = handle.prompt_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "search_knowledge_base");
        assert_eq!(
            calls[0].1.as_ref().and_then(|args| args.get("query")),
            Some(&json!("deployment notes"))
        );
    }

    #[tokio::test]
    async fn a_prompt_the_server_never_listed_is_an_error() {
        // "The server has no such prompt" and "the server composed nothing" are
        // different facts, and only the first is an error.
        let router = router_with(&[("getting_started", "Introduce Basic Memory", &[])]);

        let error = router
            .get_prompt("basic-memory", "never_listed", None)
            .await
            .expect_err("a prompt the server never advertised must not look empty");
        assert!(error.to_string().contains("never_listed"), "{error}");
    }

    /// A router with one server that publishes a prompt and one that implements
    /// no listing method at all (`canva` answers `-32601`).
    fn router_with_a_silent_server() -> MCPToolRouter {
        let good = MockMcpService::new(Vec::new(), |_| {
            Ok(rmcp::model::CallToolResult::success(Vec::new()))
        })
        .with_prompt("getting_started", "Introduce Basic Memory", &[]);
        let bad = MockMcpService::new(Vec::new(), |_| {
            Ok(rmcp::model::CallToolResult::success(Vec::new()))
        })
        .without_listings();
        let mut router = MCPToolRouter::new();
        router.register_client(McpClient::with_service(
            "basic-memory",
            Vec::new(),
            Arc::new(good),
        ));
        router.register_client(McpClient::with_service("canva", Vec::new(), Arc::new(bad)));
        router
    }

    #[tokio::test]
    async fn a_server_that_cannot_answer_does_not_hide_another_servers_prompts() {
        // The listing is not "all or nothing": canva's `-32601` is a fact about
        // canva, and folding it into a failure would make basic-memory's prompts
        // unreachable through a call that never mentions canva.
        let listing = router_with_a_silent_server()
            .list_prompts(None)
            .await
            .unwrap();

        assert!(listing.contains("getting_started"), "{listing}");
        assert!(listing.contains("## Did not answer"), "{listing}");
        assert!(listing.contains("canva"), "{listing}");
    }

    #[tokio::test]
    async fn when_no_server_answers_the_listing_says_so() {
        // "Nobody could answer" and "everybody published none" are different
        // facts, and only the second may read as "No prompts".
        let bad = MockMcpService::new(Vec::new(), |_| {
            Ok(rmcp::model::CallToolResult::success(Vec::new()))
        })
        .without_listings();
        let mut router = MCPToolRouter::new();
        router.register_client(McpClient::with_service("canva", Vec::new(), Arc::new(bad)));

        let listing = router.list_prompts(None).await.unwrap();

        assert!(listing.contains("No server answered"), "{listing}");
        assert!(!listing.contains("No prompts"), "{listing}");
    }

    #[tokio::test]
    async fn an_unknown_server_lists_the_connected_ones() {
        let router = router_with(&[]);

        let error = router
            .list_prompts(Some("nope"))
            .await
            .expect_err("an unknown server must not silently list nothing");
        assert!(error.to_string().contains("basic-memory"), "{error}");

        let error = router
            .get_prompt("nope", "getting_started", None)
            .await
            .expect_err("an unknown server must not look like a missing prompt");
        assert!(error.to_string().contains("basic-memory"), "{error}");
    }

    #[tokio::test]
    async fn an_empty_router_refuses_rather_than_reporting_no_prompts() {
        let error = MCPToolRouter::new()
            .list_prompts(None)
            .await
            .expect_err("no servers is an error, not an empty listing");
        assert!(error.to_string().contains("no MCP servers"), "{error}");
    }

    // ── the prompt tools' own risk ──────────────────────────────────────

    #[test]
    fn the_prompt_tools_are_high_unless_something_declares_otherwise() {
        // No config in a test process, so `try_settings()` is `None` — exactly
        // the "nobody declared anything" path, and it must be the restrictive
        // one.
        for tool in McpPromptTool::ALL {
            assert_eq!(
                prompt_tool_risk(tool),
                CapabilityRisk::High,
                "{} must default to High",
                tool.name()
            );
        }
    }

    #[test]
    fn a_listing_and_a_get_are_declared_separately() {
        let mut mcp = crate::config::McpSettings::default();
        assert_eq!(
            prompt_tool_risk_with(McpPromptTool::List, &mcp),
            CapabilityRisk::High
        );
        assert_eq!(
            prompt_tool_risk_with(McpPromptTool::Get, &mcp),
            CapabilityRisk::High
        );

        // Declaring the listing key moves the listing and not the get…
        mcp.prompt_list_risk = Some(CapabilityRisk::Write);
        assert_eq!(
            prompt_tool_risk_with(McpPromptTool::List, &mcp),
            CapabilityRisk::Write
        );
        assert_eq!(
            prompt_tool_risk_with(McpPromptTool::Get, &mcp),
            CapabilityRisk::High,
            "the get must keep its own default"
        );

        // …and the get key moves only the get.
        mcp.prompt_get_risk = Some(CapabilityRisk::Read);
        assert_eq!(
            prompt_tool_risk_with(McpPromptTool::Get, &mcp),
            CapabilityRisk::Read
        );
        assert_eq!(
            prompt_tool_risk_with(McpPromptTool::List, &mcp),
            CapabilityRisk::Write
        );
    }
}
