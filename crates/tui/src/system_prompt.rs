use anyhow::{Result, anyhow};
use serde_json::Value;

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

#[cfg(test)]
mod tests {
    use super::extract_system_prompt;

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
}
