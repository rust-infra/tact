//! MCP **resources**, exposed to the agent as two Codex-compatible tools.
//!
//! Tools are not the only primitive MCP defines. A server can also publish
//! *resources* — read-only content addressed by URI — and the resources are
//! often the part that makes a server make sense: Basic Memory tells a freshly
//! connected model to "read the `memory://ai_assistant_guide` resource", and
//! without a way to read it that instruction is a dead end.
//!
//! Tact does not put resources in the tool list as `mcp__…` entries, because
//! they are not tools: they have no input schema and are addressed by URI. It
//! follows Codex instead, which exposes three native tools —
//! `list_mcp_resources`, `list_mcp_resource_templates`, `read_mcp_resource` —
//! and Tact implements all three.
//!
//! Templates matter because a server whose resources are *template*-addressed
//! publishes nothing through `resources/list`. Without the third tool such a
//! server looks like one with no resources, and its URIs are not guessable:
//! `memory://{topic}` needs the placeholder vocabulary before anything can be
//! read. Tact reports the template; the model performs the substitution, which
//! keeps the read path byte-exact.
//!
//! These three names are resolved in `agent::tool_dispatch` against the live MCP
//! router, exactly like `mcp__…` names. They only exist while a server is
//! connected: with an empty router the tools are not advertised at all, so the
//! model is never handed a tool that cannot do anything.

use anyhow::{Context as _, Result, bail};
use rmcp::model::{
    ReadResourceRequestParams, ReadResourceResult, Resource, ResourceContents, ResourceTemplate,
};
use serde_json::json;

use super::{MCP_RESOURCE_TIMEOUT, MCPToolRouter, McpClient};
use crate::{ToolSpec, permission::CapabilityRisk};

/// List the resources connected servers expose (Codex's tool name).
pub const LIST_RESOURCES_TOOL: &str = "list_mcp_resources";
/// List the resource *templates* connected servers expose (Codex's tool name).
pub const LIST_RESOURCE_TEMPLATES_TOOL: &str = "list_mcp_resource_templates";
/// Read one resource by URI (Codex's tool name).
pub const READ_RESOURCE_TOOL: &str = "read_mcp_resource";

/// Which resource tool a resolved tool call is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpResourceTool {
    List,
    Templates,
    Read,
}

impl McpResourceTool {
    /// Every tool, in a stable order: the two listings first, so the URI a
    /// listing prints is adjacent to the read that consumes it.
    pub const ALL: [Self; 3] = [Self::List, Self::Templates, Self::Read];

    /// Resolves a tool name, or `None` when it is not a resource tool.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            LIST_RESOURCES_TOOL => Some(Self::List),
            LIST_RESOURCE_TEMPLATES_TOOL => Some(Self::Templates),
            READ_RESOURCE_TOOL => Some(Self::Read),
            _ => None,
        }
    }

    /// The exact name the agent must call.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::List => LIST_RESOURCES_TOOL,
            Self::Templates => LIST_RESOURCE_TEMPLATES_TOOL,
            Self::Read => READ_RESOURCE_TOOL,
        }
    }

    /// The agent-facing schema.
    #[must_use]
    pub fn spec(self) -> ToolSpec {
        match self {
            Self::List => ToolSpec {
                name: LIST_RESOURCES_TOOL.to_string(),
                description: Some(
                    "List the read-only resources (files, guides, schemas) the connected MCP \
                     servers publish. Pass `server` to inspect just one. Resources are addressed \
                     by URI; read one with `read_mcp_resource`."
                        .to_string(),
                ),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "server": {
                            "type": "string",
                            "description": "Only list this MCP server's resources."
                        }
                    }
                }),
            },
            Self::Templates => ToolSpec {
                name: LIST_RESOURCE_TEMPLATES_TOOL.to_string(),
                description: Some(
                    "List the resource *templates* the connected MCP servers publish. A template \
                     is a URI with `{…}` placeholders: substitute them and call \
                     `read_mcp_resource` with the resulting URI. Call this when \
                     `list_mcp_resources` comes back empty — a server can serve resources it \
                     never enumerates."
                        .to_string(),
                ),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "server": {
                            "type": "string",
                            "description": "Only list this MCP server's resource templates."
                        }
                    }
                }),
            },
            Self::Read => ToolSpec {
                name: READ_RESOURCE_TOOL.to_string(),
                description: Some(
                    "Read one MCP resource by its URI (see `list_mcp_resources`). Text content \
                     is returned as-is."
                        .to_string(),
                ),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "server": {
                            "type": "string",
                            "description": "The MCP server that owns the resource."
                        },
                        "uri": {
                            "type": "string",
                            "description": "The resource URI, exactly as `list_mcp_resources` printed it."
                        }
                    },
                    "required": ["server", "uri"]
                }),
            },
        }
    }
}

/// The risk of one of Tact's own resource tools.
///
/// These names belong to Tact rather than to a server, which is why no entry's
/// `tools.<name>.risk` can address them and they need `[mcp]`'s two keys instead.
/// Sits beside [`crate::permission::normalize_mcp_capability`], the same kind of
/// single sayer for server tools.
///
/// Listings and reads are separate keys because the acts are not the same: a
/// listing returns metadata, a read returns third-party content fetched over the
/// network. Both default to `High`, and so does an unresolvable config, so the
/// unconfigured path is the restrictive one.
#[must_use]
pub fn resource_tool_risk(tool: McpResourceTool) -> CapabilityRisk {
    match crate::config::try_settings() {
        Some(settings) => resource_tool_risk_with(tool, &settings.mcp),
        None => CapabilityRisk::High,
    }
}

/// [`resource_tool_risk`] against given settings.
///
/// Split out so the tool-to-key mapping is testable without installing a
/// process-global config.
#[must_use]
pub fn resource_tool_risk_with(
    tool: McpResourceTool,
    mcp: &crate::config::McpSettings,
) -> CapabilityRisk {
    match tool {
        McpResourceTool::List | McpResourceTool::Templates => mcp.resource_list_risk,
        McpResourceTool::Read => mcp.resource_read_risk,
    }
    .unwrap_or(CapabilityRisk::High)
}

impl McpClient {
    /// `resources/list` for this server.
    pub async fn list_resources(&self) -> Result<Vec<Resource>> {
        tokio::time::timeout(MCP_RESOURCE_TIMEOUT, self.service.list_resources())
            .await
            .with_context(|| {
                format!(
                    "MCP server {} did not list its resources within {}s",
                    self.server_name,
                    MCP_RESOURCE_TIMEOUT.as_secs()
                )
            })?
            .with_context(|| format!("failed to list resources from {}", self.server_name))
    }

    /// `resources/templates/list` for this server.
    pub async fn list_resource_templates(&self) -> Result<Vec<ResourceTemplate>> {
        tokio::time::timeout(MCP_RESOURCE_TIMEOUT, self.service.list_resource_templates())
            .await
            .with_context(|| {
                format!(
                    "MCP server {} did not list its resource templates within {}s",
                    self.server_name,
                    MCP_RESOURCE_TIMEOUT.as_secs()
                )
            })?
            .with_context(|| {
                format!(
                    "failed to list resource templates from {}",
                    self.server_name
                )
            })
    }

    /// `resources/read` for one URI on this server.
    pub async fn read_resource(&self, uri: &str) -> Result<ReadResourceResult> {
        tokio::time::timeout(
            MCP_RESOURCE_TIMEOUT,
            self.service.read_resource(uri.to_string()),
        )
        .await
        .with_context(|| {
            format!(
                "MCP server {} did not read {uri} within {}s",
                self.server_name,
                MCP_RESOURCE_TIMEOUT.as_secs()
            )
        })?
        .with_context(|| format!("failed to read {uri} from {}", self.server_name))
    }
}

impl MCPToolRouter {
    /// The resource tools, or none when no server is connected.
    ///
    /// Advertising them unconditionally would hand the model a tool that can
    /// only ever answer "no MCP servers are connected"; with an empty router the
    /// names are unresolvable and never appear.
    #[must_use]
    pub fn resource_tool_specs(&self) -> Vec<ToolSpec> {
        if self.clients.is_empty() {
            return Vec::new();
        }
        McpResourceTool::ALL
            .iter()
            .map(|tool| tool.spec())
            .collect()
    }

    /// Every resource of every connected server, or of one named server.
    ///
    /// Returns a rendered listing rather than the raw models: this is a tool
    /// result the model reads, and URIs are what it needs to copy.
    pub async fn list_resources(&self, server: Option<&str>) -> Result<String> {
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

        let mut listings = Vec::with_capacity(selected.len());
        for name in selected {
            let client = &self.clients[name];
            listings.push((name.to_string(), client.list_resources().await?));
        }
        Ok(render_resource_listing(&listings))
    }

    /// Every resource template of every connected server, or of one named
    /// server.
    ///
    /// The templates counterpart of [`Self::list_resources`], and the only way
    /// to discover the URIs of a server that publishes nothing through
    /// `resources/list`.
    pub async fn list_resource_templates(&self, server: Option<&str>) -> Result<String> {
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

        let mut listings = Vec::with_capacity(selected.len());
        for name in selected {
            let client = &self.clients[name];
            listings.push((name.to_string(), client.list_resource_templates().await?));
        }
        Ok(render_resource_template_listing(&listings))
    }

    /// Reads one resource and renders its contents for the model.
    pub async fn read_resource(&self, server: &str, uri: &str) -> Result<String> {
        let client = self.clients.get(server).with_context(|| {
            format!(
                "unknown MCP server {server} (connected: {})",
                self.known_servers().join(", ")
            )
        })?;
        let result = client.read_resource(uri).await?;
        Ok(render_resource_contents(uri, &result.contents))
    }

    /// Connected server names, sorted.
    fn known_servers(&self) -> Vec<String> {
        let mut names: Vec<String> = self.clients.keys().cloned().collect();
        names.sort();
        names
    }
}

/// Renders `list_mcp_resources`'s result.
///
/// One `## <server>` section per server, in the order given (already sorted by
/// the caller), so the model can copy a URI straight out of the line it read.
#[must_use]
pub fn render_resource_listing(listings: &[(String, Vec<Resource>)]) -> String {
    let total: usize = listings.iter().map(|(_, resources)| resources.len()).sum();
    if total == 0 {
        // A server whose resources are template-addressed enumerates nothing
        // here, so the next call is named rather than left to be guessed.
        return format!(
            "No resources: {} connected server(s) expose none through `resources/list`.\n\
             Call `list_mcp_resource_templates` — a server can serve resources it never \
             enumerates, and a template's `{{…}}` placeholders have to be filled before \
             `read_mcp_resource` can use it.",
            listings.len()
        );
    }

    let mut out = format!("{total} resource(s) across {} server(s):", listings.len());
    for (server, resources) in listings {
        out.push_str(&format!("\n\n## {server}"));
        for resource in resources {
            out.push_str("\n- ");
            out.push_str(&resource.uri);
            out.push_str(" — ");
            out.push_str(resource.name.as_str());
            if let Some(mime) = &resource.mime_type {
                out.push_str(&format!(" ({mime})"));
            }
            if let Some(description) = &resource.description {
                out.push_str(&format!("\n  {}", description.replace('\n', " ")));
            }
        }
    }
    out
}

/// Renders `list_mcp_resource_templates`'s result.
///
/// One `## <server>` section per server, and the substitution sentence once:
/// without it a template reads as a URI that simply fails, and the model copies
/// `memory://{topic}` verbatim into `read_mcp_resource` and gets an error it
/// cannot interpret.
#[must_use]
pub fn render_resource_template_listing(listings: &[(String, Vec<ResourceTemplate>)]) -> String {
    let total: usize = listings.iter().map(|(_, templates)| templates.len()).sum();
    if total == 0 {
        return format!(
            "No resource templates: {} connected server(s) expose none through \
             `resources/templates/list`.",
            listings.len()
        );
    }

    let mut out = format!(
        "{total} resource template(s) across {} server(s).\n\
         Fill each `{{…}}` placeholder to form a real URI, then call `read_mcp_resource` with it:",
        listings.len()
    );
    for (server, templates) in listings {
        out.push_str(&format!("\n\n## {server}"));
        for template in templates {
            out.push_str("\n- ");
            out.push_str(&template.uri_template);
            out.push_str(" — ");
            out.push_str(&template.name);
            if let Some(mime) = &template.mime_type {
                out.push_str(&format!(" ({mime})"));
            }
            if let Some(description) = &template.description {
                out.push_str(&format!("\n  {}", description.replace('\n', " ")));
            }
        }
    }
    out
}

/// Renders `read_mcp_resource`'s result.
///
/// Blobs are reported by size instead of being inlined: a base64 payload in a
/// tool result is unreadable to the model and expensive to carry.
#[must_use]
pub fn render_resource_contents(uri: &str, contents: &[ResourceContents]) -> String {
    if contents.is_empty() {
        return format!("{uri} returned no content.");
    }
    let mut sections = Vec::with_capacity(contents.len());
    for content in contents {
        let (content_uri, body) = match content {
            ResourceContents::TextResourceContents { uri, text, .. } => (uri.clone(), text.clone()),
            ResourceContents::BlobResourceContents { uri, blob, .. } => (
                uri.clone(),
                format!(
                    "[binary content, {} base64 characters — not inlined]",
                    blob.len()
                ),
            ),
        };
        sections.push(format!("# {content_uri}\n\n{body}"));
    }
    sections.join("\n\n")
}

/// Builds the request params `resources/read` needs.
#[must_use]
pub fn read_params(uri: &str) -> ReadResourceRequestParams {
    ReadResourceRequestParams {
        meta: None,
        uri: uri.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use rmcp::model::RawResource;

    use super::*;
    use crate::mcp::{MCPToolRouter, McpClient, MockMcpService};

    fn router_with(resources: &[(&str, &str, &str)]) -> MCPToolRouter {
        let service = MockMcpService::new(Vec::new(), |_| {
            Ok(rmcp::model::CallToolResult::success(Vec::new()))
        });
        let service = resources
            .iter()
            .fold(service, |service, (uri, name, text)| {
                service.with_text_resource(uri, name, text)
            });
        let mut router = MCPToolRouter::new();
        router.register_client(McpClient::with_service(
            "basic-memory",
            Vec::new(),
            Arc::new(service),
        ));
        router
    }

    #[test]
    fn the_resource_tools_exist_only_while_a_server_is_connected() {
        // With no servers the names are unresolvable, so advertising them would
        // hand the model a tool whose only possible answer is "nothing".
        assert!(MCPToolRouter::new().resource_tool_specs().is_empty());

        let specs = router_with(&[]).resource_tool_specs();
        let names: Vec<&str> = specs.iter().map(|spec| spec.name.as_str()).collect();
        // Both listings before the read, so the URI a listing prints sits next
        // to the tool that consumes it.
        assert_eq!(
            names,
            [
                LIST_RESOURCES_TOOL,
                LIST_RESOURCE_TEMPLATES_TOOL,
                READ_RESOURCE_TOOL
            ]
        );
    }

    #[test]
    fn reading_declares_the_arguments_it_cannot_infer() {
        let spec = McpResourceTool::Read.spec();
        assert_eq!(
            spec.input_schema["required"],
            serde_json::json!(["server", "uri"])
        );
    }

    #[test]
    fn a_tool_name_round_trips() {
        for tool in McpResourceTool::ALL {
            assert_eq!(McpResourceTool::from_name(tool.name()), Some(tool));
        }
        assert_eq!(McpResourceTool::from_name("mcp__x__y"), None);
    }

    #[test]
    fn a_listing_names_the_uri_the_model_must_copy() {
        let listing = vec![(
            "basic-memory".to_string(),
            vec![
                Resource::new(RawResource::new("memory://guide", "Guide"), None),
                Resource::new(
                    RawResource {
                        mime_type: Some("text/markdown".to_string()),
                        description: Some("Line one\nline two".to_string()),
                        ..RawResource::new("memory://notes", "Notes")
                    },
                    None,
                ),
            ],
        )];

        let text = render_resource_listing(&listing);
        assert!(text.contains("2 resource(s) across 1 server(s)"), "{text}");
        assert!(text.contains("## basic-memory"), "{text}");
        assert!(text.contains("- memory://guide — Guide"), "{text}");
        assert!(text.contains("(text/markdown)"), "{text}");
        // A multi-line description would otherwise break the bullet.
        assert!(text.contains("Line one line two"), "{text}");
    }

    #[test]
    fn an_empty_listing_says_so_instead_of_printing_nothing() {
        let text = render_resource_listing(&[("basic-memory".to_string(), Vec::new())]);
        assert!(text.contains("No resources"), "{text}");
        assert!(text.contains("1 connected server(s)"), "{text}");
        // The next call is named, so a server whose URIs are template-based is
        // not written off as having nothing. It used to say templates "are not
        // listed yet", which was true and useless.
        assert!(text.contains("list_mcp_resource_templates"), "{text}");
    }

    #[test]
    fn a_template_listing_teaches_the_substitution() {
        let template = ResourceTemplate::new(
            rmcp::model::RawResourceTemplate {
                uri_template: "memory://{topic}".to_string(),
                name: "Note by topic".to_string(),
                title: None,
                description: Some("One note per topic\nacross lines".to_string()),
                mime_type: Some("text/markdown".to_string()),
                icons: None,
            },
            None,
        );
        let listing = vec![("basic-memory".to_string(), vec![template])];

        let text = render_resource_template_listing(&listing);
        assert!(text.contains("1 resource template(s)"), "{text}");
        assert!(text.contains("## basic-memory"), "{text}");
        assert!(
            text.contains("- memory://{topic} — Note by topic"),
            "{text}"
        );
        assert!(text.contains("(text/markdown)"), "{text}");
        // Echoed verbatim into `read_mcp_resource` a template is just a URI that
        // fails, so the placeholder rule has to be stated.
        assert!(text.contains("placeholder"), "{text}");
        assert!(text.contains("One note per topic across lines"), "{text}");
    }

    #[test]
    fn a_template_name_round_trips() {
        assert_eq!(
            McpResourceTool::from_name(LIST_RESOURCE_TEMPLATES_TOOL),
            Some(McpResourceTool::Templates)
        );
        assert_eq!(
            McpResourceTool::Templates.name(),
            LIST_RESOURCE_TEMPLATES_TOOL
        );
    }

    #[test]
    fn no_templates_is_stated_not_rendered_blank() {
        let text = render_resource_template_listing(&[("basic-memory".to_string(), Vec::new())]);
        assert!(text.contains("No resource templates"), "{text}");
        assert!(text.contains("1 connected server(s)"), "{text}");
    }

    #[tokio::test]
    async fn a_template_only_server_no_longer_looks_empty() {
        // The whole point: `resources/list` cannot describe a
        // template-addressed server, so without the third tool it is
        // indistinguishable from a server with nothing to offer.
        let service = MockMcpService::new(Vec::new(), |_| {
            Ok(rmcp::model::CallToolResult::success(Vec::new()))
        })
        .with_resource_template("memory://{topic}", "Note by topic");
        let mut router = MCPToolRouter::new();
        router.register_client(McpClient::with_service(
            "basic-memory",
            Vec::new(),
            Arc::new(service),
        ));

        let resources = router.list_resources(None).await.unwrap();
        assert!(resources.contains("No resources"), "{resources}");
        assert!(
            resources.contains("list_mcp_resource_templates"),
            "{resources}"
        );

        let templates = router.list_resource_templates(None).await.unwrap();
        assert!(templates.contains("memory://{topic}"), "{templates}");

        // And the one named server is addressable too.
        let scoped = router
            .list_resource_templates(Some("basic-memory"))
            .await
            .unwrap();
        assert!(scoped.contains("memory://{topic}"), "{scoped}");

        let error = router
            .list_resource_templates(Some("nope"))
            .await
            .expect_err("an unknown server must not list nothing silently");
        assert!(error.to_string().contains("basic-memory"), "{error}");
    }

    #[test]
    fn a_blob_is_reported_by_size_rather_than_inlined() {
        let contents = vec![
            ResourceContents::text("hello", "memory://text"),
            ResourceContents::BlobResourceContents {
                uri: "memory://image".to_string(),
                mime_type: Some("image/png".to_string()),
                blob: "AAAA".to_string(),
                meta: None,
            },
        ];

        let text = render_resource_contents("memory://text", &contents);
        assert!(text.contains("# memory://text"), "{text}");
        assert!(text.contains("hello"), "{text}");
        assert!(text.contains("4 base64 characters"), "{text}");
        assert!(
            !text.contains("AAAA"),
            "the blob must not be inlined: {text}"
        );
    }

    #[test]
    fn empty_contents_are_stated_not_rendered_blank() {
        let text = render_resource_contents("memory://empty", &[]);
        assert!(
            text.contains("memory://empty returned no content"),
            "{text}"
        );
    }

    #[tokio::test]
    async fn the_router_lists_and_reads_a_real_resource() {
        let router = router_with(&[("memory://guide", "Guide", "Read me first.")]);

        let listing = router.list_resources(None).await.unwrap();
        assert!(listing.contains("memory://guide"), "{listing}");

        let content = router
            .read_resource("basic-memory", "memory://guide")
            .await
            .unwrap();
        assert!(content.contains("Read me first."), "{content}");
    }

    #[tokio::test]
    async fn an_unknown_server_lists_the_connected_ones() {
        let router = router_with(&[]);

        let error = router
            .list_resources(Some("nope"))
            .await
            .expect_err("an unknown server must not silently list nothing");
        assert!(error.to_string().contains("basic-memory"), "{error}");

        let error = router
            .read_resource("nope", "memory://guide")
            .await
            .expect_err("an unknown server must not look like a missing resource");
        assert!(error.to_string().contains("basic-memory"), "{error}");
    }

    #[tokio::test]
    async fn an_empty_router_refuses_rather_than_reporting_no_resources() {
        let error = MCPToolRouter::new()
            .list_resources(None)
            .await
            .expect_err("no servers is an error, not an empty listing");
        assert!(error.to_string().contains("no MCP servers"), "{error}");
    }
    // ── the resource tools' own risk ────────────────────────────────────

    #[test]
    fn the_resource_tools_are_high_unless_something_declares_otherwise() {
        // No config in a test process, so `try_settings()` is `None` — which is
        // exactly the "nobody declared anything" path, and it must be the
        // restrictive one.
        for tool in McpResourceTool::ALL {
            assert_eq!(
                resource_tool_risk(tool),
                CapabilityRisk::High,
                "{} must default to High",
                tool.name()
            );
        }
    }

    #[test]
    fn a_listing_and_a_read_are_declared_separately() {
        // The two keys exist because the acts are not the same, so the mapping
        // from tool to key is the property under test.
        let mut mcp = crate::config::McpSettings::default();
        assert_eq!(
            resource_tool_risk_with(McpResourceTool::List, &mcp),
            CapabilityRisk::High
        );
        assert_eq!(
            resource_tool_risk_with(McpResourceTool::Read, &mcp),
            CapabilityRisk::High
        );

        // Declaring the listing key moves both listings and neither the read…
        mcp.resource_list_risk = Some(CapabilityRisk::Write);
        assert_eq!(
            resource_tool_risk_with(McpResourceTool::List, &mcp),
            CapabilityRisk::Write
        );
        assert_eq!(
            resource_tool_risk_with(McpResourceTool::Templates, &mcp),
            CapabilityRisk::Write
        );
        assert_eq!(
            resource_tool_risk_with(McpResourceTool::Read, &mcp),
            CapabilityRisk::High,
            "the read must keep its own default"
        );

        // …and the read key moves only the read.
        mcp.resource_read_risk = Some(CapabilityRisk::Read);
        assert_eq!(
            resource_tool_risk_with(McpResourceTool::Read, &mcp),
            CapabilityRisk::Read
        );
        assert_eq!(
            resource_tool_risk_with(McpResourceTool::List, &mcp),
            CapabilityRisk::Write
        );
    }
}
