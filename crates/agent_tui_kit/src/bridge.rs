//! The in/out contract between the kit and the host project.

/// A user/agent command the kit sends to the host.
///
/// Draft: aliases [`tact_protocol::UserCommand`] wholesale.
///
/// TODO(T4.4): split — the generic variants stay protocol-level
/// (`SubmitTask`, `Cancel`, `Compact`, `QueryStats`, `QueryBackground`,
/// `SetModel`, `SetThinkingBudget`, `SetReasoningEffort`, `SetPermissionMode`);
/// Tact-only variants (`QueryBalance`, …) move to the extension channel.
pub type Command = tact_protocol::UserCommand;

/// What the host project implements to drive the kit.
pub trait AgentBridge {
    /// Send a user/agent command out (maps to Tact's `user_cmd_tx`).
    fn send_command(&mut self, cmd: Command);

    /// Optional host extension surface (balance, quota, plugins, voice, …).
    fn extension(&mut self) -> Option<&mut dyn BridgeExtension> {
        None
    }
}

/// Host-specific capabilities the shell renders — NOT part of the kit core.
///
/// Tact implements this for plugins / voice / skills / account balance.
///
/// TODO(T2.3): finalize event variants during the protocol audit (Phase 0
/// T0.3); currently uninhabited.
pub trait BridgeExtension {
    /// Handle a host-specific event produced by the app layer.
    fn on_event(&mut self, event: ExtensionEvent);

    /// Extra keybindings the extension contributes.
    fn keybindings(&self) -> &'static [(&'static str, &'static str)] {
        &[]
    }
}

/// Host-specific events handed from the app layer to the extension.
///
/// TODO(T2.3): variants for plugin events / account (balance, quota) /
/// voice / skills, filled from the T0.3 audit table.
#[derive(Debug, Clone)]
pub enum ExtensionEvent {}
