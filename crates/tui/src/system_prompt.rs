use std::collections::BTreeSet;
use std::sync::LazyLock;

use anyhow::{Result, anyhow};
use regex::Regex;
use serde_json::Value;

/// `skill://<server>/<skill>/SKILL.md` path mentioned inside a tool description.
///
/// Regexes cannot look around, so the character class enumerates the
/// delimiters MCP servers put after a path (punctuation, quotes, backticks,
/// brackets) instead of trimming a trailing run.
static SKILL_PATH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"skill://[^\s`'"()\[\]<>,;|]+"#).expect("valid MCP skill path regex")
});

/// Extract the system instructions from a persisted provider request body.
pub(crate) fn extract_system_prompt(request_body: &[u8]) -> Result<String> {
    let value: Value = serde_json::from_slice(request_body)
        .map_err(|err| anyhow!("invalid persisted request JSON: {err}"))?;

    if let Some(instructions) = value.get("instructions").and_then(Value::as_str) {
        return Ok(instructions.to_owned());
    }

    let Some(messages) = value.get("messages").and_then(Value::as_array) else {
        return Err(anyhow!("request contains no system prompt"));
    };
    for message in messages {
        if message.get("role").and_then(Value::as_str) != Some("system") {
            continue;
        }
        if let Some(content) = message.get("content").and_then(Value::as_str) {
            return Ok(content.to_owned());
        }
        if let Some(parts) = message.get("content").and_then(Value::as_array) {
            let text = parts
                .iter()
                .filter_map(|part| {
                    part.get("text")
                        .and_then(Value::as_str)
                        .or_else(|| part.get("content").and_then(Value::as_str))
                })
                .collect::<Vec<_>>()
                .join("\n");
            if !text.is_empty() {
                return Ok(text);
            }
        }
    }
    Err(anyhow!("request contains no system prompt"))
}

/// MCP skill paths advertised by this request's tool descriptions.
///
/// MCP servers point at their skills from inside their tool descriptions
/// (Figma: "prefer the /figma-use skill if available, otherwise read
/// `skill://figma/figma-use/SKILL.md`"). Tact forwards those descriptions
/// verbatim and MCP contributes nothing to `# Available skills` (that list is
/// disk-only), so the persisted request body is the only record of which MCP
/// skills a request carried.
pub(crate) fn extract_mcp_skill_paths(request_body: &[u8]) -> Vec<String> {
    let Ok(value) = serde_json::from_slice::<Value>(request_body) else {
        return Vec::new();
    };
    let Some(tools) = value.get("tools").and_then(Value::as_array) else {
        return Vec::new();
    };

    // Only the tool definitions are scanned: the same URI can appear in the
    // conversation (a tool result echoing a description), which is not the
    // request advertising a skill.
    let mut paths = BTreeSet::new();
    for tool in tools {
        let Ok(text) = serde_json::to_string(tool) else {
            continue;
        };
        for found in SKILL_PATH.find_iter(&text) {
            paths.insert(found.as_str().to_string());
        }
    }
    paths.into_iter().collect()
}

/// Popup view for "Assembled current prompt": the system prompt exactly as the
/// provider received it, followed by the MCP skill paths that same request
/// carried in its tool descriptions.
pub(crate) fn assemble_prompt_view(request_body: &[u8]) -> Result<String> {
    let prompt = extract_system_prompt(request_body)?;
    let paths = extract_mcp_skill_paths(request_body);
    if paths.is_empty() {
        return Ok(prompt);
    }

    let mut view = prompt;
    view.push_str(
        "\n\n## MCP skills\n\nAdvertised inside this request's MCP tool descriptions:\n\n",
    );
    for path in paths {
        view.push_str("- ");
        view.push_str(&path);
        view.push('\n');
    }
    Ok(view)
}

#[cfg(test)]
mod tests {
    use super::{assemble_prompt_view, extract_mcp_skill_paths, extract_system_prompt};

    /// Responses-shaped body with Figma-style MCP tool descriptions.
    fn body_with_mcp_skills() -> Vec<u8> {
        serde_json::json!({
            "instructions": "# Your role\n\nagent",
            "tools": [
                {
                    "type": "function",
                    "name": "mcp__figma__use_figma",
                    "description": "IMPORTANT: Before calling this tool, load figma-use guidance — \
                                    prefer the /figma-use skill if available, otherwise read the \
                                    `skill://figma/figma-use/SKILL.md` with resources/read."
                },
                {
                    "type": "function",
                    "name": "mcp__figma__create_shader",
                    "description": "You MUST load the figma-shaders skill before calling this tool. \
                                    If it is not installed, read skill://figma/figma-shaders/SKILL.md \
                                    with resources/read or get_figma_skill"
                },
                {
                    "type": "function",
                    "name": "mcp__figma__update_shader",
                    "description": "Read skill://figma/figma-shaders/SKILL.md first."
                },
                { "type": "function", "name": "bash", "description": "Run a shell command." }
            ]
        })
        .to_string()
        .into_bytes()
    }

    #[test]
    fn extracts_responses_instructions() {
        assert_eq!(
            extract_system_prompt(br#"{"instructions":"assembled"}"#).unwrap(),
            "assembled"
        );
    }

    #[test]
    fn extracts_chat_system_message() {
        assert_eq!(
            extract_system_prompt(br#"{"messages":[{"role":"system","content":"assembled"}]}"#)
                .unwrap(),
            "assembled"
        );
    }

    #[test]
    fn extracts_structured_system_content() {
        let body = br#"{"messages":[{"role":"system","content":[{"type":"text","text":"one"},{"type":"text","text":"two"}]}]}"#;
        assert_eq!(extract_system_prompt(body).unwrap(), "one\ntwo");
    }

    #[test]
    fn rejects_invalid_or_missing_prompt() {
        assert!(extract_system_prompt(b"not json").is_err());
        assert!(extract_system_prompt(br#"{"model":"x"}"#).is_err());
    }

    #[test]
    fn mcp_skill_paths_are_deduped_and_sorted() {
        assert_eq!(
            extract_mcp_skill_paths(&body_with_mcp_skills()),
            vec![
                "skill://figma/figma-shaders/SKILL.md".to_string(),
                "skill://figma/figma-use/SKILL.md".to_string(),
            ]
        );
    }

    /// A `skill://` string in the conversation is not a skill the request
    /// advertises — only tool definitions count.
    #[test]
    fn mcp_skill_paths_ignore_non_tool_text() {
        let body = serde_json::json!({
            "instructions": "read skill://figma/figma-use/SKILL.md",
            "input": [{ "role": "user", "content": "see skill://figma/other/SKILL.md" }],
            "tools": [{ "type": "function", "name": "bash", "description": "Run a shell." }]
        })
        .to_string()
        .into_bytes();

        assert!(extract_mcp_skill_paths(&body).is_empty());
        assert_eq!(
            assemble_prompt_view(&body).unwrap(),
            "read skill://figma/figma-use/SKILL.md"
        );
    }

    #[test]
    fn assembled_view_keeps_the_prompt_and_appends_mcp_skills() {
        let body = body_with_mcp_skills();
        assert_eq!(
            extract_system_prompt(&body).unwrap(),
            "# Your role\n\nagent",
            "the prompt itself must stay verbatim"
        );

        let view = assemble_prompt_view(&body).unwrap();
        assert!(view.starts_with("# Your role\n\nagent\n\n## MCP skills"));
        assert!(view.contains("- skill://figma/figma-shaders/SKILL.md\n"));
        assert!(view.contains("- skill://figma/figma-use/SKILL.md\n"));
        assert_eq!(view.matches("skill://figma/figma-shaders").count(), 1);
    }
}
