use crate::render::util::{LOG_THINKING_INDENT, LOG_TOOL_INDENT};
use crate::widgets::state::RawMessageType;

fn is_plan_step_line(raw: &str) -> bool {
    raw.strip_prefix("  ")
        .and_then(|rest| {
            let (num, after) = rest.split_once(". ")?;
            (!num.is_empty() && num.chars().all(|c| c.is_ascii_digit()) && !after.is_empty())
                .then_some(())
        })
        .is_some()
}

/// Classify plain-text system / info rows for indent and styling.
pub(crate) fn classify_system_message(raw: &str) -> RawMessageType {
    let raw = raw.trim_end();

    if raw.starts_with('▶')
        || raw.starts_with("Executing ")
        || raw.starts_with("Error invoking tool ")
        || (raw.starts_with('⚠') && (raw.contains("Need approval:") || raw.contains("需要审批:")))
        || (raw.starts_with("Generated ") && raw.contains(" steps:"))
        || (raw.starts_with("生成了 ") && raw.contains("个步骤"))
        || is_plan_step_line(raw)
    {
        return RawMessageType::SysTool;
    }

    if raw.starts_with('✓') || raw.starts_with('✗') || raw.starts_with('✔') {
        if raw.contains("Step ")
            || raw.contains("步骤 ")
            || raw.contains("Selected:")
            || raw.contains("已选择:")
            || raw.contains("Step approved")
            || raw.contains("步骤已批准")
            || raw.contains("Step rejected")
            || raw.contains("步骤已拒绝")
        {
            return RawMessageType::SysTool;
        }
    }

    RawMessageType::LLM
}

impl RawMessageType {
    pub(crate) fn log_indent(self) -> u16 {
        match self {
            Self::LLM => 0,
            Self::LLMThinking => LOG_THINKING_INDENT,
            Self::SysTool => LOG_TOOL_INDENT,
        }
    }
}
