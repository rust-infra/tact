//! Product-owned system-prompt assembly for kernel activations.
//!
//! This adapter owns prompt-only state (skills, memory, instruction files, the
//! directory snapshot, and MCP instructions). It has no transcript, cursor,
//! persistence, usage, or event authority.

use std::{
    collections::HashSet,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use anyhow::{Context as _, Result};
use tact_agent_api::ContextView;

use crate::{
    ToolSpec,
    agent::{assemble_agents_md_prompt, cached_md_section, load_dynamic_context},
    compact::{approx_text_tokens, context_pressure_advisory},
    config::AgentSettings,
    mcp::McpCatalogHandle,
    memory::MEMORY_GUIDANCE,
    memory::MemoryManager,
    prompt::{SystemPrompt, responses_prompt_template},
    skill::{SharedSkillRegistry, catalog_budget_chars, lock_skills},
};

/// Prompt sections cached for the lifetime of one logical agent.
#[derive(Debug, Default, Clone)]
pub(crate) struct PromptCaches {
    pub(crate) directory_snapshot: Option<String>,
    pub(crate) agents_md: Option<String>,
}

struct PromptState {
    caches: PromptCaches,
}

/// Product prompt assembler injected into `TactContextPolicy`.
pub(crate) struct SystemPromptAssembler {
    static_prompt: Option<String>,
    responses_prompt: bool,
    workdir: PathBuf,
    settings: AgentSettings,
    skill_registry: SharedSkillRegistry,
    memory_manager: Arc<Mutex<MemoryManager>>,
    mcp: McpCatalogHandle,
    tool_names: HashSet<String>,
    state: Mutex<PromptState>,
}

impl SystemPromptAssembler {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        static_prompt: Option<String>,
        responses_prompt: bool,
        workdir: PathBuf,
        settings: AgentSettings,
        skill_registry: SharedSkillRegistry,
        memory_manager: Arc<Mutex<MemoryManager>>,
        mcp: McpCatalogHandle,
        tools: &[ToolSpec],
        caches: PromptCaches,
    ) -> Self {
        Self {
            static_prompt,
            responses_prompt,
            workdir,
            settings,
            skill_registry,
            memory_manager,
            mcp,
            tool_names: tools.iter().map(|tool| tool.name.clone()).collect(),
            state: Mutex::new(PromptState { caches }),
        }
    }

    #[cfg(test)]
    pub(crate) fn static_text(prompt: impl Into<String>) -> Self {
        Self {
            static_prompt: Some(prompt.into()),
            responses_prompt: false,
            workdir: PathBuf::new(),
            settings: crate::config::test_support::resolved_config().agent,
            skill_registry: Arc::new(Mutex::new(crate::skill::SkillRegistry::new(
                Vec::<PathBuf>::new(),
            ))),
            memory_manager: Arc::new(Mutex::new(MemoryManager::new(PathBuf::new()))),
            mcp: McpCatalogHandle::new(crate::mcp::MCPToolRouter::new()),
            tool_names: HashSet::new(),
            state: Mutex::new(PromptState {
                caches: PromptCaches::default(),
            }),
        }
    }

    pub(crate) fn build(&self, state: &ContextView<'_>) -> Result<String> {
        if let Some(prompt) = &self.static_prompt {
            return Ok(prompt.clone());
        }

        let mut prompt_builder = SystemPrompt::builder();
        if self.responses_prompt {
            prompt_builder.template(responses_prompt_template());
        }
        let memory = if self.settings.memory_enabled {
            self.load_memory_prompt()?
        } else {
            String::new()
        };
        let memory_guidance = if self.settings.memory_enabled {
            MEMORY_GUIDANCE.trim()
        } else {
            ""
        };
        let advisory = self.context_advisory(state);
        let mut prompt_state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let agents_md = cached_md_section(&mut prompt_state.caches.agents_md, || {
            assemble_agents_md_prompt(&self.workdir, &self.settings.instruction_sources)
        });
        let dynamic_context = load_dynamic_context(
            &self.workdir,
            &mut prompt_state.caches.directory_snapshot,
            self.settings.snapshot_max_items,
            &self.settings.model,
            advisory,
        );

        let prompt = prompt_builder
            .role(format!(
                "You are a coding agent operating in {}.",
                self.workdir.display()
            ))
            .guidelines([
                "Try to understand how to complete the task well before completing it.",
            ])
            .constraints([
                "Think step by step",
                "Think before you act; respond with your thoughts before calling tools",
                "Do not make up any assumptions, use tools to get the information you need",
                "Use the provided tools to interact with the system and accomplish the task",
                "If you are stuck, or otherwise cannot complete the task, respond with your thoughts and stop",
                "If the task is completed, or otherwise cannot continue, like requiring user feedback, stop.",
                "Always end your response with a visible text conclusion; never exit after thinking alone without a text block — even a single-sentence summary of your reasoning result is enough.",
                "When editing files, always re-read the file first if its content may have changed since you last read it",
                "If a tool result was truncated and you need the details, re-run the relevant tool (e.g., read_file)",
                "For small edits to existing files, prefer edit_file over write_file; use write_file only for new files or complete rewrites",
            ])
            .skills_available({
                let registry = lock_skills(&self.skill_registry);
                if self.settings.skill_body_auto_inject {
                    registry.describe_available_with_body()
                } else {
                    registry.describe_available_bounded(
                        catalog_budget_chars(self.settings.model_context_window),
                        self.settings.skill_description_cap,
                    )
                }
            })
            .memory(memory)
            .additional(agents_md)
            .mcp_instructions(self.mcp.instructions_block())
            .dynamic_context(dynamic_context)
            .memory_guidance(memory_guidance)
            .build()?;

        prompt
            .to_prompt()
            .render()
            .context("failed to render system prompt")
    }

    pub(crate) fn caches(&self) -> PromptCaches {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .caches
            .clone()
    }

    fn context_advisory(&self, state: &ContextView<'_>) -> Option<String> {
        if !self.tool_names.contains("spawn_subagent") {
            return None;
        }
        let context_tokens = state
            .last_token_total
            .map(usize::try_from)
            .transpose()
            .ok()
            .flatten()
            .filter(|tokens| *tokens > 0)
            .unwrap_or_else(|| estimate_agent_context_tokens(state.transcript));
        context_pressure_advisory(
            context_tokens,
            state.capabilities.known_context_window().unwrap_or(0),
        )
        .then(crate::compact::subagent_advisory)
    }

    fn load_memory_prompt(&self) -> Result<String> {
        self.memory_manager
            .lock()
            .map_err(|_| anyhow::anyhow!("memory manager lock poisoned"))
            .map(|manager| manager.load_memory_index_prompt())
    }
}

pub(crate) fn estimate_agent_context_tokens(messages: &[tact_agent_api::AgentMessage]) -> usize {
    match serde_json::to_string(messages) {
        Ok(serialized) => approx_text_tokens(&serialized),
        Err(error) => {
            tracing::warn!(
                %error,
                message_count = messages.len(),
                "failed to serialize logical context for token estimation"
            );
            usize::MAX / 2
        }
    }
}
