pub use crate::types::ResponsesRequestOptions;

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
