//! Product implementation of the kernel tool port.

use std::{
    path::{Component, Path},
    sync::{
        RwLock,
        atomic::{AtomicU64, Ordering},
    },
};

use async_trait::async_trait;
use serde_json::{Value, json};
use tact_agent_api::{
    AgentContentBlock, CancelToken, ResolvedToolCall, ResourceAccess, ResourceKey, ResourceScope,
    ToolCall, ToolDescriptor, ToolDirective, ToolError, ToolErrorKind, ToolExecutionContext,
    ToolExecutionError, ToolExecutionErrorKind, ToolExecutionResult, ToolOutcome, ToolPort,
    ToolResolver, ToolSnapshot, ToolSpec,
};

use crate::runtime::tool_presentation::{argument_pair, presentation_for, presentation_metadata};
use crate::{
    agent::tool_schedule::ToolResources,
    compact::{persist_large_output, persist_large_output_over_tokens},
    consts::TactPath,
    mcp::{McpCatalogHandle, McpPromptTool, McpResourceTool},
    permission::CapabilityRisk,
    security::{
        RedactionConfig, SecurityConfig,
        redact::level_for_call,
        sensitive::{Scanner, Tier},
    },
    tool::{ToolContext, ToolEffect, ToolMetadata, ToolRouter},
};

enum ResolvedKind {
    Native {
        metadata: &'static ToolMetadata,
    },
    Mcp {
        full_name: String,
        server: String,
        tool: String,
    },
    Resource(McpResourceTool),
    Prompt(McpPromptTool),
}

/// Tact's native and MCP tools behind one kernel-facing capability.
pub(crate) struct TactToolPort {
    tools: ToolRouter,
    mcp: McpCatalogHandle,
    context: ToolContext,
    scanner: Scanner,
    redaction: RedactionConfig,
    hide_compact: bool,
    snapshot: RwLock<ToolSnapshot>,
    next_generation: AtomicU64,
}

impl TactToolPort {
    pub(crate) fn new(
        tools: ToolRouter,
        mcp: McpCatalogHandle,
        context: ToolContext,
        security: SecurityConfig,
        scanner: Scanner,
        hide_compact: bool,
    ) -> Self {
        Self {
            tools,
            mcp,
            context,
            scanner,
            redaction: security.redaction,
            hide_compact,
            snapshot: RwLock::new(ToolSnapshot {
                specs: Vec::new(),
                generation: 0,
            }),
            next_generation: AtomicU64::new(1),
        }
    }

    pub(crate) fn mcp_catalog(&self) -> &McpCatalogHandle {
        &self.mcp
    }

    fn resolve_kind(&self, call: &ToolCall) -> Result<ResolvedKind, ToolError> {
        if let Ok(native) = self.tools.resolve(&call.name) {
            return Ok(ResolvedKind::Native {
                metadata: native.metadata(),
            });
        }
        if let Ok(Some(mcp)) = self.mcp.resolve_tool(&call.name) {
            return Ok(ResolvedKind::Mcp {
                full_name: mcp.full_name,
                server: mcp.server,
                tool: mcp.tool,
            });
        }
        if let Some(tool) = McpResourceTool::from_name(&call.name) {
            return Ok(ResolvedKind::Resource(tool));
        }
        if let Some(tool) = McpPromptTool::from_name(&call.name) {
            return Ok(ResolvedKind::Prompt(tool));
        }
        Err(ToolError::new(
            ToolErrorKind::UnknownTool,
            format!("unknown tool: {}", call.name),
        ))
    }

    fn descriptor(&self, call: &ToolCall, kind: &ResolvedKind) -> ToolDescriptor {
        let (description, metadata) = match kind {
            ResolvedKind::Native { metadata } => {
                let (arg_full, arg_summary) = argument_pair(metadata, &call.input, None, None);
                let presentation = presentation_for(metadata, &call.input);
                (
                    metadata.description.to_string(),
                    json!({
                        "kind": "native",
                        "stable_name": metadata.name,
                        "risk": native_risk(metadata, &call.input, &self.scanner).to_string(),
                        "permission_prompt": prompt_policy(metadata),
                        "sensitive": sensitive_json(metadata, &call.input, &self.scanner),
                        "redaction_target": metadata.permission.target(&call.input),
                        "auto_approved": false,
                        "arg_full": arg_full,
                        "arg_summary": arg_summary,
                        "presentation": presentation_metadata(&presentation),
                    }),
                )
            }
            ResolvedKind::Mcp {
                full_name,
                server,
                tool,
            } => (
                self.mcp_description(full_name),
                json!({
                    "kind": "mcp",
                    "stable_name": full_name,
                    "server": server,
                    "tool": tool,
                    "risk": self.mcp.risk_for(server, tool).to_string(),
                    "permission_prompt": {"kind": "json"},
                    "sensitive": Value::Null,
                    "redaction_target": Value::Null,
                    "auto_approved": self.mcp.is_auto_approved(server, tool),
                    "arg_full": call.input.to_string(),
                    "arg_summary": generic_arg_summary(&call.input),
                }),
            ),
            ResolvedKind::Resource(tool) => (
                self.mcp_description(tool.name()),
                json!({
                    "kind": "mcp_resource",
                    "stable_name": tool.name(),
                    "risk": self.mcp.resource_risk(*tool).to_string(),
                    "permission_prompt": {"kind": "json"},
                    "sensitive": Value::Null,
                    "redaction_target": Value::Null,
                    "auto_approved": false,
                    "arg_full": call.input.to_string(),
                    "arg_summary": generic_arg_summary(&call.input),
                }),
            ),
            ResolvedKind::Prompt(tool) => (
                self.mcp_description(tool.name()),
                json!({
                    "kind": "mcp_prompt",
                    "stable_name": tool.name(),
                    "risk": self.mcp.prompt_risk(*tool).to_string(),
                    "permission_prompt": {"kind": "json"},
                    "sensitive": Value::Null,
                    "redaction_target": Value::Null,
                    "auto_approved": false,
                    "arg_full": call.input.to_string(),
                    "arg_summary": generic_arg_summary(&call.input),
                }),
            ),
        };
        ToolDescriptor {
            name: call.name.clone(),
            description,
            metadata,
        }
    }

    fn mcp_description(&self, name: &str) -> String {
        self.mcp
            .all_tools()
            .into_iter()
            .chain(self.mcp.resource_tool_specs())
            .chain(self.mcp.prompt_tool_specs())
            .find(|spec| spec.name == name)
            .and_then(|spec| spec.description)
            .unwrap_or_default()
    }

    fn access(&self, kind: &ResolvedKind, input: &Value) -> ResourceAccess {
        let resources = match kind {
            ResolvedKind::Native { metadata } => {
                if metadata.name == crate::tool::SPAWN_SUBAGENT_METADATA.name
                    && input.get("worktree").and_then(Value::as_bool) == Some(true)
                {
                    ToolResources::independent()
                } else {
                    metadata.resources.resolve(input, &self.context.work_dir)
                }
            }
            ResolvedKind::Mcp { server, .. } => {
                crate::agent::tool_schedule::mcp_server_resources(server)
            }
            ResolvedKind::Resource(McpResourceTool::Read) => named_server(input)
                .map(crate::agent::tool_schedule::mcp_server_resources)
                .unwrap_or_else(ToolResources::barrier),
            ResolvedKind::Prompt(McpPromptTool::Get) => named_server(input)
                .map(crate::agent::tool_schedule::mcp_server_resources)
                .unwrap_or_else(ToolResources::barrier),
            ResolvedKind::Resource(_) | ResolvedKind::Prompt(_) => ToolResources::barrier(),
        };
        resources_to_access(resources)
    }

    fn redact_result(&self, kind: &ResolvedKind, input: &Value, text: String) -> String {
        let (sensitive, target) = match kind {
            ResolvedKind::Native { metadata } => (
                metadata
                    .permission
                    .sensitive(input, &self.scanner)
                    .is_some(),
                metadata.permission.target(input),
            ),
            _ => (false, None),
        };
        let level = level_for_call(&self.redaction, sensitive, target.as_deref());
        if level == crate::security::RedactionLevel::Off {
            text
        } else {
            crate::security::redact::redact(&text, level, &self.redaction.extra_patterns)
                .into_owned()
        }
    }

    fn execution_context(
        &self,
        kind: &ResolvedKind,
        call: &ResolvedToolCall,
        context: &ToolExecutionContext,
    ) -> ToolContext {
        let (sensitive, target) = match kind {
            ResolvedKind::Native { metadata } => (
                metadata
                    .permission
                    .sensitive(&call.input, &self.scanner)
                    .is_some(),
                metadata.permission.target(&call.input),
            ),
            _ => (false, None),
        };
        let level = level_for_call(&self.redaction, sensitive, target.as_deref());
        let mut invocation = self
            .context
            .for_invocation_with_redaction(&call.call_id, level);
        invocation.progress_reporter = invocation
            .progress_reporter
            .forwarding(context.progress.clone());
        // Tool implementations still observe the product's shared cancel flag.
        // Wiring a detached flag here only recorded cancellation that happened
        // before the call and left long-running tools (notably bash) running.
        invocation.cancel_flag = self.context.cancel_flag.clone();
        invocation
    }

    async fn execute_native(
        &self,
        metadata: &'static ToolMetadata,
        call: &ResolvedToolCall,
        context: &ToolExecutionContext,
    ) -> ToolExecutionResult {
        let invocation = self.execution_context(&ResolvedKind::Native { metadata }, call, context);
        let result = self
            .tools
            .call_result(&invocation, &call.name, call.input.clone())
            .await;
        invocation.progress_reporter.flush();
        match result {
            Ok(result) => {
                let mut text = self.redact_result(
                    &ResolvedKind::Native { metadata },
                    &call.input,
                    result.content,
                );
                let mut status = tact_agent_api::ToolExecutionStatus::Succeeded;
                let mut error = None;
                if matches!(
                    metadata.output,
                    crate::tool::OutputPolicy::PersistLargeOutput
                ) {
                    match persist_large_output(
                        &TactPath::new(&self.context.work_dir),
                        &call.call_id,
                        &text,
                    )
                    .await
                    {
                        Ok(persisted) => text = persisted,
                        Err(persist_error) => {
                            let message = format!("Error persisting large output: {persist_error}");
                            status = tact_agent_api::ToolExecutionStatus::Failed;
                            error = Some(ToolExecutionError {
                                kind: ToolExecutionErrorKind::OutputPersistence,
                                message: message.clone(),
                            });
                            text = message;
                        }
                    }
                }
                let mut outcome = ToolOutcome {
                    content: vec![AgentContentBlock::Text { text }],
                    directives: directives(result.effects),
                    metadata: None,
                };
                if metadata.name == "compact"
                    && status == tact_agent_api::ToolExecutionStatus::Succeeded
                {
                    outcome.directives.push(ToolDirective::RequestCompaction {
                        focus: call
                            .input
                            .get("focus")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                    });
                }
                if metadata.name == crate::tool::goal::GOAL_PROPOSE_COMPLETION_METADATA.name
                    && context.remaining_turns == Some(1)
                    && status == tact_agent_api::ToolExecutionStatus::Succeeded
                {
                    outcome.directives.push(ToolDirective::CompleteRun);
                }
                if let Some(image) = result.image {
                    outcome.content.push(tact_llm::neutral::agent_content_block(
                        &tact_llm::ContentBlock::Image { source: image },
                    ));
                }
                ToolExecutionResult {
                    status,
                    outcome,
                    error,
                }
            }
            Err(call_error) => ToolExecutionResult::failure(ToolExecutionError {
                kind: ToolExecutionErrorKind::Tool,
                message: format!("Error invoking tool {}: {call_error}", call.name),
            }),
        }
    }

    async fn execute_mcp(
        &self,
        full_name: &str,
        server: &str,
        tool: &str,
        call: &ResolvedToolCall,
        context: &ToolExecutionContext,
    ) -> ToolExecutionResult {
        let invocation = self.execution_context(
            &ResolvedKind::Mcp {
                full_name: full_name.to_string(),
                server: server.to_string(),
                tool: tool.to_string(),
            },
            call,
            context,
        );
        let result = self.mcp.call(full_name, call.input.clone()).await;
        invocation.progress_reporter.flush();
        match result {
            Ok(output) => {
                let output = self.redact_result(
                    &ResolvedKind::Mcp {
                        full_name: full_name.to_string(),
                        server: server.to_string(),
                        tool: tool.to_string(),
                    },
                    &call.input,
                    output,
                );
                let persisted = match self.mcp.output_token_limit(full_name) {
                    Some(limit) => {
                        persist_large_output_over_tokens(
                            &TactPath::new(&self.context.work_dir),
                            &call.call_id,
                            &output,
                            limit,
                        )
                        .await
                    }
                    None => {
                        persist_large_output(
                            &TactPath::new(&self.context.work_dir),
                            &call.call_id,
                            &output,
                        )
                        .await
                    }
                };
                match persisted {
                    Ok(content) => ToolExecutionResult::success(ToolOutcome::text(content)),
                    Err(persist_error) => ToolExecutionResult::failure(ToolExecutionError {
                        kind: ToolExecutionErrorKind::OutputPersistence,
                        message: format!("Error persisting large MCP output: {persist_error}"),
                    }),
                }
            }
            Err(error) => ToolExecutionResult::failure(ToolExecutionError {
                kind: ToolExecutionErrorKind::Mcp,
                message: format!("Error invoking MCP tool {full_name}: {error}"),
            }),
        }
    }

    async fn execute_resource(
        &self,
        tool: McpResourceTool,
        call: &ResolvedToolCall,
        context: &ToolExecutionContext,
    ) -> ToolExecutionResult {
        let invocation = self.execution_context(&ResolvedKind::Resource(tool), call, context);
        let server = call.input.get("server").and_then(Value::as_str);
        let result = match tool {
            McpResourceTool::List => self.mcp.list_resources(server).await,
            McpResourceTool::Templates => self.mcp.list_resource_templates(server).await,
            McpResourceTool::Read => {
                let Some(server) = server else {
                    return ToolExecutionResult::failure(ToolExecutionError {
                        kind: ToolExecutionErrorKind::Mcp,
                        message: "Error invoking read_mcp_resource: `server` is required".into(),
                    });
                };
                let Some(uri) = call.input.get("uri").and_then(Value::as_str) else {
                    return ToolExecutionResult::failure(ToolExecutionError {
                        kind: ToolExecutionErrorKind::Mcp,
                        message: "Error invoking read_mcp_resource: `uri` is required".into(),
                    });
                };
                self.mcp.read_resource(server, uri).await
            }
        };
        invocation.progress_reporter.flush();
        match result {
            Ok(text) => ToolExecutionResult::success(ToolOutcome::text(self.redact_result(
                &ResolvedKind::Resource(tool),
                &call.input,
                text,
            ))),
            Err(error) => ToolExecutionResult::failure(ToolExecutionError {
                kind: ToolExecutionErrorKind::Mcp,
                message: format!("Error invoking {}: {error}", tool.name()),
            }),
        }
    }

    async fn execute_prompt(
        &self,
        tool: McpPromptTool,
        call: &ResolvedToolCall,
        context: &ToolExecutionContext,
    ) -> ToolExecutionResult {
        let invocation = self.execution_context(&ResolvedKind::Prompt(tool), call, context);
        let server = call.input.get("server").and_then(Value::as_str);
        let result = match tool {
            McpPromptTool::List => self.mcp.list_prompts(server).await,
            McpPromptTool::Get => {
                let Some(server) = server else {
                    return ToolExecutionResult::failure(ToolExecutionError {
                        kind: ToolExecutionErrorKind::Mcp,
                        message: "Error invoking get_mcp_prompt: `server` is required".into(),
                    });
                };
                let Some(name) = call.input.get("name").and_then(Value::as_str) else {
                    return ToolExecutionResult::failure(ToolExecutionError {
                        kind: ToolExecutionErrorKind::Mcp,
                        message: "Error invoking get_mcp_prompt: `name` is required".into(),
                    });
                };
                match prompt_arguments(&call.input) {
                    Ok(arguments) => self.mcp.get_prompt(server, name, arguments).await,
                    Err(message) => Err(anyhow::anyhow!(message)),
                }
            }
        };
        invocation.progress_reporter.flush();
        match result {
            Ok(text) => ToolExecutionResult::success(ToolOutcome::text(self.redact_result(
                &ResolvedKind::Prompt(tool),
                &call.input,
                text,
            ))),
            Err(error) => ToolExecutionResult::failure(ToolExecutionError {
                kind: ToolExecutionErrorKind::Mcp,
                message: format!("Error invoking {}: {error}", tool.name()),
            }),
        }
    }
}

#[async_trait]
impl ToolResolver for TactToolPort {
    fn resolve(&self, call: &ToolCall) -> Result<ResolvedToolCall, ToolError> {
        let kind = self.resolve_kind(call)?;
        Ok(ResolvedToolCall {
            call_id: call.id.clone(),
            name: call.name.clone(),
            input: call.input.clone(),
            access: self.access(&kind, &call.input),
            descriptor: self.descriptor(call, &kind),
        })
    }
}

#[async_trait]
impl ToolPort for TactToolPort {
    async fn refresh(&self, cancel: CancelToken) -> Result<ToolSnapshot, ToolError> {
        if cancel.is_cancelled() {
            return Err(ToolError::new(
                ToolErrorKind::Adapter,
                "tool refresh cancelled",
            ));
        }
        let report = tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                return Err(ToolError::new(
                    ToolErrorKind::Adapter,
                    "tool refresh cancelled",
                ));
            }
            report = self.mcp.refresh_changed() => report,
        };
        for (server, refresh) in &report.changed {
            self.context
                .events
                .send(tact_protocol::AgentUpdate::Info(format!(
                    "[mcp] {server} changed its tool list: {}",
                    refresh.describe()
                )));
        }
        for (server, reason) in &report.failed {
            self.context
                .events
                .send(tact_protocol::AgentUpdate::Info(format!(
                    "[mcp] {server} tool-list refresh failed: {reason}"
                )));
        }

        let mut specs = self
            .tools
            .tool_specs()
            .iter()
            .map(tact_llm::neutral::neutral_tool)
            .collect::<Vec<_>>();
        if self.hide_compact {
            specs.retain(|spec| spec.name != "compact");
        }
        specs.extend(
            self.mcp
                .all_tools()
                .iter()
                .map(tact_llm::neutral::neutral_tool),
        );
        specs.extend(
            self.mcp
                .resource_tool_specs()
                .iter()
                .map(tact_llm::neutral::neutral_tool),
        );
        specs.extend(
            self.mcp
                .prompt_tool_specs()
                .iter()
                .map(tact_llm::neutral::neutral_tool),
        );

        let mut current = self
            .snapshot
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if same_specs(&current.specs, &specs) {
            return Ok(current.clone());
        }
        let generation = self.next_generation.fetch_add(1, Ordering::AcqRel);
        *current = ToolSnapshot { specs, generation };
        Ok(current.clone())
    }

    async fn execute(
        &self,
        call: ResolvedToolCall,
        context: ToolExecutionContext,
    ) -> Result<ToolExecutionResult, ToolError> {
        if context.cancel.is_cancelled() {
            return Ok(ToolExecutionResult::failure(ToolExecutionError {
                kind: ToolExecutionErrorKind::Tool,
                message: "Cancelled by user".into(),
            }));
        }
        let raw = ToolCall {
            id: call.call_id.clone(),
            name: call.name.clone(),
            input: call.input.clone(),
        };
        let result = match self.resolve_kind(&raw)? {
            ResolvedKind::Native { metadata } => {
                self.execute_native(metadata, &call, &context).await
            }
            ResolvedKind::Mcp {
                full_name,
                server,
                tool,
            } => {
                self.execute_mcp(&full_name, &server, &tool, &call, &context)
                    .await
            }
            ResolvedKind::Resource(tool) => self.execute_resource(tool, &call, &context).await,
            ResolvedKind::Prompt(tool) => self.execute_prompt(tool, &call, &context).await,
        };
        Ok(result)
    }
}

fn generic_arg_summary(input: &Value) -> String {
    let full = input.to_string();
    if full.chars().count() <= 120 {
        return full;
    }
    format!("{}...", full.chars().take(117).collect::<String>())
}

fn native_risk(metadata: &ToolMetadata, input: &Value, scanner: &Scanner) -> CapabilityRisk {
    if metadata.permission.sensitive(input, scanner).is_some() {
        CapabilityRisk::High
    } else {
        metadata.permission.resolve(input)
    }
}

fn prompt_policy(metadata: &ToolMetadata) -> Value {
    match metadata.permission_prompt {
        crate::tool::PermissionPromptPolicy::Json => json!({"kind": "json"}),
        crate::tool::PermissionPromptPolicy::Command { field } => {
            json!({"kind": "command", "field": field})
        }
        crate::tool::PermissionPromptPolicy::Question { field } => {
            json!({"kind": "question", "field": field})
        }
        crate::tool::PermissionPromptPolicy::Path { field } => {
            json!({"kind": "path", "field": field})
        }
        crate::tool::PermissionPromptPolicy::PatchTarget { patch_field } => {
            json!({"kind": "patch_target", "patch_field": patch_field})
        }
    }
}

fn sensitive_json(metadata: &ToolMetadata, input: &Value, scanner: &Scanner) -> Value {
    let Some(hit) = metadata.permission.sensitive(input, scanner) else {
        return Value::Null;
    };
    json!({
        "tier": match hit.tier {
            Tier::Credential => "credential",
            Tier::Secret => "secret",
        },
        "kind": hit.kind.label(),
        "matched": hit.matched,
        "pattern": hit.pattern,
    })
}

fn directives(effects: Vec<ToolEffect>) -> Vec<ToolDirective> {
    effects
        .into_iter()
        .map(|effect| match effect {
            ToolEffect::CompactHistory { focus } => ToolDirective::RequestCompaction { focus },
            ToolEffect::GoalYield { reason } => ToolDirective::YieldToHost {
                reason: match reason {
                    crate::tool::GoalYieldReason::NeedsUser => {
                        tact_agent_api::HostYieldReason::NeedsUser
                    }
                },
            },
        })
        .collect()
}

fn named_server(input: &Value) -> Option<&str> {
    input.get("server").and_then(Value::as_str)
}

fn resources_to_access(resources: ToolResources) -> ResourceAccess {
    ResourceAccess {
        reads: resources
            .reads
            .iter()
            .map(|path| ResourceScope::descendants(resource_key(path)))
            .collect(),
        writes: resources
            .writes
            .iter()
            .map(|path| ResourceScope::exact(resource_key(path)))
            .collect(),
        barrier: resources.barrier,
    }
}

fn resource_key(path: &Path) -> ResourceKey {
    let text = path.to_string_lossy();
    if let Some(server) = text.strip_prefix("__mcp__") {
        return ResourceKey::new("mcp_server", vec![server.to_string()]);
    }
    if let Some(scope) = text
        .strip_prefix("__tact_")
        .and_then(|value| value.strip_suffix("__"))
    {
        return ResourceKey::new("shared_state", vec![scope.to_string()]);
    }
    ResourceKey::new(
        "path",
        path.components()
            .filter_map(|component| match component {
                Component::RootDir => None,
                Component::Normal(value) => Some(value.to_string_lossy().into_owned()),
                Component::Prefix(prefix) => {
                    Some(prefix.as_os_str().to_string_lossy().into_owned())
                }
                Component::CurDir | Component::ParentDir => {
                    Some(component.as_os_str().to_string_lossy().into_owned())
                }
            })
            .collect(),
    )
}

fn same_specs(left: &[ToolSpec], right: &[ToolSpec]) -> bool {
    left.len() == right.len()
        && left.iter().zip(right).all(|(left, right)| {
            left.name == right.name
                && left.description == right.description
                && left.input_schema == right.input_schema
        })
}

fn prompt_arguments(input: &Value) -> Result<Option<serde_json::Map<String, Value>>, String> {
    let Some(map) = input.get("arguments").and_then(Value::as_object) else {
        return Ok(None);
    };
    let mut arguments = serde_json::Map::new();
    for (key, value) in map {
        let text = match value {
            Value::String(text) => text.clone(),
            Value::Number(_) | Value::Bool(_) => value.to_string(),
            _ => {
                return Err(format!("argument `{key}` must be a string (got {value})"));
            }
        };
        arguments.insert(key.clone(), Value::String(text));
    }
    Ok((!arguments.is_empty()).then_some(arguments))
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use tact_agent_api::{
        AgentContentBlock, CancelToken, ToolExecutionContext, ToolOutputChunk, ToolProgressSink,
        ToolResolver,
    };
    use tact_protocol::ToolOutputChunk as HostToolOutputChunk;

    use super::*;
    use crate::{
        mcp::MCPToolRouter,
        permission::{PermissionManager, PermissionMode},
        tool::test_support::test_context,
        tool::{Tool, ToolCallResult, ToolMetadata},
    };

    struct NoopProgress;

    impl ToolProgressSink for NoopProgress {
        fn emit(&self, _chunks: Vec<ToolOutputChunk>) {}
    }

    const SPLIT_SECRET_METADATA: ToolMetadata = ToolMetadata::read_json(
        "split_secret",
        "Emit a deliberately split provider token.",
        "Split Secret",
    );

    struct SplitSecretTool;

    #[async_trait]
    impl Tool for SplitSecretTool {
        fn metadata(&self) -> &'static ToolMetadata {
            &SPLIT_SECRET_METADATA
        }

        fn input_schema(&self) -> Value {
            json!({"type": "object", "properties": {}})
        }

        async fn call(
            &self,
            context: crate::tool::ToolContext,
            _input: Value,
        ) -> anyhow::Result<ToolCallResult> {
            const SECRET: &str = "sk-7325c3231cef402d8481c32a49c4898a";
            context
                .progress_reporter
                .report(vec![HostToolOutputChunk::stdout("prefix\nsk-7325c32")]);
            context
                .progress_reporter
                .report(vec![HostToolOutputChunk::stdout(
                    "31cef402d8481c32a49c4898a\n",
                )]);
            context.progress_reporter.flush();
            Ok(ToolCallResult::text(SECRET))
        }
    }

    #[derive(Default)]
    struct RecordingProgress(Mutex<Vec<ToolOutputChunk>>);

    impl ToolProgressSink for RecordingProgress {
        fn emit(&self, chunks: Vec<ToolOutputChunk>) {
            self.0.lock().unwrap().extend(chunks);
        }
    }

    impl RecordingProgress {
        fn text(&self) -> String {
            self.0
                .lock()
                .unwrap()
                .iter()
                .map(|chunk| chunk.text.as_str())
                .collect()
        }
    }

    #[tokio::test]
    async fn resolving_a_write_does_not_execute_it() {
        let context = test_context("kernel_tool_resolution_is_read_only");
        let target = context.work_dir.join("resolution-only.txt");
        let permissions = PermissionManager::try_new(PermissionMode::Default).unwrap();
        let port = TactToolPort::new(
            crate::tool::toolset(),
            McpCatalogHandle::new(MCPToolRouter::new()),
            context,
            permissions.security_config(),
            permissions.scanner(),
            false,
        );
        let call = ToolCall {
            id: "write-1".into(),
            name: "write_file".into(),
            input: json!({
                "path": "resolution-only.txt",
                "content": "execution requires a separate ToolPort call",
            }),
        };

        let resolved = port.resolve(&call).expect("resolve write_file");
        assert!(
            !target.exists(),
            "ToolResolver::resolve must not execute the tool"
        );

        port.execute(
            resolved,
            ToolExecutionContext {
                call_id: call.id,
                progress: Arc::new(NoopProgress),
                cancel: CancelToken::new(),
                remaining_turns: None,
            },
        )
        .await
        .expect("execute write_file");
        assert_eq!(
            std::fs::read_to_string(target).unwrap(),
            "execution requires a separate ToolPort call"
        );
    }

    #[test]
    fn port_classifies_native_mcp_and_subagent_resource_scopes() {
        let context = test_context("kernel_tool_resource_scopes");
        let permissions = PermissionManager::try_new(PermissionMode::Default).unwrap();
        let port = TactToolPort::new(
            crate::tool::toolset(),
            McpCatalogHandle::new(MCPToolRouter::new()),
            context,
            permissions.security_config(),
            permissions.scanner(),
            false,
        );
        let call = |name: &str, input: Value| ToolCall {
            id: format!("{name}-1"),
            name: name.into(),
            input,
        };

        let read = call("read_file", json!({"path": "notes.md"}));
        let read_access = port.access(
            &port.resolve_kind(&read).expect("resolve read_file"),
            &read.input,
        );
        assert_eq!(read_access.reads.len(), 1);
        assert_eq!(read_access.reads[0].key.namespace, "path");
        assert!(read_access.reads[0].covers_descendants);
        assert!(!read_access.barrier);

        let write = call("write_file", json!({"path": "notes.md", "content": "x"}));
        let write_access = port.access(
            &port.resolve_kind(&write).expect("resolve write_file"),
            &write.input,
        );
        assert_eq!(write_access.writes.len(), 1);
        assert_eq!(write_access.writes[0].key.namespace, "path");
        assert!(!write_access.writes[0].covers_descendants);

        let bash = call("bash", json!({"command": "pwd"}));
        let bash_access = port.access(
            &port.resolve_kind(&bash).expect("resolve bash"),
            &bash.input,
        );
        assert!(bash_access.barrier);

        let mcp = ResolvedKind::Mcp {
            full_name: "mcp__postgres__query".into(),
            server: "postgres".into(),
            tool: "query".into(),
        };
        let mcp_access = port.access(&mcp, &json!({}));
        assert_eq!(mcp_access.writes.len(), 1);
        assert_eq!(mcp_access.writes[0].key.namespace, "mcp_server");
        assert_eq!(mcp_access.writes[0].key.components, ["postgres"]);

        let blocking_subagent = call("spawn_subagent", json!({"prompt": "work"}));
        let blocking_access = port.access(
            &port
                .resolve_kind(&blocking_subagent)
                .expect("resolve spawn_subagent"),
            &blocking_subagent.input,
        );
        assert!(blocking_access.barrier);

        let worktree_subagent = call(
            "spawn_subagent",
            json!({"prompt": "work", "worktree": true}),
        );
        let worktree_access = port.access(
            &port
                .resolve_kind(&worktree_subagent)
                .expect("resolve worktree subagent"),
            &worktree_subagent.input,
        );
        assert_eq!(worktree_access, ResourceAccess::independent());
    }

    #[tokio::test]
    async fn tool_port_redacts_split_live_chunks_and_the_final_result() {
        const SECRET: &str = "sk-7325c3231cef402d8481c32a49c4898a";

        let context = test_context("kernel_tool_split_secret_redaction");
        let permissions =
            PermissionManager::try_new(PermissionMode::Auto).expect("permission manager");
        let port = TactToolPort::new(
            crate::tool::ToolRouter::new()
                .route(SplitSecretTool)
                .expect("register split-secret tool"),
            McpCatalogHandle::new(MCPToolRouter::new()),
            context,
            permissions.security_config(),
            permissions.scanner(),
            false,
        );
        let call = ToolCall {
            id: "split-secret-1".into(),
            name: SPLIT_SECRET_METADATA.name.into(),
            input: json!({}),
        };
        let resolved = port.resolve(&call).expect("resolve split-secret tool");
        let progress = Arc::new(RecordingProgress::default());
        let result = port
            .execute(
                resolved,
                ToolExecutionContext {
                    call_id: call.id,
                    progress: progress.clone(),
                    cancel: CancelToken::new(),
                    remaining_turns: None,
                },
            )
            .await
            .expect("execute split-secret tool");

        let live = progress.text();
        assert!(live.contains("[redacted:api-key]"), "{live}");
        assert!(
            !live.contains(SECRET),
            "live output leaked the token: {live}"
        );

        let final_text = result
            .outcome
            .content
            .iter()
            .filter_map(|block| match block {
                AgentContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<String>();
        assert!(final_text.contains("[redacted:api-key]"), "{final_text}");
        assert!(
            !final_text.contains(SECRET),
            "final output leaked the token: {final_text}"
        );
    }
}
