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

pub mod settings;

use std::fmt;

use anyhow::Result;
use serde_json::Value;
use strum_macros::{Display, EnumString};

use crate::tool::PermissionPromptPolicy;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Display)]
#[strum(serialize_all = "snake_case")]
pub enum CapabilityRisk {
    Read,
    Write,
    High,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, EnumString)]
#[strum(serialize_all = "snake_case")]
pub enum PermissionMode {
    Default,
    Plan,
    Auto,
}

impl fmt::Display for PermissionMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let label = match self {
            PermissionMode::Default => "default - ask for writes",
            PermissionMode::Plan => "plan - read only",
            PermissionMode::Auto => "auto - allow non-high operations",
        };

        write!(f, "{label}")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionBehavior {
    Allow,
    Deny,
    Ask,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionDecision {
    pub behavior: PermissionBehavior,
    pub reason: String,
}

impl PermissionDecision {
    fn allow(reason: impl Into<String>) -> Self {
        Self {
            behavior: PermissionBehavior::Allow,
            reason: reason.into(),
        }
    }

    fn ask(reason: impl Into<String>) -> Self {
        Self {
            behavior: PermissionBehavior::Ask,
            reason: reason.into(),
        }
    }

    fn deny(reason: impl Into<String>) -> Self {
        Self {
            behavior: PermissionBehavior::Deny,
            reason: reason.into(),
        }
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

impl PermissionManager {
    /// Create a new manager with no loaded permission-settings store.
    ///
    /// This constructor is suitable for isolated tests and callers that
    /// do not have a project directory (e.g. some test harnesses).  It
    /// does not inherit any persistent allow/ask/deny rules.
    pub fn try_new(mode: PermissionMode) -> Result<Self> {
        Ok(Self {
            mode,
            always_allowed_tools: vec!["read_file".to_string()],
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
            always_allowed_tools: vec!["read_file".to_string()],
            consecutive_denials: 0,
            max_consecutive_denials: 3,
            settings: Some(settings),
        })
    }

    pub fn mode(&self) -> PermissionMode {
        self.mode
    }

    pub fn set_mode(&mut self, mode: PermissionMode) {
        self.mode = mode;
    }

    pub fn rules(&self) -> &[String] {
        &self.always_allowed_tools
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
    /// 4. Default mode + high risk → evaluate settings action:
    ///    - Deny → deny (settings deny blocks even high-risk).
    ///    - Ask/Allow/None → ask (high-risk always needs confirmation).
    /// 5. Default mode + matching settings deny → deny.
    /// 6. Default mode + matching settings ask → ask.
    /// 7. Default mode + matching settings allow → allow.
    /// 8. Otherwise → ask (existing default behavior).
    pub fn check(
        &mut self,
        tool_name: &str,
        risk: CapabilityRisk,
        input: &Value,
    ) -> PermissionDecision {
        // 1. Read capabilities are always allowed.
        if risk == CapabilityRisk::Read {
            self.consecutive_denials = 0;
            return PermissionDecision::allow("Read-only capability allowed");
        }

        // 2. Plan mode blocks all write/High operations.
        if self.mode == PermissionMode::Plan {
            return PermissionDecision::deny("Plan mode: write operations are blocked");
        }

        // 3. Auto mode trusts the agent — skip all risk checks.
        if self.mode == PermissionMode::Auto {
            self.consecutive_denials = 0;
            return PermissionDecision::allow("Auto mode: all capabilities auto-approved");
        }

        // At this point we are in Default mode.

        // 4-7. Evaluate loaded settings rules.
        // For high-risk: deny if settings say Deny; allow if settings say
        // Allow (user explicitly trusts this input pattern); otherwise ask.
        // For non-high-risk: follow settings action normally.
        let settings_action = self
            .settings
            .as_ref()
            .map(|settings| settings.cached_effective_rules().action(tool_name, input));

        if risk == CapabilityRisk::High {
            // High-risk: deny if settings say Deny, allow if settings say
            // Allow, otherwise ask.
            match settings_action {
                Some(settings::RuleAction::Deny) => {
                    return PermissionDecision::deny(format!(
                        "Blocked by project permission rule: {}",
                        tool_name
                    ));
                }
                Some(settings::RuleAction::Allow) => {
                    self.consecutive_denials = 0;
                    return PermissionDecision::allow(format!(
                        "Allowed by project permission rule (high-risk): {}",
                        tool_name
                    ));
                }
                _ => {}
            }
            return PermissionDecision::ask(format!(
                "High-risk capability requires approval: {}",
                tool_name
            ));
        }

        if let Some(action) = settings_action {
            match action {
                settings::RuleAction::Deny => {
                    return PermissionDecision::deny(format!(
                        "Blocked by project permission rule: {}",
                        tool_name
                    ));
                }
                settings::RuleAction::Ask => {
                    return PermissionDecision::ask(format!(
                        "Project permission rule requires confirmation: {}",
                        tool_name
                    ));
                }
                settings::RuleAction::Allow => {
                    self.consecutive_denials = 0;
                    return PermissionDecision::allow(format!(
                        "Allowed by project permission rule: {}",
                        tool_name
                    ));
                }
                settings::RuleAction::None => {
                    // No matching rule — fall through to in-memory list.
                }
            }
        }

        // Fallback: in-memory same-session always-allowed list.
        if self.is_always_allowed(tool_name, input) {
            self.consecutive_denials = 0;
            return PermissionDecision::allow(format!("Always allowed tool: {tool_name}"));
        }

        // 8. Default: ask.
        PermissionDecision::ask(format!("Default mode: asking user for {tool_name}"))
    }

    pub fn ask_user(&mut self, tool_name: &str, risk: CapabilityRisk) -> Result<bool> {
        // In non-interactive mode (no UI available), we can't ask the user.
        // Apply reasonable defaults based on risk level:
        // - Write & below → allow (user chose Default mode, we trust non-destructive ops)
        // - High → deny (too dangerous without explicit confirmation)
        let choice = match risk {
            CapabilityRisk::High => {
                eprintln!("[permission] non-interactive: denying high-risk {}", tool_name);
                UserPermissionChoice::Deny
            }
            _ => {
                eprintln!(
                    "[permission] non-interactive: allowing {} \
                     (use --auto mode for unattended runs)",
                    tool_name
                );
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
    /// **Persistence errors are logged as warnings and never convert an
    /// already-approved choice into a denial.**
    pub fn allow_tool_with_input(
        &mut self,
        tool_name: &str,
        policy: PermissionPromptPolicy,
        input: &Value,
    ) {
        // Generate the narrowest parameter-aware rule.
        let rule = settings::PermissionRule::generate(tool_name, policy, input);
        let rule_string = rule.to_rule_string();

        // When no settings store is available, add the generated rule
        // string to the in-memory allow list.  This preserves same-session
        // approval for the specific input without granting unrelated inputs.
        if self.settings.is_none() {
            if !self.always_allowed_tools.contains(&rule_string) {
                self.always_allowed_tools.push(rule_string);
            }
            return;
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

    #[allow(dead_code)]
    pub fn set_max_consecutive_denials(&mut self, max: usize) {
        self.max_consecutive_denials = max;
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
        PermissionPromptPolicy::Json => format!("Allow {name}?"),
    }
}

/// MCP tools always start as High risk.
pub fn normalize_mcp_capability(_server: &str, _tool: &str) -> CapabilityRisk {
    CapabilityRisk::High
}

#[allow(dead_code)]
fn truncate_for_prompt(input: &Value, _max_chars: usize) -> String {
    input.to_string()
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
    fn high_risk_requires_approval_even_for_allowed_tool() {
        let mut mgr = PermissionManager::try_new(PermissionMode::Default).unwrap();
        mgr.allow_tool("bash");
        let decision = mgr.check("bash", CapabilityRisk::High, &Value::Null);
        assert_eq!(decision.behavior, PermissionBehavior::Ask);
    }

    // ── Settings-aware tests ────────────────────────────────────

    #[test]
    fn loaded_allow_rule_skips_write_prompt() {
        // Build settings with an allow rule for bash.
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
        let mut mgr2 = PermissionManager::try_new_with_settings(
            PermissionMode::Default,
            PermissionSettings::load_from(&project_file, None),
        )
        .unwrap();
        let decision2 = mgr2.check(
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
        let dir = tempfile::tempdir().unwrap();
        let project_file = dir.path().join(".tact/settings.json");
        std::fs::create_dir_all(dir.path().join(".tact")).unwrap();
        std::fs::write(
            &project_file,
            r#"{"permissions": {"ask": ["bash(command:cargo test *)"]}}"#,
        )
        .unwrap();

        let settings = PermissionSettings::load_from(&project_file, None);
        let mut mgr =
            PermissionManager::try_new_with_settings(PermissionMode::Default, settings).unwrap();

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
            PermissionManager::try_new_with_settings(PermissionMode::Default, settings).unwrap();

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

        // A matching allow rule should permit High-risk tools — the user
        // explicitly configured this trust.
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
        let dir = tempfile::tempdir().unwrap();
        let project_file = dir.path().join(".tact/settings.json");
        std::fs::create_dir_all(dir.path().join(".tact")).unwrap();
        std::fs::write(
            &project_file,
            r#"{"permissions": {"ask": ["bash(command:cargo test *)"]}}"#,
        )
        .unwrap();

        let settings = PermissionSettings::load_from(&project_file, None);
        let mut mgr =
            PermissionManager::try_new_with_settings(PermissionMode::Default, settings).unwrap();

        // A matching ask rule should still prompt for High-risk tools.
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
            PermissionManager::try_new_with_settings(PermissionMode::Default, settings).unwrap();

        // A matching deny rule blocks even high-risk tools.
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

    // ── Fix 2: Non-interactive ask_user behavior ──────────────

    #[test]
    fn non_interactive_ask_user_allows_writes_and_denies_high_risk() {
        let mut mgr = PermissionManager::try_new(PermissionMode::Default).unwrap();

        // Write risk: allow in non-interactive mode
        let approved = mgr.ask_user("bash", CapabilityRisk::Write).unwrap();
        assert!(
            approved,
            "Non-interactive ask_user should allow Write operations"
        );
        assert_eq!(
            mgr.consecutive_denials, 0,
            "Write allow should not increment denials"
        );

        // High risk: deny in non-interactive mode
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
    fn non_interactive_read_should_not_call_ask_user_but_still_works() {
        let mut mgr = PermissionManager::try_new(PermissionMode::Default).unwrap();
        // Read operations are handled before ask_user is ever called,
        // but if it were called, they should be allowed.
        let approved = mgr.ask_user("read_file", CapabilityRisk::Read).unwrap();
        assert!(approved, "Non-interactive ask_user should allow Read");
    }
}
