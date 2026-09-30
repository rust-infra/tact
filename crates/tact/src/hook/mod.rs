//! Agent lifecycle hooks.
//!
//! Hooks allow injecting custom behaviour at several points in the agent loop:
//! - [`SessionStart`](Hook::SessionStart) — before the first LLM call.
//! - [`PreToolUse`](Hook::PreToolUse) — before executing a tool, can mutate
//!   the [`ToolUse`] input.
//! - [`PostToolUse`](Hook::PostToolUse) — after tool execution, can mutate
//!   the [`ToolResult`].
//! - [`PostToolUseFailure`](Hook::PostToolUseFailure) — after a tool *fails*
//!   (observational; the tool already errored).
//! - [`Notification`](Hook::Notification) — when the agent surfaces a user
//!   notification (only `permission_prompt` today); observational.
//! - [`TaskCompleted`](Hook::TaskCompleted) — once per completed user task
//!   (observational).
//! - [`Stop`](Hook::Stop) — once at the outer turn boundary; a `Block` continues
//!   the turn (Codex continuation-fragment semantics).
//! - [`SessionEnd`](Hook::SessionEnd) — once at teardown (observational).
//! - [`PreCompact`](Hook::PreCompact) / [`PostCompact`](Hook::PostCompact) —
//!   around compaction; PreCompact can veto it.
//!
//! Each hook returns [`HookControl::Continue`] or [`HookControl::Block`]
//! to permit or veto the next operation.  The [`invoke_hooks!`] macro
//! iterates over registered hooks of a given type and short-circuits on
//! the first `Block`.

pub mod rtk_filter;

use std::pin::Pin;

use anyhow::Result;

use crate::{LoopState, compact::CompactTrigger};

/// Framing around a hook-injected context message.
///
/// The context reaches the model as a user message, so it carries markers that
/// name its author — the convention `<subagent-finished>` and
/// `<context-handoff>` already use here. Codex gets the same effect from its
/// `developer` role; Tact's message model has no such role, and the markers
/// also survive a reload, where the in-memory `MessageKind` does not.
pub const HOOK_CONTEXT_OPEN_TAG: &str = "<hook-context>";
pub const HOOK_CONTEXT_CLOSE_TAG: &str = "</hook-context>";

/// Why the `SessionStart` hooks are running.
///
/// Codex's `source` matcher vocabulary: a plugin writes `startup|resume|compact`
/// and decides per case. Tact reports the real one — a session that restored
/// history is a resume, and a compaction that just summarized history away
/// re-runs the hooks as `compact`, which is how a plugin re-orients.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionStartSource {
    /// A fresh session.
    Startup,
    /// History was restored from disk (`--resume-last`, `--session`, `/resume`).
    Resume,
    /// A compaction just replaced the history these hooks orient against.
    Compact,
}

impl SessionStartSource {
    /// The `source` matcher value, as Codex spells it.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Startup => "startup",
            Self::Resume => "resume",
            Self::Compact => "compact",
        }
    }
}

/// True when `text` is a hook-injected context cell (see
/// [`HOOK_CONTEXT_OPEN_TAG`]).
pub fn is_hook_context_text(text: &str) -> bool {
    text.trim_start().starts_with(HOOK_CONTEXT_OPEN_TAG)
}

/// Returns the context carried by a `<hook-context>` cell, or `text`
/// unchanged when it is not one — the inverse of the framing
/// `Agent::inject_pending_session_context` applies.
pub fn hook_context_body(text: &str) -> &str {
    let trimmed = text.trim();
    trimmed
        .strip_prefix(HOOK_CONTEXT_OPEN_TAG)
        .and_then(|rest| rest.strip_suffix(HOOK_CONTEXT_CLOSE_TAG))
        .map(str::trim)
        .unwrap_or(text)
}

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

/// Mutable context handed to subagent-stop hooks, after the child's
/// `agent_loop` has returned and its summary has been computed. The hook may
/// rewrite `summary` (fed back to the parent); it runs post-completion, so a
/// `Block` from a Rust hook is log-only.
#[derive(Debug, Clone)]
pub struct SubagentStopContext {
    /// The child session id (also used as the plugin `agent_id` / `agent_type`).
    pub agent_id: String,
    /// The subagent identifier used for matcher scoping (== `agent_id` today).
    pub agent_type: String,
    /// The task prompt the child was spawned with.
    pub prompt: String,
    /// Whether the child's agent loop returned `Ok`.
    pub success: bool,
    /// Whether the child was cooperatively cancelled.
    pub cancelled: bool,
    /// The child's final summary; mutable so hooks can rewrite it.
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum HookControl {
    #[default]
    Continue,
    Block(String),
}

/// Mutable context handed to session-start hooks.
///
/// A `SessionStart` hook may inject context into the conversation (Codex
/// `additionalContext`, Claude Code `hookSpecificOutput.additionalContext`, or
/// a plugin's plain stdout). Each chunk is recorded as its own synthetic user
/// message before the first turn — one message per hook, matching Codex — and
/// is framed with `<hook-context>` markers so the model does not read it as
/// something the user typed.
#[derive(Debug, Clone, Default)]
pub struct SessionStartContext {
    /// Context chunks, one message each, in hook-registration order.
    pub additional_contexts: Vec<String>,
}

impl SessionStartContext {
    /// Records one chunk, ignoring blank output.
    pub fn push_additional_context(&mut self, text: &str) {
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            self.additional_contexts.push(trimmed.to_string());
        }
    }
}

pub trait SessionStartFn:
    for<'a> Fn(
        &'a LoopState,
        &'a mut SessionStartContext,
    ) -> Pin<Box<dyn Future<Output = Result<HookControl>> + Send + 'a>>
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
    for<'a> Fn(
        &'a LoopState,
        &'a mut String,
    ) -> Pin<Box<dyn Future<Output = Result<HookControl>> + Send + 'a>>
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
    for<'a> Fn(
        &'a mut SubagentStartContext,
    ) -> Pin<Box<dyn Future<Output = Result<HookControl>> + Send + 'a>>
    + Send
    + Sync
{
}

/// Subagent-stop hook: runs after a subagent's `agent_loop` has returned and
/// its summary is computed (Claude Code `SubagentStop`). Like
/// [`SubagentStartFn`] it lives on `ToolContext` (no parent `Agent` handle),
/// and receives a mutable [`SubagentStopContext`] so it can rewrite the summary.
pub trait SubagentStopFn:
    for<'a> Fn(
        &'a mut SubagentStopContext,
    ) -> Pin<Box<dyn Future<Output = Result<HookControl>> + Send + 'a>>
    + Send
    + Sync
{
}

/// Stop hook: runs once at the outer turn boundary when `agent_loop` is about
/// to return (Codex `Stop`). Read-only access to [`LoopState`]; a `Block`
/// means "do not stop — continue the turn with the block reason as the next
/// prompt" (Codex continuation-fragment semantics, inverted from the other
/// hooks where `Block` vetoes).
pub trait StopFn:
    for<'a> Fn(&'a LoopState) -> Pin<Box<dyn Future<Output = Result<HookControl>> + Send + 'a>>
    + Send
    + Sync
{
}

/// Session-end hook: runs once at real teardown, symmetrical with
/// [`SessionStart`](Hook::SessionStart) (Codex `SessionEnd`). Observational;
/// a `Block` is log-only because the session is ending.
pub trait SessionEndFn:
    for<'a> Fn(&'a LoopState) -> Pin<Box<dyn Future<Output = Result<HookControl>> + Send + 'a>>
    + Send
    + Sync
{
}

/// Pre-compact hook: runs before compaction (Codex `PreCompact`). Read-only
/// [`LoopState`] + the compaction [`CompactTrigger`]; a `Block` vetoes the
/// compaction.
pub trait PreCompactFn:
    for<'a> Fn(
        &'a LoopState,
        CompactTrigger,
    ) -> Pin<Box<dyn Future<Output = Result<HookControl>> + Send + 'a>>
    + Send
    + Sync
{
}

/// Post-compact hook: runs after a successful compaction (Codex `PostCompact`).
/// Read-only [`LoopState`] + [`CompactTrigger`]; a `Block` is log-only.
pub trait PostCompactFn:
    for<'a> Fn(
        &'a LoopState,
        CompactTrigger,
    ) -> Pin<Box<dyn Future<Output = Result<HookControl>> + Send + 'a>>
    + Send
    + Sync
{
}

/// Post-tool-failure hook: runs after a tool fails (Claude Code
/// `PostToolUseFailure`). Read-only [`LoopState`] + [`ToolUse`] + the error
/// message; a `Block` is log-only because the tool already failed.
pub trait PostToolUseFailureFn:
    for<'a> Fn(
        &'a LoopState,
        &'a ToolUse,
        &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<HookControl>> + Send + 'a>>
    + Send
    + Sync
{
}

/// Mutable-free context handed to notification hooks (Claude Code
/// `Notification`). Only `permission_prompt` is surfaced today.
#[derive(Debug, Clone)]
pub struct NotificationContext {
    /// The notification kind; `permission_prompt` is the only value Tact emits.
    pub notification_type: String,
    /// Short title (the tool name for permission prompts).
    pub title: String,
    /// The user-facing message (the formatted permission prompt).
    pub message: String,
}

/// Notification hook: runs when the agent surfaces a user notification
/// (Claude Code `Notification`). Read-only [`LoopState`] +
/// [`NotificationContext`]; a `Block` is log-only (observational).
pub trait NotificationFn:
    for<'a> Fn(
        &'a LoopState,
        &'a NotificationContext,
    ) -> Pin<Box<dyn Future<Output = Result<HookControl>> + Send + 'a>>
    + Send
    + Sync
{
}

/// Task-completed hook: runs once per completed user task (Claude Code
/// `TaskCompleted`). Read-only [`LoopState`]; a `Block` is log-only.
pub trait TaskCompletedFn:
    for<'a> Fn(&'a LoopState) -> Pin<Box<dyn Future<Output = Result<HookControl>> + Send + 'a>>
    + Send
    + Sync
{
}

impl<F> SessionStartFn for F where
    F: for<'a> Fn(
            &'a LoopState,
            &'a mut SessionStartContext,
        ) -> Pin<Box<dyn Future<Output = Result<HookControl>> + Send + 'a>>
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
    F: for<'a> Fn(
            &'a LoopState,
            &'a mut String,
        ) -> Pin<Box<dyn Future<Output = Result<HookControl>> + Send + 'a>>
        + Send
        + Sync
{
}

impl<F> SubagentStartFn for F where
    F: for<'a> Fn(
            &'a mut SubagentStartContext,
        ) -> Pin<Box<dyn Future<Output = Result<HookControl>> + Send + 'a>>
        + Send
        + Sync
{
}

impl<F> SubagentStopFn for F where
    F: for<'a> Fn(
            &'a mut SubagentStopContext,
        ) -> Pin<Box<dyn Future<Output = Result<HookControl>> + Send + 'a>>
        + Send
        + Sync
{
}

impl<F> StopFn for F where
    F: for<'a> Fn(&'a LoopState) -> Pin<Box<dyn Future<Output = Result<HookControl>> + Send + 'a>>
        + Send
        + Sync
{
}

impl<F> SessionEndFn for F where
    F: for<'a> Fn(&'a LoopState) -> Pin<Box<dyn Future<Output = Result<HookControl>> + Send + 'a>>
        + Send
        + Sync
{
}

impl<F> PreCompactFn for F where
    F: for<'a> Fn(
            &'a LoopState,
            CompactTrigger,
        ) -> Pin<Box<dyn Future<Output = Result<HookControl>> + Send + 'a>>
        + Send
        + Sync
{
}

impl<F> PostCompactFn for F where
    F: for<'a> Fn(
            &'a LoopState,
            CompactTrigger,
        ) -> Pin<Box<dyn Future<Output = Result<HookControl>> + Send + 'a>>
        + Send
        + Sync
{
}

impl<F> PostToolUseFailureFn for F where
    F: for<'a> Fn(
            &'a LoopState,
            &'a ToolUse,
            &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<HookControl>> + Send + 'a>>
        + Send
        + Sync
{
}

impl<F> NotificationFn for F where
    F: for<'a> Fn(
            &'a LoopState,
            &'a NotificationContext,
        ) -> Pin<Box<dyn Future<Output = Result<HookControl>> + Send + 'a>>
        + Send
        + Sync
{
}

impl<F> TaskCompletedFn for F where
    F: for<'a> Fn(&'a LoopState) -> Pin<Box<dyn Future<Output = Result<HookControl>> + Send + 'a>>
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
    Stop(Box<dyn StopFn>),
    SessionEnd(Box<dyn SessionEndFn>),
    PreCompact(Box<dyn PreCompactFn>),
    PostCompact(Box<dyn PostCompactFn>),
    PostToolUseFailure(Box<dyn PostToolUseFailureFn>),
    Notification(Box<dyn NotificationFn>),
    TaskCompleted(Box<dyn TaskCompletedFn>),
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
