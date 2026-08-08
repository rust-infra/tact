use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::LlmError;

/// Responses-only request fields that do not belong in the shared
/// Anthropic/Chat Completions-shaped request model.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ResponsesRequestOptions {
    pub parallel_tool_calls: Option<bool>,
    pub truncation: Option<String>,
    pub metadata: Option<Map<String, Value>>,
    pub user: Option<String>,
    pub prompt_cache_key: Option<String>,
    pub text: Option<Value>,
    pub extra: Map<String, Value>,
}

impl ResponsesRequestOptions {
    pub(crate) fn apply_to(&self, body: &mut Value) -> Result<(), LlmError> {
        let object = body.as_object_mut().ok_or_else(|| {
            LlmError::Unsupported("Responses request body must be a JSON object".to_string())
        })?;

        if let Some(value) = self.parallel_tool_calls {
            object.insert("parallel_tool_calls".to_string(), Value::Bool(value));
        }
        if let Some(value) = &self.truncation {
            object.insert("truncation".to_string(), Value::String(value.clone()));
        }
        if let Some(value) = &self.metadata {
            object.insert("metadata".to_string(), Value::Object(value.clone()));
        }
        if let Some(value) = &self.user {
            object.insert("user".to_string(), Value::String(value.clone()));
        }
        if let Some(value) = &self.prompt_cache_key {
            object.insert("prompt_cache_key".to_string(), Value::String(value.clone()));
        }
        if let Some(value) = &self.text {
            object.insert("text".to_string(), value.clone());
        }

        const RESERVED: &[&str] = &[
            "parallel_tool_calls",
            "truncation",
            "metadata",
            "user",
            "prompt_cache_key",
            "text",
            "model",
            "input",
            "instructions",
            "tools",
            "tool_choice",
            "reasoning",
            "include",
            "max_output_tokens",
            "temperature",
            "top_p",
            "store",
            "context_management",
            "stream",
        ];
        for (key, value) in &self.extra {
            if RESERVED.contains(&key.as_str()) || object.contains_key(key) {
                return Err(LlmError::Unsupported(format!(
                    "Responses request option '{key}' conflicts with an existing field"
                )));
            }
            object.insert(key.clone(), value.clone());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::ResponsesRequestOptions;

    #[test]
    fn responses_options_patch_only_populates_responses_fields() {
        let options = ResponsesRequestOptions {
            parallel_tool_calls: Some(false),
            truncation: Some("auto".into()),
            metadata: Some(serde_json::Map::from_iter([(
                "ticket".into(),
                serde_json::json!("r-1"),
            )])),
            user: Some("user-1".into()),
            prompt_cache_key: Some("cache-1".into()),
            text: Some(serde_json::json!({"format": {"type": "text"}})),
            extra: Default::default(),
        };
        let mut body = serde_json::json!({"model": "gpt-5", "input": []});
        options.apply_to(&mut body).unwrap();
        assert_eq!(body["parallel_tool_calls"], false);
        assert_eq!(body["truncation"], "auto");
        assert_eq!(body["metadata"]["ticket"], "r-1");
        assert_eq!(body["user"], "user-1");
        assert_eq!(body["prompt_cache_key"], "cache-1");
        assert_eq!(body["text"]["format"]["type"], "text");
    }

    #[test]
    fn responses_options_rejects_extra_collision() {
        let options = ResponsesRequestOptions {
            extra: serde_json::Map::from_iter([("text".into(), serde_json::json!("bad"))]),
            ..Default::default()
        };
        let mut body = serde_json::json!({});
        let error = options.apply_to(&mut body).unwrap_err().to_string();
        assert!(error.contains("Responses request option 'text'"));
    }
}
