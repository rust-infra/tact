//! Permission system for tool invocation.
//!
//! Every tool call is classified by [`CapabilityRisk`] (Read / Write / High)
//! from its typed metadata. The [`PermissionManager`] decides whether to allow,
//! deny, or ask the user, depending on:
//!
//! - The active [`PermissionMode`] (Default, Plan, Auto).
//! - The risk level of the tool.
//! - A per-user allow-list (`always_allowed_tools`).
//! - Consecutive denials (which may trigger a suggestion to switch to Plan mode).
//! - Loaded JSON permission settings (global and project) — see [`settings`].

mod kernel_service;
pub mod settings;

pub use kernel_service::{PermissionManagerService, PermissionResponder};

use anyhow::Result;
use serde_json::Value;
use strum_macros::Display;

use crate::tool::PermissionPromptPolicy;

// The decision types and the decision itself live in the Kernel; this module
// keeps the stateful manager, the settings parser, and the prompt formatting.
pub use tact::permission::{
    CapabilityRisk, PermissionBehavior, PermissionDecision, PermissionMode, RuleAction,
};

/// What an "always allow this tool" gesture managed to record.
///
/// [`AllowOutcome::NotNarrowable`] exists so the UI can tell the user the
/// approval will not stick. Silently doing nothing there was the previous
/// behaviour of a *different* code path (the bare-rule fallback), which is how
/// a `bash` command containing a `:` came to grant every future command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AllowOutcome {
    /// A rule was recorded — persisted to settings, or held in memory for this
    /// session when no settings store exists.
    Recorded,
    /// No rule narrower than the whole tool could be expressed for this call,
    /// so nothing was recorded. The call itself is still approved once; the
    /// next identical call asks again.
    NotNarrowable,
}

impl AllowOutcome {
    #[must_use]
    pub fn is_recorded(self) -> bool {
        matches!(self, Self::Recorded)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Display)]
#[allow(dead_code)]
enum UserPermissionChoice {
    #[strum(serialize = "allow once")]
    AllowOnce,
    #[strum(serialize = "deny")]
    Deny,
    #[strum(serialize = "always allow this tool")]
    AlwaysAllow,
}

#[derive(Debug)]
pub struct PermissionManager {
    mode: PermissionMode,
    always_allowed_tools: Vec<String>,
    consecutive_denials: usize,
    max_consecutive_denials: usize,
    /// Loaded JSON permission settings (global + project), if any.
    settings: Option<settings::PermissionSettings>,
}

/// A `Clone`-able point-in-time view of a [`PermissionManager`]'s inherited
/// state: the active mode, the in-session always-allowed list, and the loaded
/// settings. Stamped onto [`crate::tool::ToolContext`] at dispatch time so a
/// subagent can inherit the parent's permission context (Claude-style) instead
/// of always starting from `PermissionMode::Default`.
///
/// Deliberately excludes the denial counters, which are per-process and reset
/// when a subagent is spawned.
#[derive(Clone)]
pub struct PermissionSnapshot {
    pub mode: PermissionMode,
    /// Rules the user granted *this session* ("Always allow this tool" with no
    /// settings store to persist into).
    ///
    /// Deliberately empty at construction. `read_file` used to be seeded here,
    /// which did nothing while `read_file` was always classified `Read` — but
    /// once a sensitive target can escalate it to `High`, the seed let every
    /// `read_file` input through, `.env` included. An allow-list entry nobody
    /// granted must not be able to outrank the guard.
    pub always_allowed_tools: Vec<String>,
    pub settings: Option<settings::PermissionSettings>,
}

impl PermissionManager {
    /// Create a new manager with no loaded permission-settings store.
    ///
    /// This constructor is suitable for isolated tests and callers that
    /// do not have a project directory (e.g. some test harnesses).  It
    /// does not inherit any persistent allow/ask/deny rules.
    pub fn try_new(mode: PermissionMode) -> Result<Self> {
        Ok(Self {
            mode,
            always_allowed_tools: Vec::new(),
            consecutive_denials: 0,
            max_consecutive_denials: 3,
            settings: None,
        })
    }

    /// Create a new manager with loaded permission settings.
    ///
    /// The `settings` handle provides merged global + project rules and
    /// the ability to persist new allow rules via
    /// [`allow_tool_with_input`].
    pub fn try_new_with_settings(
        mode: PermissionMode,
        settings: settings::PermissionSettings,
    ) -> Result<Self> {
        Ok(Self {
            mode,
            always_allowed_tools: Vec::new(),
            consecutive_denials: 0,
            max_consecutive_denials: 3,
            settings: Some(settings),
        })
    }

    pub fn mode(&self) -> PermissionMode {
        self.mode
    }

    /// The sensitive-path guard this manager's settings describe.
    ///
    /// With no settings store the guard is still **on**, built from the
    /// registry's defaults — an absent project directory must not mean absent
    /// protection. `permissions.sensitive_paths.enabled = false` is the only
    /// way to turn it off, and that has to be written down.
    #[must_use]
    pub fn scanner(&self) -> crate::security::sensitive::Scanner {
        match &self.settings {
            Some(settings) => settings.security_config().scanner(),
            None => crate::security::sensitive::Scanner::builtin(),
        }
    }

    /// The sensitive-path and redaction configuration in effect.
    #[must_use]
    pub fn security_config(&self) -> crate::security::SecurityConfig {
        match &self.settings {
            Some(settings) => settings.security_config().clone(),
            None => crate::security::SecurityConfig::default(),
        }
    }

    pub fn set_mode(&mut self, mode: PermissionMode) {
        self.mode = mode;
    }

    pub fn rules(&self) -> &[String] {
        &self.always_allowed_tools
    }

    /// Capture a `Clone`-able point-in-time snapshot of the inherited state
    /// (mode + allow-list + settings). Used to seed a subagent's own
    /// [`PermissionManager`] so it inherits the parent's permission context.
    pub fn snapshot(&self) -> PermissionSnapshot {
        PermissionSnapshot {
            mode: self.mode,
            always_allowed_tools: self.always_allowed_tools.clone(),
            settings: self.settings.clone(),
        }
    }

    /// Build a fresh manager from a [`PermissionSnapshot`]. The denial
    /// counters are reset (`consecutive_denials = 0`, max stays 3) so a
    /// subagent starts with a clean slate rather than inheriting the parent's
    /// in-flight denial streak.
    pub fn from_snapshot(s: PermissionSnapshot) -> Self {
        Self {
            mode: s.mode,
            always_allowed_tools: s.always_allowed_tools,
            consecutive_denials: 0,
            max_consecutive_denials: 3,
            settings: s.settings,
        }
    }

    /// Check permission for a tool given its stable name, resolved risk,
    /// and the current structured input.
    ///
    /// The input is used to evaluate parameter-aware allow/ask/deny rules
    /// from the loaded settings.  Mode semantics are authoritative and
    /// applied first:
    ///
    /// 1. Read → auto-allow (regardless of mode).
    /// 2. Plan mode → deny writes.
    /// 3. Auto mode → allow everything.
    /// 4. Default mode + matching settings deny → deny.
    /// 5. Default mode + matching settings allow → allow
    ///    (including high-risk — user explicitly trusts the pattern).
    /// 6. Default mode + matching settings ask → ask (non-high only;
    ///    high-risk Ask/None still uses the high-risk ask path).
    /// 7. Default mode + server-policy auto-approve → allow.
    /// 8. Default mode + high risk + in-session always-allowed (exact tool
    ///    *and* input) → allow. A high-risk tool is still asked for the first
    ///    time; this only honours an allow the user granted explicitly.
    /// 9. Default mode + high risk, not allow-listed → ask.
    /// 10. Default mode + in-session always-allowed → allow.
    /// 11. Otherwise → ask.
    pub fn check(
        &mut self,
        tool_name: &str,
        risk: CapabilityRisk,
        input: &Value,
    ) -> PermissionDecision {
        self.check_with_auto(tool_name, risk, input, false)
    }

    /// [`Self::check`] plus the one policy that lives outside the permission
    /// store: an MCP server entry's `approval_mode: "auto"`.
    ///
    /// Deliberately a *separate* switch rather than a lower risk: `Read` is
    /// allowed before plan mode is consulted, so mapping a server policy onto
    /// `Read` would let a write tool run in plan mode. `auto_approved` is
    /// consulted after plan mode and after an explicit `deny` rule, so it can
    /// only ever skip the *ask* step.
    pub fn check_with_auto(
        &mut self,
        tool_name: &str,
        risk: CapabilityRisk,
        input: &Value,
        auto_approved: bool,
    ) -> PermissionDecision {
        let always_allowed = self.is_always_allowed(tool_name, input);
        let rules = self
            .settings
            .as_ref()
            .map(|settings| settings as &dyn tact::PermissionRules);
        let decision = tact::decide(
            self.mode,
            &tact::DecisionInput {
                capability: tool_name,
                risk,
                input,
                rules,
                auto_approved,
                always_allowed,
            },
        );
        if decision.behavior == PermissionBehavior::Allow {
            self.consecutive_denials = 0;
        }
        decision
    }

    pub fn ask_user(&mut self, tool_name: &str, risk: CapabilityRisk) -> Result<bool> {
        // No UI: Default-mode fallback when check() returned Ask.
        // High → deny; Write/Read → allow once (Read should not reach here).
        let choice = match risk {
            CapabilityRisk::High => {
                eprintln!(
                    "[permission] non-interactive: denying high-risk {}",
                    tool_name
                );
                UserPermissionChoice::Deny
            }
            CapabilityRisk::Write | CapabilityRisk::Read => {
                eprintln!("[permission] non-interactive: allowing {}", tool_name);
                UserPermissionChoice::AllowOnce
            }
        };
        let approved = self.apply_user_choice(choice, tool_name);
        if !approved && self.should_suggest_plan_mode() {
            eprintln!(
                "[{} consecutive denials -- consider switching to plan mode]",
                self.consecutive_denials
            );
        }
        Ok(approved)
    }

    fn apply_user_choice(&mut self, choice: UserPermissionChoice, tool_name: &str) -> bool {
        match choice {
            UserPermissionChoice::AllowOnce => {
                self.consecutive_denials = 0;
                true
            }
            UserPermissionChoice::Deny => {
                self.consecutive_denials += 1;
                false
            }
            UserPermissionChoice::AlwaysAllow => {
                self.allow_tool(tool_name);
                self.consecutive_denials = 0;
                true
            }
        }
    }

    pub fn allow_tool(&mut self, tool_name: &str) {
        if !self.is_always_allowed(tool_name, &Value::Null) {
            self.always_allowed_tools.push(tool_name.to_string());
        }
    }

    /// Generate a parameter-aware permission rule from the current tool
    /// call and persist it to project settings.
    ///
    /// The rule is generated via [`PermissionRule::generate`] using the
    /// tool's metadata policy (or `Json` if none is available).
    ///
    /// Unlike [`allow_tool`], this method does **not** add a bare tool name
    /// to the in-memory `always_allowed_tools` list, because doing so would
    /// grant approval for unrelated future inputs with the same tool.
    /// Input-aware approvals are matched by the persisted settings rules
    /// (or, after a successful persist, by the cached effective rules).
    ///
    /// When no settings store is available (e.g. agent started without
    /// a project directory), the generated rule string is added to the
    /// in-memory allow list so that the same input is still approved for
    /// the remainder of the session.  This is strictly narrower than a
    /// bare tool name because matching requires both tool name and input.
    ///
    /// When the call cannot be narrowed to a rule — see
    /// [`PermissionRule::generate`] — nothing is recorded and
    /// [`AllowOutcome::NotNarrowable`] is returned. **The caller must surface
    /// that**: a click on "Always allow this tool" that silently does nothing
    /// is indistinguishable from a bug.
    ///
    /// **Persistence errors are logged as warnings and never convert an
    /// already-approved choice into a denial.**
    pub fn allow_tool_with_input(
        &mut self,
        tool_name: &str,
        policy: PermissionPromptPolicy,
        input: &Value,
    ) -> AllowOutcome {
        // Generate the narrowest parameter-aware rule.
        let Some(rule) = settings::PermissionRule::generate(tool_name, policy, input) else {
            return AllowOutcome::NotNarrowable;
        };
        let rule_string = rule.to_rule_string();

        // When no settings store is available, add the generated rule
        // string to the in-memory allow list.  This preserves same-session
        // approval for the specific input without granting unrelated inputs.
        if self.settings.is_none() {
            if !self.always_allowed_tools.contains(&rule_string) {
                self.always_allowed_tools.push(rule_string);
            }
            return AllowOutcome::Recorded;
        }

        // Persist to project settings — warn on failure, never deny.
        if let Some(settings) = &mut self.settings
            && let Err(e) = settings.persist_project_allow(&rule_string)
        {
            tracing::warn!(
                "Failed to persist permission rule '{}': {}. The operation remains approved.",
                rule_string,
                e
            );
        }
        AllowOutcome::Recorded
    }

    /// The program-level rule the popup's "always allow this program" choice
    /// would record for this call, or `None` when that choice cannot be offered
    /// at all.
    ///
    /// An associated function, not a method: the answer depends on the call and
    /// not on any state the manager holds. Both the option's presence and the
    /// rule the popup previews come from this one decision, so they cannot
    /// disagree — a button that recorded something other than what it showed
    /// would be worse than no button.
    ///
    /// See [`settings::PermissionRule::generate_prefix`] for what "cannot be
    /// offered" covers: a non-command tool, a compound command, a command whose
    /// segments cannot be enumerated, an interpreter or destructive program,
    /// and anything narrower than two words.
    #[must_use]
    pub fn prefix_rule_for(
        tool_name: &str,
        policy: PermissionPromptPolicy,
        input: &Value,
    ) -> Option<settings::PermissionRule> {
        settings::PermissionRule::generate_prefix(tool_name, policy, input)
    }

    /// Record the program-level rule for this call, persisting it.
    ///
    /// The wide end of the popup: `bash(command:^cargo test)` covers every
    /// later `cargo test …` in every session. [`Self::prefix_rule_for`] gates
    /// whether the choice is offered, so a `NotNarrowable` here means the call
    /// changed between the prompt and the answer — record nothing and say so.
    pub fn allow_tool_as_prefix(
        &mut self,
        tool_name: &str,
        policy: PermissionPromptPolicy,
        input: &Value,
    ) -> AllowOutcome {
        let Some(rule) = settings::PermissionRule::generate_prefix(tool_name, policy, input) else {
            return AllowOutcome::NotNarrowable;
        };
        let rule_string = rule.to_rule_string();

        if self.settings.is_none() {
            if !self.always_allowed_tools.contains(&rule_string) {
                self.always_allowed_tools.push(rule_string);
            }
            self.consecutive_denials = 0;
            return AllowOutcome::Recorded;
        }

        if let Some(settings) = &mut self.settings
            && let Err(e) = settings.persist_project_allow(&rule_string)
        {
            tracing::warn!(
                "Failed to persist permission rule '{}': {}. The operation remains approved.",
                rule_string,
                e
            );
        }
        self.consecutive_denials = 0;
        AllowOutcome::Recorded
    }

    /// Record an approval for **this session only**, without persisting it.
    ///
    /// The in-memory twin of [`Self::allow_tool_with_input`]: the same rule
    /// generation, and therefore the same narrowness, but the rule never
    /// reaches the settings file.
    ///
    /// That narrowness is what makes this option safe to offer on a `bash`
    /// command. The generated rule is keyed on the command the user was shown,
    /// so a *different* command — a `sudo`, say — still asks. Recording a bare
    /// tool name here instead would let one click on an ordinary command
    /// approve every future shell command for the rest of the session, which is
    /// exactly the regression that keeps [`Self::allow_tool`] out of the
    /// high-risk path.
    ///
    /// A rule that cannot be narrowed records nothing and returns
    /// [`AllowOutcome::NotNarrowable`]; the caller must surface that rather
    /// than pretend the click worked.
    pub fn allow_tool_for_session(
        &mut self,
        tool_name: &str,
        policy: PermissionPromptPolicy,
        input: &Value,
    ) -> AllowOutcome {
        let Some(rule) = settings::PermissionRule::generate(tool_name, policy, input) else {
            return AllowOutcome::NotNarrowable;
        };
        let rule_string = rule.to_rule_string();
        if !self.always_allowed_tools.contains(&rule_string) {
            self.always_allowed_tools.push(rule_string);
        }
        self.consecutive_denials = 0;
        AllowOutcome::Recorded
    }

    fn is_always_allowed(&self, tool_name: &str, input: &Value) -> bool {
        self.always_allowed_tools.iter().any(|allowed| {
            // Bare tool name match (legacy allow_tool).
            if allowed == tool_name {
                return true;
            }
            // Generated rule match (input-aware allow_tool_with_input
            // without settings store).
            if let Some(rule) = settings::PermissionRule::parse(allowed) {
                return rule.matches(tool_name, input);
            }
            false
        })
    }

    fn should_suggest_plan_mode(&self) -> bool {
        self.consecutive_denials >= self.max_consecutive_denials
    }
}

/// Format a user-facing permission prompt using typed policy.
pub fn format_permission_prompt(
    name: &str,
    policy: PermissionPromptPolicy,
    input: &Value,
) -> String {
    let field_str = |field: &str| input.get(field).and_then(|v| v.as_str()).unwrap_or("");
    match policy {
        PermissionPromptPolicy::Command { field } => format!("Run command: {}", field_str(field)),
        PermissionPromptPolicy::Question { field } => format!("Ask user: {}", field_str(field)),
        PermissionPromptPolicy::Path { field } => format!("Allow {name} on {}?", field_str(field)),
        PermissionPromptPolicy::PatchTarget { patch_field } => {
            match crate::tool::patch_target_paths(field_str(patch_field)) {
                paths if paths.is_empty() => format!("Allow {name}?"),
                paths if paths.len() == 1 => format!("Allow {name} on {}?", paths[0]),
                paths => format!(
                    "Allow {name} on {} files? {}",
                    paths.len(),
                    paths.join(", ")
                ),
            }
        }
        PermissionPromptPolicy::Json => format!("Allow {name}?"),
    }
}

/// The risk of an MCP tool whose entry declares none.
///
/// MCP tools always *start* as High risk: the tool is third-party, and its
/// entry is what has to say otherwise — through `tools.<name>.risk` or the
/// entry's `default_tool_risk`. Kept as the single named place that says "no
/// declaration means High", so the fallback cannot drift.
pub fn normalize_mcp_capability(_server: &str, _tool: &str) -> CapabilityRisk {
    CapabilityRisk::High
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::permission::settings::PermissionSettings;

    #[test]
    fn allow_list_matches_exact_name() {
        let mut mgr = PermissionManager::try_new(PermissionMode::Default).unwrap();
        mgr.allow_tool("read_file");
        assert!(mgr.is_always_allowed("read_file", &Value::Null));
    }

    #[test]
    fn deny_increments_consecutive_count() {
        let mut mgr = PermissionManager::try_new(PermissionMode::Default).unwrap();
        mgr.apply_user_choice(UserPermissionChoice::Deny, "bash");
        assert_eq!(mgr.consecutive_denials, 1);
        mgr.apply_user_choice(UserPermissionChoice::Deny, "bash");
        assert_eq!(mgr.consecutive_denials, 2);
    }

    #[test]
    fn allow_resets_consecutive_count() {
        let mut mgr = PermissionManager::try_new(PermissionMode::Default).unwrap();
        mgr.apply_user_choice(UserPermissionChoice::Deny, "bash");
        mgr.apply_user_choice(UserPermissionChoice::AllowOnce, "bash");
        assert_eq!(mgr.consecutive_denials, 0);
    }

    #[test]
    fn plan_mode_denies_write_including_mcp() {
        let mut mgr = PermissionManager::try_new(PermissionMode::Plan).unwrap();
        let decision = mgr.check("bash", CapabilityRisk::Write, &Value::Null);
        assert_eq!(decision.behavior, PermissionBehavior::Deny);
        let mcp_decision = mgr.check("mcp__srv__tool", CapabilityRisk::Write, &Value::Null);
        assert_eq!(mcp_decision.behavior, PermissionBehavior::Deny);
    }

    #[test]
    fn plan_mode_allows_readonly_shell_commands_and_denies_others() {
        // End-to-end through `PermissionPolicy::ShellCommand::resolve`:
        // provably read-only commands become Read and are allowed even in
        // plan mode; everything else is still denied without prompting.
        use crate::tool::PermissionPolicy;

        let policy = PermissionPolicy::ShellCommand {
            command_field: "command",
        };
        let mut mgr = PermissionManager::try_new(PermissionMode::Plan).unwrap();

        for cmd in ["ls", "ls -la", "grep -rn needle .", "git status"] {
            let risk = policy.resolve(&serde_json::json!({ "command": cmd }));
            assert_eq!(risk, CapabilityRisk::Read, "{cmd} should resolve to Read");
            let decision = mgr.check("bash", risk, &serde_json::json!({ "command": cmd }));
            assert_eq!(
                decision.behavior,
                PermissionBehavior::Allow,
                "{cmd} should be allowed in plan mode"
            );
        }

        for cmd in [
            "rm -rf /",
            "cargo test",
            "ls | wc -l",
            "git push",
            "find . -delete",
        ] {
            let risk = policy.resolve(&serde_json::json!({ "command": cmd }));
            assert_eq!(risk, CapabilityRisk::Write, "{cmd} should resolve to Write");
            let decision = mgr.check("bash", risk, &serde_json::json!({ "command": cmd }));
            assert_eq!(
                decision.behavior,
                PermissionBehavior::Deny,
                "{cmd} should be denied in plan mode"
            );
        }
    }

    #[test]
    fn auto_mode_allows_non_high_capabilities() {
        let mut mgr = PermissionManager::try_new(PermissionMode::Auto).unwrap();
        let decision = mgr.check("bash", CapabilityRisk::Write, &Value::Null);
        assert_eq!(decision.behavior, PermissionBehavior::Allow);
    }

    #[test]
    fn auto_mode_allows_high_risk_capabilities() {
        let mut mgr = PermissionManager::try_new(PermissionMode::Auto).unwrap();
        let decision = mgr.check("bash", CapabilityRisk::High, &Value::Null);
        assert_eq!(decision.behavior, PermissionBehavior::Allow);
    }

    #[test]
    fn default_mode_asks_for_write() {
        let mut mgr = PermissionManager::try_new(PermissionMode::Default).unwrap();
        let decision = mgr.check("bash", CapabilityRisk::Write, &Value::Null);
        assert_eq!(decision.behavior, PermissionBehavior::Ask);
    }

    #[test]
    fn default_mode_allows_resolved_read_capability() {
        let mut manager = PermissionManager::try_new(PermissionMode::Default).unwrap();
        let decision = manager.check("read_file", CapabilityRisk::Read, &Value::Null);
        assert_eq!(decision.behavior, PermissionBehavior::Allow);
    }

    #[test]
    fn path_prompt_policy_preserves_file_prompt() {
        let prompt = format_permission_prompt(
            "edit_file",
            PermissionPromptPolicy::Path { field: "path" },
            &serde_json::json!({"path": "src/lib.rs"}),
        );
        assert_eq!(prompt, "Allow edit_file on src/lib.rs?");
    }

    #[test]
    fn always_allow_and_check_skips_prompt() {
        let mut mgr = PermissionManager::try_new(PermissionMode::Default).unwrap();
        mgr.allow_tool("bash");
        let decision = mgr.check("bash", CapabilityRisk::Write, &Value::Null);
        assert_eq!(decision.behavior, PermissionBehavior::Allow);
    }

    #[test]
    fn high_risk_is_allowed_only_after_an_explicit_allow() {
        // The first call always asks; once the user grants the tool, the grant
        // is honoured rather than recorded and ignored. A *bare* allow is the
        // broadest form of that grant (every input), so it covers High too —
        // mode and settings rules are what still gate it, and both are
        // asserted elsewhere.
        let mut mgr = PermissionManager::try_new(PermissionMode::Default).unwrap();
        let asked = mgr.check("bash", CapabilityRisk::High, &Value::Null);
        assert_eq!(asked.behavior, PermissionBehavior::Ask);

        mgr.allow_tool("bash");
        let decision = mgr.check("bash", CapabilityRisk::High, &Value::Null);
        assert_eq!(decision.behavior, PermissionBehavior::Allow);
    }

    // ── Settings-aware tests ────────────────────────────────────

    /// Temp project `.tact/settings.json` + Default-mode manager. Keep `dir`
    /// alive for the test duration so the path stays valid.
    fn mgr_with_project_settings(json: &str) -> (tempfile::TempDir, PermissionManager) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".tact/settings.json");
        std::fs::create_dir_all(dir.path().join(".tact")).unwrap();
        std::fs::write(&path, json).unwrap();
        let settings = PermissionSettings::load_from(&path, None);
        let mgr =
            PermissionManager::try_new_with_settings(PermissionMode::Default, settings).unwrap();
        (dir, mgr)
    }

    #[test]
    fn loaded_allow_rule_skips_write_prompt() {
        let (_dir, mut mgr) = mgr_with_project_settings(
            r#"{"permissions": {"allow": ["bash(command:cargo test *)"]}}"#,
        );

        // Matching input → Allow (skips prompt)
        let decision = mgr.check(
            "bash",
            CapabilityRisk::Write,
            &serde_json::json!({"command": "cargo test -p tact"}),
        );
        assert_eq!(
            decision.behavior,
            PermissionBehavior::Allow,
            "Matching allow rule should skip prompt"
        );

        // Non-matching input → Ask (fall through to default)
        let decision2 = mgr.check(
            "bash",
            CapabilityRisk::Write,
            &serde_json::json!({"command": "git push"}),
        );
        assert_eq!(
            decision2.behavior,
            PermissionBehavior::Ask,
            "Non-matching input should fall through to default Ask"
        );
    }

    #[test]
    fn loaded_ask_rule_still_prompts() {
        let (_dir, mut mgr) = mgr_with_project_settings(
            r#"{"permissions": {"ask": ["bash(command:cargo test *)"]}}"#,
        );

        let decision = mgr.check(
            "bash",
            CapabilityRisk::Write,
            &serde_json::json!({"command": "cargo test -p tact"}),
        );
        assert_eq!(
            decision.behavior,
            PermissionBehavior::Ask,
            "Matching ask rule should prompt"
        );
    }

    #[test]
    fn loaded_deny_rule_blocks_before_prompt() {
        let (_dir, mut mgr) =
            mgr_with_project_settings(r#"{"permissions": {"deny": ["bash(command:rm *)"]}}"#);

        let decision = mgr.check(
            "bash",
            CapabilityRisk::Write,
            &serde_json::json!({"command": "rm -rf /"}),
        );
        assert_eq!(
            decision.behavior,
            PermissionBehavior::Deny,
            "Matching deny rule should block before prompt"
        );
    }

    #[test]
    fn high_risk_allow_rule_is_respected() {
        let (_dir, mut mgr) = mgr_with_project_settings(
            r#"{"permissions": {"allow": ["bash(command:cargo test *)"]}}"#,
        );

        let decision = mgr.check(
            "bash",
            CapabilityRisk::High,
            &serde_json::json!({"command": "cargo test -p tact"}),
        );
        assert_eq!(
            decision.behavior,
            PermissionBehavior::Allow,
            "High risk should be allowed when a matching allow rule exists"
        );
    }

    #[test]
    fn high_risk_ask_rule_still_asks() {
        let (_dir, mut mgr) = mgr_with_project_settings(
            r#"{"permissions": {"ask": ["bash(command:cargo test *)"]}}"#,
        );

        let decision = mgr.check(
            "bash",
            CapabilityRisk::High,
            &serde_json::json!({"command": "cargo test -p tact"}),
        );
        assert_eq!(
            decision.behavior,
            PermissionBehavior::Ask,
            "High risk should ask when a matching ask rule exists"
        );
    }

    #[test]
    fn high_risk_deny_rule_blocks_high_risk() {
        let (_dir, mut mgr) =
            mgr_with_project_settings(r#"{"permissions": {"deny": ["bash(command:rm *)"]}}"#);

        let decision = mgr.check(
            "bash",
            CapabilityRisk::High,
            &serde_json::json!({"command": "rm -rf /"}),
        );
        assert_eq!(
            decision.behavior,
            PermissionBehavior::Deny,
            "High risk should be denied when a matching deny rule exists"
        );

        // Non-matching input → no deny rule matches, high risk → Ask.
        let decision2 = mgr.check(
            "bash",
            CapabilityRisk::High,
            &serde_json::json!({"command": "cargo test"}),
        );
        assert_eq!(
            decision2.behavior,
            PermissionBehavior::Ask,
            "High risk should ask when no deny rule matches"
        );
    }

    #[test]
    fn allow_tool_with_input_persists_and_adds_to_in_memory() {
        let dir = tempfile::tempdir().unwrap();
        let project_file = dir.path().join(".tact/settings.json");
        std::fs::create_dir_all(dir.path().join(".tact")).unwrap();

        let settings = PermissionSettings::load_from(&project_file, None);
        let mut mgr =
            PermissionManager::try_new_with_settings(PermissionMode::Default, settings).unwrap();

        let input = serde_json::json!({"command": "cargo test"});
        mgr.allow_tool_with_input(
            "bash",
            PermissionPromptPolicy::Command { field: "command" },
            &input,
        );

        // In-memory: bare tool name is NOT added (privilege escalation prevention).
        // The generated rule only allows the specific input, not all bash calls.
        assert!(
            !mgr.is_always_allowed("bash", &Value::Null),
            "Bare tool name must not be added"
        );

        // On disk: the generated parameter rule should exist
        let content = std::fs::read_to_string(&project_file).unwrap();
        let doc: serde_json::Value = serde_json::from_str(&content).unwrap();
        let allow = doc
            .pointer("/permissions/allow")
            .and_then(|v| v.as_array())
            .unwrap();
        assert_eq!(allow.len(), 1);
        assert_eq!(allow[0].as_str(), Some("bash(command:cargo test)"));

        // After persist, the in-memory cached effective rules should also know
        // about the rule so subsequent checks match via settings before falling
        // through.
        let decision = mgr.check(
            "bash",
            CapabilityRisk::Write,
            &serde_json::json!({"command": "cargo test"}),
        );
        assert_eq!(
            decision.behavior,
            PermissionBehavior::Allow,
            "Generated rule should match subsequent check with same input"
        );

        // Privilege escalation regression: a different command with the same
        // tool must NOT be allowed by the cached settings rule.
        let decision2 = mgr.check(
            "bash",
            CapabilityRisk::Write,
            &serde_json::json!({"command": "rm -rf /"}),
        );
        assert_eq!(
            decision2.behavior,
            PermissionBehavior::Ask,
            "Different input with same tool must fall through to Ask"
        );
    }

    /// "Allow for this session" is the in-memory twin of "always allow this
    /// tool": the same narrow rule, but it must never reach the settings file.
    #[test]
    fn session_allow_records_a_narrow_rule_and_never_persists() {
        let dir = tempfile::tempdir().unwrap();
        let project_file = dir.path().join(".tact/settings.json");
        std::fs::create_dir_all(dir.path().join(".tact")).unwrap();
        std::fs::write(&project_file, r#"{"permissions":{"allow":[]}}"#).unwrap();

        let settings = PermissionSettings::load_from(&project_file, None);
        let mut mgr =
            PermissionManager::try_new_with_settings(PermissionMode::Default, settings).unwrap();

        let shown = serde_json::json!({"command": "cargo test"});
        let outcome = mgr.allow_tool_for_session(
            "bash",
            PermissionPromptPolicy::Command { field: "command" },
            &shown,
        );
        assert_eq!(outcome, AllowOutcome::Recorded);

        // The call the user was shown is approved...
        assert!(mgr.is_always_allowed("bash", &shown));
        // ...but no tool-wide approval was recorded alongside it.
        assert!(
            !mgr.is_always_allowed("bash", &Value::Null),
            "a session approval must not become a tool-wide one"
        );

        // An unrelated — here, high-risk — command still asks.
        let decision = mgr.check(
            "bash",
            CapabilityRisk::High,
            &serde_json::json!({"command": "sudo rm -rf /var"}),
        );
        assert_eq!(
            decision.behavior,
            PermissionBehavior::Ask,
            "an unrelated command must not ride on the session approval"
        );

        // And nothing reached the settings file.
        let content = std::fs::read_to_string(&project_file).unwrap();
        let doc: serde_json::Value = serde_json::from_str(&content).unwrap();
        let allow = doc
            .pointer("/permissions/allow")
            .and_then(|v| v.as_array())
            .unwrap();
        assert!(
            allow.is_empty(),
            "a session-scoped approval must not touch the settings file: {allow:?}"
        );
    }

    /// The wide end of the popup. A persisted program-level rule, and the one
    /// thing it must still not do: cover a chained command.
    #[test]
    fn prefix_allow_persists_and_still_refuses_a_chained_command() {
        let dir = tempfile::tempdir().unwrap();
        let project_file = dir.path().join(".tact/settings.json");
        std::fs::create_dir_all(dir.path().join(".tact")).unwrap();
        std::fs::write(&project_file, r#"{"permissions":{"allow":[]}}"#).unwrap();

        let settings = PermissionSettings::load_from(&project_file, None);
        let mut mgr =
            PermissionManager::try_new_with_settings(PermissionMode::Default, settings).unwrap();

        let outcome = mgr.allow_tool_as_prefix(
            "bash",
            PermissionPromptPolicy::Command { field: "command" },
            &serde_json::json!({"command": "cargo test --lib"}),
        );
        assert_eq!(outcome, AllowOutcome::Recorded);

        let content = std::fs::read_to_string(&project_file).unwrap();
        let doc: serde_json::Value = serde_json::from_str(&content).unwrap();
        let allow = doc
            .pointer("/permissions/allow")
            .and_then(|v| v.as_array())
            .unwrap();
        assert_eq!(allow.len(), 1);
        assert_eq!(allow[0].as_str(), Some("bash(command:^cargo test)"));

        // The plain invocation is covered...
        let allowed = mgr.check(
            "bash",
            CapabilityRisk::Write,
            &serde_json::json!({"command": "cargo test --all-targets"}),
        );
        assert_eq!(allowed.behavior, PermissionBehavior::Allow);
        // ...and the chained one is not.
        let chained = mgr.check(
            "bash",
            CapabilityRisk::Write,
            &serde_json::json!({"command": "cargo test; rm -rf /"}),
        );
        assert_eq!(chained.behavior, PermissionBehavior::Ask);
    }

    /// The popup offers one choice for both kinds of prompt, so the dispatch
    /// has to produce a folder rule for a path and a program rule for a command.
    #[test]
    fn prefix_rule_for_dispatches_on_the_prompt_kind() {
        let folder = PermissionManager::prefix_rule_for(
            "edit_file",
            PermissionPromptPolicy::Path { field: "path" },
            &serde_json::json!({"path": "docs/usage.md"}),
        )
        .expect("a nested path should be offerable");
        assert_eq!(folder.to_rule_string(), "edit_file(path:@docs)");

        let program = PermissionManager::prefix_rule_for(
            "bash",
            PermissionPromptPolicy::Command { field: "command" },
            &serde_json::json!({"command": "cargo test"}),
        )
        .expect("a two-word command should be offerable");
        assert_eq!(program.to_rule_string(), "bash(command:^cargo test)");

        // A question prompt has neither a program nor a folder to name.
        assert!(
            PermissionManager::prefix_rule_for(
                "ask_user",
                PermissionPromptPolicy::Question { field: "question" },
                &serde_json::json!({"question": "continue?"}),
            )
            .is_none()
        );
    }

    /// What gates the option's presence is the same decision that produces the
    /// rule the popup previews, so the two cannot drift apart.
    #[test]
    fn prefix_rule_for_gates_and_describes_the_choice() {
        let rule = PermissionManager::prefix_rule_for(
            "bash",
            PermissionPromptPolicy::Command { field: "command" },
            &serde_json::json!({"command": "cargo test --lib"}),
        )
        .expect("a two-word command should be offerable");
        assert_eq!(rule.to_rule_string(), "bash(command:^cargo test)");

        // A chained command is not offerable at all — so the popup will not
        // show a button that could only record nothing.
        assert!(
            PermissionManager::prefix_rule_for(
                "bash",
                PermissionPromptPolicy::Command { field: "command" },
                &serde_json::json!({"command": "cargo test; rm -rf /"}),
            )
            .is_none()
        );
    }

    /// Exact-match narrowness is the whole safety argument for offering the
    /// session option on `bash`, so pin it: a longer command is a different
    /// command, and a chained one is a different command twice over.
    #[test]
    fn session_allow_does_not_widen_to_a_longer_command() {
        let mut mgr = PermissionManager::try_new(PermissionMode::Default).unwrap();
        mgr.allow_tool_for_session(
            "bash",
            PermissionPromptPolicy::Command { field: "command" },
            &serde_json::json!({"command": "cargo test"}),
        );

        for other in [
            "cargo test --lib",
            "cargo test; rm -rf /tmp/x",
            "cargo build",
        ] {
            assert!(
                !mgr.is_always_allowed("bash", &serde_json::json!({"command": other})),
                "{other} must not be covered by a rule for `cargo test`"
            );
        }
    }

    /// When no rule narrower than the whole tool can be built, the session
    /// choice records nothing — it must not fall back to a bare allow, which is
    /// how a `bash` command containing a `:` once granted every future command.
    #[test]
    fn session_allow_records_nothing_when_the_rule_cannot_be_narrowed() {
        let mut mgr = PermissionManager::try_new(PermissionMode::Default).unwrap();
        let outcome = mgr.allow_tool_for_session(
            "bash",
            PermissionPromptPolicy::Command { field: "command" },
            &serde_json::json!({"command": "git commit -m \"fix: thing\""}),
        );
        assert_eq!(outcome, AllowOutcome::NotNarrowable);
        assert!(
            !mgr.is_always_allowed("bash", &Value::Null),
            "a refused rule must not leave a tool-wide approval behind"
        );
        assert!(mgr.rules().is_empty());
    }

    #[test]
    fn allow_tool_with_input_failure_does_not_deny() {
        // Isolated temp path: create a regular file where the `.tact` directory
        // would be, so the directory creation inside persist_project_allow fails.
        let dir = tempfile::tempdir().unwrap();
        let tact_dir = dir.path().join(".tact");
        // Write a regular file at the path that would need to be a directory.
        std::fs::write(&tact_dir, "i am a file, not a directory").unwrap();

        let bad_path = tact_dir.join("settings.json");
        let settings = PermissionSettings::load_from(&bad_path, None);
        let mut mgr =
            PermissionManager::try_new_with_settings(PermissionMode::Default, settings).unwrap();

        let input = serde_json::json!({"command": "ls"});
        // This should not panic or return an error — persistence failure is
        // logged as a warning, and the tool remains approved.
        mgr.allow_tool_with_input(
            "bash",
            PermissionPromptPolicy::Command { field: "command" },
            &input,
        );

        // Bare tool name is NOT added when persistence fails (and never should
        // be for input-aware approvals).  The current call was approved by the
        // user; future calls go through normal check flow.
        assert!(!mgr.is_always_allowed("bash", &Value::Null));
    }

    #[test]
    fn settings_none_falls_through_to_in_memory() {
        let mut mgr = PermissionManager::try_new(PermissionMode::Default).unwrap();
        // No settings loaded; uses in-memory always_allowed_tools.
        mgr.allow_tool("bash");

        let decision = mgr.check("bash", CapabilityRisk::Write, &Value::Null);
        assert_eq!(decision.behavior, PermissionBehavior::Allow);
    }

    #[test]
    fn deny_rule_takes_precedence_over_allow() {
        let dir = tempfile::tempdir().unwrap();
        let project_file = dir.path().join(".tact/settings.json");
        std::fs::create_dir_all(dir.path().join(".tact")).unwrap();
        std::fs::write(
            &project_file,
            r#"{
                "permissions": {
                    "allow": ["bash(command:cargo *)"],
                    "deny": ["bash(command:cargo test --doc *)"]
                }
            }"#,
        )
        .unwrap();

        let settings = PermissionSettings::load_from(&project_file, None);
        let mut mgr =
            PermissionManager::try_new_with_settings(PermissionMode::Default, settings).unwrap();

        // Matches both allow and deny → deny wins
        let decision = mgr.check(
            "bash",
            CapabilityRisk::Write,
            &serde_json::json!({"command": "cargo test --doc foobar"}),
        );
        assert_eq!(decision.behavior, PermissionBehavior::Deny);

        // Matches only allow → allow
        let decision2 = mgr.check(
            "bash",
            CapabilityRisk::Write,
            &serde_json::json!({"command": "cargo build"}),
        );
        assert_eq!(decision2.behavior, PermissionBehavior::Allow);
    }

    // ── Dispatch-facing tests ──────────────────────────────
    //
    // These tests exercise the PermissionManager::check path with structured
    // input and loaded parameter rules — the same codepath used during tool
    // dispatch.  Full async dispatch (agent_loop, tool resolution, MCP) cannot
    // be isolated in a synchronous unit test because it depends on tokio
    // runtime, MockClient with streaming, and the entire Agent/ToolRouter
    // infrastructure.  The manager-level check() is the narrowest synchronous
    // boundary where structured input meets permission evaluation.

    #[test]
    fn structured_input_reaches_permission_evaluation() {
        // Verify that a loaded parameter rule (field + glob) is correctly
        // evaluated by check() with structured input, simulating what
        // dispatch passes to the manager.
        let dir = tempfile::tempdir().unwrap();
        let project_file = dir.path().join(".tact/settings.json");
        std::fs::create_dir_all(dir.path().join(".tact")).unwrap();
        std::fs::write(
            &project_file,
            r#"{"permissions": {"allow": ["edit_file(path:src/*.rs)"]}}"#,
        )
        .unwrap();

        let settings = PermissionSettings::load_from(&project_file, None);
        let mut mgr =
            PermissionManager::try_new_with_settings(PermissionMode::Default, settings).unwrap();

        // Structured input matching the parameter rule → Allow
        let decision = mgr.check(
            "edit_file",
            CapabilityRisk::Write,
            &serde_json::json!({"path": "src/lib.rs", "old_text": "a", "new_text": "b"}),
        );
        assert_eq!(
            decision.behavior,
            PermissionBehavior::Allow,
            "Structured input matching parameter rule should allow"
        );

        // Structured input not matching the parameter rule → Ask (fall through)
        let decision2 = mgr.check(
            "edit_file",
            CapabilityRisk::Write,
            &serde_json::json!({"path": "README.md", "old_text": "a", "new_text": "b"}),
        );
        assert_eq!(
            decision2.behavior,
            PermissionBehavior::Ask,
            "Non-matching structured input should fall through to Ask"
        );
    }

    #[test]
    fn structured_input_parameter_rule_with_capability_risk() {
        // Simulate dispatch: each tool call carries a CapabilityRisk (Read,
        // Write, High) from its metadata.  Parameter rules are evaluated
        // respecting the risk level.
        let dir = tempfile::tempdir().unwrap();
        let project_file = dir.path().join(".tact/settings.json");
        std::fs::create_dir_all(dir.path().join(".tact")).unwrap();
        std::fs::write(
            &project_file,
            r#"{"permissions": {"allow": ["bash(command:cargo test *)"]}}"#,
        )
        .unwrap();

        let settings = PermissionSettings::load_from(&project_file, None);
        let mut mgr =
            PermissionManager::try_new_with_settings(PermissionMode::Default, settings).unwrap();

        // Write risk: matching parameter allow rule → Allow
        let decision = mgr.check(
            "bash",
            CapabilityRisk::Write,
            &serde_json::json!({"command": "cargo test -p tact"}),
        );
        assert_eq!(decision.behavior, PermissionBehavior::Allow);

        // High risk: matching allow rule → Allow (user explicitly trusts this)
        let decision2 = mgr.check(
            "bash",
            CapabilityRisk::High,
            &serde_json::json!({"command": "cargo test -p tact"}),
        );
        assert_eq!(
            decision2.behavior,
            PermissionBehavior::Allow,
            "High risk should be allowed when matching allow rule exists"
        );

        // High risk: matching deny rule → Deny (deny blocks high-risk)
        let project_file2 = dir.path().join(".tact/settings.json");
        std::fs::write(
            &project_file2,
            r#"{"permissions": {"deny": ["bash(command:rm *)"]}}"#,
        )
        .unwrap();
        let settings2 = PermissionSettings::load_from(&project_file2, None);
        let mut mgr2 =
            PermissionManager::try_new_with_settings(PermissionMode::Default, settings2).unwrap();

        let decision3 = mgr2.check(
            "bash",
            CapabilityRisk::High,
            &serde_json::json!({"command": "rm -rf /"}),
        );
        assert_eq!(
            decision3.behavior,
            PermissionBehavior::Deny,
            "High risk should be denied when matching deny rule exists"
        );
    }

    // ── Fix 4: Plan/Auto mode with settings ─────────────────────────

    #[test]
    fn plan_mode_with_settings_allow_still_denies_writes() {
        // Plan mode is authoritative: even if a settings allow rule matches,
        // write/High operations must be denied.
        let dir = tempfile::tempdir().unwrap();
        let project_file = dir.path().join(".tact/settings.json");
        std::fs::create_dir_all(dir.path().join(".tact")).unwrap();
        std::fs::write(&project_file, r#"{"permissions": {"allow": ["bash"]}}"#).unwrap();

        let settings = PermissionSettings::load_from(&project_file, None);
        let mut mgr =
            PermissionManager::try_new_with_settings(PermissionMode::Plan, settings).unwrap();

        // Write risk — Plan mode denies regardless of allow rule.
        let decision = mgr.check(
            "bash",
            CapabilityRisk::Write,
            &serde_json::json!({"command": "ls"}),
        );
        assert_eq!(
            decision.behavior,
            PermissionBehavior::Deny,
            "Plan mode denies writes even with matching settings allow"
        );

        // High risk — Plan mode denies regardless of allow rule.
        let decision2 = mgr.check(
            "bash",
            CapabilityRisk::High,
            &serde_json::json!({"command": "ls"}),
        );
        assert_eq!(
            decision2.behavior,
            PermissionBehavior::Deny,
            "Plan mode denies high-risk even with matching settings allow"
        );

        // Read still works in Plan mode.
        let decision3 = mgr.check(
            "read_file",
            CapabilityRisk::Read,
            &serde_json::json!({"path": "foo.txt"}),
        );
        assert_eq!(
            decision3.behavior,
            PermissionBehavior::Allow,
            "Plan mode allows reads"
        );
    }

    #[test]
    fn auto_mode_with_settings_deny_still_allows() {
        // Auto mode is authoritative: even if a settings deny rule matches,
        // all non-read operations are auto-approved.
        let dir = tempfile::tempdir().unwrap();
        let project_file = dir.path().join(".tact/settings.json");
        std::fs::create_dir_all(dir.path().join(".tact")).unwrap();
        std::fs::write(
            &project_file,
            r#"{"permissions": {"deny": ["bash(command:rm *)"]}}"#,
        )
        .unwrap();

        let settings = PermissionSettings::load_from(&project_file, None);
        let mut mgr =
            PermissionManager::try_new_with_settings(PermissionMode::Auto, settings).unwrap();

        // Write risk — Auto mode allows.
        let decision = mgr.check(
            "bash",
            CapabilityRisk::Write,
            &serde_json::json!({"command": "rm -rf /"}),
        );
        assert_eq!(
            decision.behavior,
            PermissionBehavior::Allow,
            "Auto mode allows writes even with matching settings deny"
        );

        // High risk — Auto mode allows everything.
        let decision2 = mgr.check(
            "bash",
            CapabilityRisk::High,
            &serde_json::json!({"command": "rm -rf /"}),
        );
        assert_eq!(
            decision2.behavior,
            PermissionBehavior::Allow,
            "Auto mode allows high risk even with matching settings deny"
        );

        // Read still works in Auto mode.
        let decision3 = mgr.check(
            "read_file",
            CapabilityRisk::Read,
            &serde_json::json!({"path": "foo.txt"}),
        );
        assert_eq!(
            decision3.behavior,
            PermissionBehavior::Allow,
            "Auto mode allows reads"
        );
    }

    // ── Fix 1 regression: prevent same-session privilege escalation ──

    #[test]
    fn allow_tool_with_input_prevents_privilege_escalation() {
        // After allowing bash "cargo test" via allow_tool_with_input,
        // a different bash command (rm -rf /) must NOT be automatically
        // allowed. The generated rule is input-specific.
        let dir = tempfile::tempdir().unwrap();
        let project_file = dir.path().join(".tact/settings.json");
        std::fs::create_dir_all(dir.path().join(".tact")).unwrap();

        let settings = PermissionSettings::load_from(&project_file, None);
        let mut mgr =
            PermissionManager::try_new_with_settings(PermissionMode::Default, settings).unwrap();

        // Allow bash "cargo test" — generates rule bash(command:cargo test)
        mgr.allow_tool_with_input(
            "bash",
            PermissionPromptPolicy::Command { field: "command" },
            &serde_json::json!({"command": "cargo test"}),
        );

        // Same input → Allow (via cached settings rule)
        let decision = mgr.check(
            "bash",
            CapabilityRisk::Write,
            &serde_json::json!({"command": "cargo test"}),
        );
        assert_eq!(
            decision.behavior,
            PermissionBehavior::Allow,
            "Same input should be allowed by generated rule"
        );

        // Different input → Ask (no bare tool name grants unrelated inputs)
        let decision2 = mgr.check(
            "bash",
            CapabilityRisk::Write,
            &serde_json::json!({"command": "rm -rf /"}),
        );
        assert_eq!(
            decision2.behavior,
            PermissionBehavior::Ask,
            "Different input with same tool must NOT be auto-allowed"
        );

        // Bare tool name must NOT be in always_allowed_tools
        assert!(
            !mgr.is_always_allowed("bash", &Value::Null),
            "Bare tool name must not be in always_allowed_tools"
        );
    }

    #[test]
    fn non_interactive_ask_user_allows_writes_and_denies_high_risk() {
        let mut mgr = PermissionManager::try_new(PermissionMode::Default).unwrap();

        let approved = mgr.ask_user("bash", CapabilityRisk::Write).unwrap();
        assert!(
            approved,
            "Non-interactive ask_user should allow Write operations"
        );
        assert_eq!(
            mgr.consecutive_denials, 0,
            "Write allow should not increment denials"
        );

        // Defensive: Read should not reach ask_user, but AllowOnce if it does.
        assert!(mgr.ask_user("read_file", CapabilityRisk::Read).unwrap());

        let approved2 = mgr.ask_user("rm", CapabilityRisk::High).unwrap();
        assert!(
            !approved2,
            "Non-interactive ask_user should deny High-risk operations"
        );
        assert_eq!(
            mgr.consecutive_denials, 1,
            "High-risk denial should increment denials"
        );
    }

    #[test]
    fn snapshot_round_trip_preserves_mode_and_allow_list() {
        let mut mgr = PermissionManager::try_new(PermissionMode::Plan).unwrap();
        mgr.allow_tool("bash");
        mgr.allow_tool("write_file");

        let snap = mgr.snapshot();
        assert_eq!(snap.mode, PermissionMode::Plan);
        assert_eq!(
            snap.always_allowed_tools,
            vec!["bash".to_string(), "write_file".to_string()]
        );
        assert!(snap.settings.is_none());

        let restored = PermissionManager::from_snapshot(snap);
        assert_eq!(restored.mode(), PermissionMode::Plan);
        assert_eq!(restored.rules(), &["bash", "write_file"]);
    }

    #[test]
    fn snapshot_round_trip_with_settings() {
        let dir = tempfile::tempdir().unwrap();
        let project_file = dir.path().join(".tact/settings.json");
        std::fs::create_dir_all(dir.path().join(".tact")).unwrap();
        let settings = PermissionSettings::load_from(&project_file, None);
        let mut mgr =
            PermissionManager::try_new_with_settings(PermissionMode::Auto, settings).unwrap();
        mgr.allow_tool("bash");

        let restored = PermissionManager::from_snapshot(mgr.snapshot());
        assert_eq!(restored.mode(), PermissionMode::Auto);
        assert!(restored.settings.is_some(), "settings must be cloned");
        assert!(restored.rules().contains(&"bash".to_string()));
    }

    #[test]
    fn from_snapshot_resets_denial_counters() {
        let mut mgr = PermissionManager::try_new(PermissionMode::Default).unwrap();
        // Push the denial counter past zero, then snapshot + restore.
        mgr.apply_user_choice(UserPermissionChoice::Deny, "bash");
        mgr.apply_user_choice(UserPermissionChoice::Deny, "bash");
        assert_eq!(mgr.consecutive_denials, 2);

        let restored = PermissionManager::from_snapshot(mgr.snapshot());
        assert_eq!(restored.consecutive_denials, 0);
        assert_eq!(restored.max_consecutive_denials, 3);
    }

    #[test]
    fn from_snapshot_plan_parent_yields_read_only_child() {
        // Regression: a read-only (Plan) parent must not spawn a Default-mode
        // subagent that can write. Inheriting the snapshot preserves Plan mode.
        let parent = PermissionManager::try_new(PermissionMode::Plan).unwrap();
        let mut child = PermissionManager::from_snapshot(parent.snapshot());
        assert_eq!(child.mode(), PermissionMode::Plan);
        let decision = child.check("write_file", CapabilityRisk::Write, &Value::Null);
        assert_eq!(decision.behavior, PermissionBehavior::Deny);
    }

    #[test]
    fn from_snapshot_auto_parent_yields_auto_child() {
        let parent = PermissionManager::try_new(PermissionMode::Auto).unwrap();
        let child = PermissionManager::from_snapshot(parent.snapshot());
        assert_eq!(child.mode(), PermissionMode::Auto);
    }

    // ── MCP server-entry auto-approval ───────────────────────────────────

    #[test]
    fn server_auto_approval_skips_the_default_high_risk_prompt() {
        let mut mgr = PermissionManager::try_new(PermissionMode::Default).unwrap();

        // Without the policy a high-risk MCP tool asks…
        let asked = mgr.check("mcp__demo__search", CapabilityRisk::High, &Value::Null);
        assert_eq!(asked.behavior, PermissionBehavior::Ask);
        assert_eq!(mgr.consecutive_denials, 0);

        // …and with it, the prompt is skipped.
        let allowed = mgr.check_with_auto(
            "mcp__demo__search",
            CapabilityRisk::High,
            &Value::Null,
            true,
        );
        assert_eq!(allowed.behavior, PermissionBehavior::Allow);
        assert!(
            allowed.reason.contains("MCP server entry"),
            "{}",
            allowed.reason
        );
    }

    #[test]
    fn a_high_risk_tool_honours_a_granted_always_allow() {
        // The TUI offers "Always allow this tool" for every risk. With no
        // settings store the grant lands in the in-memory list, which the
        // High branch used to skip — so the click was recorded and then
        // ignored, and the next identical call asked again.
        let mut mgr = PermissionManager::try_new(PermissionMode::Default).unwrap();
        let input = serde_json::json!({"query": "notes"});

        let asked = mgr.check("mcp__demo__search", CapabilityRisk::High, &input);
        assert_eq!(asked.behavior, PermissionBehavior::Ask);

        mgr.allow_tool_with_input("mcp__demo__search", PermissionPromptPolicy::Json, &input);

        let allowed = mgr.check("mcp__demo__search", CapabilityRisk::High, &input);
        assert_eq!(allowed.behavior, PermissionBehavior::Allow);
        assert!(
            allowed.reason.contains("Always-allowed"),
            "{}",
            allowed.reason
        );
    }

    #[test]
    fn a_high_risk_tool_still_asks_when_nothing_was_ever_allowed() {
        // The headless path depends on this: `ask_user` denies High, so the
        // list must stay the only way in.
        let mut mgr = PermissionManager::try_new(PermissionMode::Default).unwrap();
        let decision = mgr.check("mcp__demo__search", CapabilityRisk::High, &Value::Null);
        assert_eq!(decision.behavior, PermissionBehavior::Ask);
    }

    #[test]
    fn a_granted_always_allow_does_not_unlock_plan_mode() {
        // The grant relaxes the *prompt*, never the mode: plan mode is
        // evaluated before the High branch.
        let mut mgr = PermissionManager::try_new(PermissionMode::Plan).unwrap();
        mgr.allow_tool("mcp__demo__write_note");
        let decision = mgr.check("mcp__demo__write_note", CapabilityRisk::High, &Value::Null);
        assert_eq!(decision.behavior, PermissionBehavior::Deny);
    }

    #[test]
    fn an_explicit_deny_rule_still_outranks_a_granted_always_allow() {
        let (_dir, mut mgr) =
            mgr_with_project_settings(r#"{"permissions": {"deny": ["mcp__demo__write_note"]}}"#);
        mgr.allow_tool("mcp__demo__write_note");
        let decision = mgr.check("mcp__demo__write_note", CapabilityRisk::High, &Value::Null);
        assert_eq!(decision.behavior, PermissionBehavior::Deny);
    }

    #[test]
    fn server_auto_approval_does_not_unlock_plan_mode() {
        // `Read` is allowed before plan mode is consulted, so a server policy
        // must never be expressed as a lower risk.
        let mut mgr = PermissionManager::try_new(PermissionMode::Plan).unwrap();
        let decision = mgr.check_with_auto(
            "mcp__demo__write_note",
            CapabilityRisk::High,
            &Value::Null,
            true,
        );
        assert_eq!(decision.behavior, PermissionBehavior::Deny);
    }

    #[test]
    fn an_explicit_deny_rule_outranks_server_auto_approval() {
        let (_dir, mut mgr) =
            mgr_with_project_settings(r#"{"permissions": {"deny": ["mcp__demo__write_note"]}}"#);

        let decision = mgr.check_with_auto(
            "mcp__demo__write_note",
            CapabilityRisk::High,
            &Value::Null,
            true,
        );
        assert_eq!(decision.behavior, PermissionBehavior::Deny);
    }

    #[test]
    fn an_explicit_ask_rule_outranks_server_auto_approval() {
        let (_dir, mut mgr) =
            mgr_with_project_settings(r#"{"permissions": {"ask": ["mcp__demo__write_note"]}}"#);

        let decision = mgr.check_with_auto(
            "mcp__demo__write_note",
            CapabilityRisk::High,
            &Value::Null,
            true,
        );
        assert_eq!(decision.behavior, PermissionBehavior::Ask);
    }

    #[test]
    fn a_server_auto_tool_still_asks_when_it_is_not_approved() {
        let (_dir, mut mgr) =
            mgr_with_project_settings(r#"{"permissions": {"allow": ["read_file"]}}"#);

        // `auto_approved: false` is the unchanged path: no local rule matches a
        // high-risk MCP tool, so it asks.
        let decision = mgr.check_with_auto(
            "mcp__demo__search",
            CapabilityRisk::High,
            &Value::Null,
            false,
        );
        assert_eq!(decision.behavior, PermissionBehavior::Ask);
    }

    #[test]
    fn check_keeps_the_old_behaviour_without_a_server_policy() {
        let mut mgr = PermissionManager::try_new(PermissionMode::Default).unwrap();
        let decision = mgr.check("mcp__demo__search", CapabilityRisk::High, &Value::Null);
        assert_eq!(decision.behavior, PermissionBehavior::Ask);
    }
}
