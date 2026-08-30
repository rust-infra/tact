//! Agent lifecycle hooks.
//!
//! Hooks allow injecting custom behaviour at three points in the agent loop:
//! - [`SessionStart`](Hook::SessionStart) — before the first LLM call.
//! - [`PreToolUse`](Hook::PreToolUse) — before executing a tool, can mutate
//!   the [`ToolUse`] input.
//! - [`PostToolUse`](Hook::PostToolUse) — after tool execution, can mutate
//!   the [`ToolResult`].
//!
//! Each hook returns [`HookControl::Continue`] or [`HookControl::Block`]
//! to permit or veto the next operation.  The [`invoke_hooks!`] macro
//! iterates over registered hooks of a given type and short-circuits on
//! the first `Block`.

pub mod rtk_filter;

use std::pin::Pin;

use anyhow::Result;

use crate::LoopState;

#[derive(Debug)]
pub struct ToolUse {
    pub id: String,
    pub name: String,
    pub input: serde_json::Value,
}

#[derive(Debug)]
pub struct ToolResult {
    pub tool_use_id: String,
    pub content: String,
}

/// Mutable context handed to subagent-start hooks. Hooks may append context to
/// `system_prompt` (e.g. Claude Code plugins that inject a mode into every
/// subagent) and inspect the task prompt.
#[derive(Debug, Clone)]
pub struct SubagentStartContext {
    pub name: String,
    pub prompt: String,
    pub system_prompt: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookControl {
    Continue,
    Block(String),
}

impl Default for HookControl {
    fn default() -> Self {
        Self::Continue
    }
}

pub trait SessionStartFn:
    for<'a> Fn(&'a LoopState) -> Pin<Box<dyn Future<Output = Result<HookControl>> + Send + 'a>>
    + Send
    + Sync
{
}

pub trait PreToolUseFn:
    for<'a> Fn(
        &'a LoopState,
        &'a mut ToolUse,
    ) -> Pin<Box<dyn Future<Output = Result<HookControl>> + Send + 'a>>
    + Send
    + Sync
{
}

pub trait PostToolUseFn:
    for<'tool> Fn(
        &'tool LoopState,
        &'tool ToolUse,
        &'tool mut ToolResult,
        tact_protocol::StepStatus,
    ) -> Pin<Box<dyn Future<Output = Result<HookControl>> + Send + 'tool>>
    + Send
    + Sync
{
}

/// User-prompt lifecycle hook: runs when a user turn message enters the agent
/// loop and may append context to the prompt text (Claude Code
/// `UserPromptSubmit.additionalContext`).
pub trait UserPromptSubmitFn:
    for<'a> Fn(&'a LoopState, &'a mut String) -> Pin<Box<dyn Future<Output = Result<HookControl>> + Send + 'a>>
    + Send
    + Sync
{
}

/// Subagent-start hook: runs when a subagent is about to start and may mutate
/// the subagent's system prompt (Claude Code `SubagentStart`).
///
/// Unlike the other hooks this one does **not** receive [`LoopState`] because
/// `spawn_subagent` is a tool handler with only a [`ToolContext`]; plugin
/// command hooks are stored on the context as `Arc<dyn SubagentStartFn>` and
/// invoked directly by the spawn path.
pub trait SubagentStartFn:
    for<'a> Fn(&'a mut SubagentStartContext) -> Pin<Box<dyn Future<Output = Result<HookControl>> + Send + 'a>>
    + Send
    + Sync
{
}

impl<F> SessionStartFn for F where
    F: for<'a> Fn(&'a LoopState) -> Pin<Box<dyn Future<Output = Result<HookControl>> + Send + 'a>>
        + Send
        + Sync
{
}

impl<F> PreToolUseFn for F where
    F: for<'tool> Fn(
            &'tool LoopState,
            &'tool mut ToolUse,
        ) -> Pin<Box<dyn Future<Output = Result<HookControl>> + Send + 'tool>>
        + Send
        + Sync
{
}

impl<F> PostToolUseFn for F where
    F: for<'tool> Fn(
            &'tool LoopState,
            &'tool ToolUse,
            &'tool mut ToolResult,
            tact_protocol::StepStatus,
        ) -> Pin<Box<dyn Future<Output = Result<HookControl>> + Send + 'tool>>
        + Send
        + Sync
{
}

impl<F> UserPromptSubmitFn for F where
    F: for<'a> Fn(&'a LoopState, &'a mut String) -> Pin<Box<dyn Future<Output = Result<HookControl>> + Send + 'a>>
        + Send
        + Sync
{
}

impl<F> SubagentStartFn for F where
    F: for<'a> Fn(&'a mut SubagentStartContext) -> Pin<Box<dyn Future<Output = Result<HookControl>> + Send + 'a>>
        + Send
        + Sync
{
}

#[derive(strum_macros::EnumDiscriminants, strum_macros::Display)]
#[strum_discriminants(name(HookTypes), derive(strum_macros::Display))]
pub enum Hook {
    SessionStart(Box<dyn SessionStartFn>),
    UserPromptSubmit(Box<dyn UserPromptSubmitFn>),
    PreToolUse(Box<dyn PreToolUseFn>),
    PostToolUse(Box<dyn PostToolUseFn>),
}

#[macro_export]
macro_rules! invoke_hooks {
    ($hook_type:ident, $self_expr:expr $(, $arg:expr)* ) => {{
        let mut control = $crate::hook::HookControl::Continue;

        for hook in $self_expr.hooks_by_type($crate::hook::HookTypes::$hook_type) {
            if let $crate::hook::Hook::$hook_type(hook_fn) = hook {
                match hook_fn($self_expr $(, $arg)*).await? {
                    $crate::hook::HookControl::Continue => {}
                    $crate::hook::HookControl::Block(reason) => {
                        control = $crate::hook::HookControl::Block(reason);
                        break;
                    }
                }
            }
        }

        anyhow::Ok(control)
    }};
}
