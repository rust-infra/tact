//! Tact adapters between the product and kernel contracts.

// These adapters are staged before composition cutover. Remove the module-level
// allowance as each adapter gains its production caller.
#[allow(dead_code)]
pub(crate) mod activation_view;
#[cfg(test)]
mod baseline;
#[allow(dead_code)]
pub(crate) mod cancel_bridge;
#[allow(dead_code)]
pub(crate) mod compaction_adapter;
#[allow(dead_code)]
pub(crate) mod composition;
#[allow(dead_code)]
pub(crate) mod context_adapter;
#[allow(dead_code)]
pub(crate) mod event_adapter;
#[allow(dead_code)]
pub(crate) mod facade_state;
#[allow(dead_code)]
pub(crate) mod gate_adapter;
#[allow(dead_code)]
pub(crate) mod hook_adapter;
#[allow(dead_code)]
pub(crate) mod model_adapter;
#[allow(dead_code)]
pub(crate) mod noop_transcript;
#[allow(dead_code)]
pub(crate) mod permission_authority;
#[allow(dead_code)]
pub(crate) mod prompt_adapter;
#[allow(dead_code)]
pub(crate) mod services;
#[allow(dead_code)]
pub(crate) mod store_adapter;
#[allow(dead_code)]
pub(crate) mod tool_adapter;
#[allow(dead_code)]
pub(crate) mod tool_presentation;
#[allow(dead_code)]
pub(crate) mod usage_adapter;

use tact_protocol::TokenUsageInfo;

/// Convert provider-neutral token accounting into the host-facing protocol type.
pub(crate) fn host_neutral_token_usage(usage: &tact_agent_api::TokenUsage) -> TokenUsageInfo {
    TokenUsageInfo {
        prompt: usage.input_tokens,
        completion: usage.output_tokens,
        total: usage.total_tokens,
        prompt_cache_hit_tokens: usage.cache_read_input_tokens,
        prompt_cache_miss_tokens: usage.cache_miss_input_tokens,
        reasoning_tokens: usage.reasoning_tokens,
    }
}

/// Convert the host-facing usage type for provider test fixtures and other
/// legacy call sites that must enter the provider layer.
#[cfg(test)]
pub(crate) fn provider_token_usage(usage: &TokenUsageInfo) -> tact_llm::TokenUsageInfo {
    tact_llm::TokenUsageInfo {
        prompt: usage.prompt,
        completion: usage.completion,
        total: usage.total,
        prompt_cache_hit_tokens: usage.prompt_cache_hit_tokens,
        prompt_cache_miss_tokens: usage.prompt_cache_miss_tokens,
        reasoning_tokens: usage.reasoning_tokens,
    }
}

#[cfg(test)]
mod tests {
    fn production_source(source: &str) -> &str {
        source
            .split_once("#[cfg(test)]")
            .map_or(source, |(production, _)| production)
    }

    #[test]
    fn capability_adapters_do_not_hold_kernel_state_or_persistence_authority() {
        for (name, source) in [
            ("hook_adapter.rs", include_str!("hook_adapter.rs")),
            ("services.rs", include_str!("services.rs")),
            ("cancel_bridge.rs", include_str!("cancel_bridge.rs")),
            (
                "compaction_adapter.rs",
                include_str!("compaction_adapter.rs"),
            ),
            ("context_adapter.rs", include_str!("context_adapter.rs")),
            ("composition.rs", include_str!("composition.rs")),
            ("event_adapter.rs", include_str!("event_adapter.rs")),
            ("gate_adapter.rs", include_str!("gate_adapter.rs")),
            ("model_adapter.rs", include_str!("model_adapter.rs")),
            (
                "permission_authority.rs",
                include_str!("permission_authority.rs"),
            ),
            ("prompt_adapter.rs", include_str!("prompt_adapter.rs")),
            ("tool_adapter.rs", include_str!("tool_adapter.rs")),
            ("tool_presentation.rs", include_str!("tool_presentation.rs")),
            ("store_adapter.rs", include_str!("store_adapter.rs")),
            ("usage_adapter.rs", include_str!("usage_adapter.rs")),
        ] {
            let production = production_source(source);
            for forbidden in ["AgentState", "crate::Agent", "Arc<Mutex<Agent>>"] {
                assert!(
                    !production.contains(forbidden),
                    "{name} must not hold kernel-owned {forbidden} authority"
                );
            }
        }
    }

    #[test]
    fn policy_adapters_cannot_execute_tools_or_own_persistence_or_events() {
        for (name, source) in [
            (
                "compaction_adapter.rs",
                include_str!("compaction_adapter.rs"),
            ),
            ("context_adapter.rs", include_str!("context_adapter.rs")),
            ("gate_adapter.rs", include_str!("gate_adapter.rs")),
            ("prompt_adapter.rs", include_str!("prompt_adapter.rs")),
            ("services.rs", include_str!("services.rs")),
        ] {
            let production = production_source(source);
            for forbidden in ["ToolPort", "TranscriptStore", "UsageSink", "EventPort"] {
                assert!(
                    !production.contains(forbidden),
                    "{name} must not hold {forbidden} authority"
                );
            }
        }
    }

    #[test]
    fn persistence_adapters_cannot_execute_tools_or_emit_kernel_events() {
        for (name, source) in [
            ("store_adapter.rs", include_str!("store_adapter.rs")),
            ("usage_adapter.rs", include_str!("usage_adapter.rs")),
        ] {
            let production = production_source(source);
            for forbidden in ["AgentState", "ToolPort", "EventPort", "crate::Agent"] {
                assert!(
                    !production.contains(forbidden),
                    "{name} must not hold {forbidden} authority"
                );
            }
        }
    }

    #[test]
    fn activation_view_cannot_obtain_facade_or_persistence_authority() {
        let production = production_source(include_str!("activation_view.rs"));
        let (_, view_body) = production
            .split_once("impl<'a> TactActivationView")
            .expect("activation view implementation");
        for forbidden in [
            "crate::Agent",
            "AgentState",
            "TranscriptStore",
            "UsageSink",
            "ToolPort",
            "EventPort",
        ] {
            assert!(
                !view_body.contains(forbidden),
                "activation view must not expose {forbidden} authority"
            );
        }
    }
}
