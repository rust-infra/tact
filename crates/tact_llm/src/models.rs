//! OpenAI-compatible `GET {base_url}/models` for `/model` picker supplement.

pub fn models_url_from_base_url(base_url: &str) -> String {
    format!("{}/models", base_url.trim_end_matches('/'))
}

pub fn parse_models_response(body: &str) -> anyhow::Result<Vec<String>> {
    #[derive(serde::Deserialize)]
    struct ModelEntry {
        id: String,
    }
    #[derive(serde::Deserialize)]
    struct ModelsResponse {
        data: Vec<ModelEntry>,
    }
    let raw: ModelsResponse = serde_json::from_str(body)?;
    Ok(raw.data.into_iter().map(|e| e.id).collect())
}

pub fn merge_model_candidates(config: &[String], api: &[String]) -> Vec<String> {
    let mut out = config.to_vec();
    for id in api {
        if !out.iter().any(|c| c == id) {
            out.push(id.clone());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn models_url_joins_base() {
        assert_eq!(
            models_url_from_base_url("https://api.openai.com/v1"),
            "https://api.openai.com/v1/models"
        );
        assert_eq!(
            models_url_from_base_url("https://api.openai.com/v1/"),
            "https://api.openai.com/v1/models"
        );
    }

    #[test]
    fn parse_models_response_reads_ids_ignores_extra() {
        let body = r#"{
            "object": "list",
            "data": [
                {"id": "gpt-4o", "object": "model", "owned_by": "openai"},
                {"id": "o3-mini", "extra": true}
            ]
        }"#;
        assert_eq!(
            parse_models_response(body).unwrap(),
            vec!["gpt-4o".to_string(), "o3-mini".to_string()]
        );
    }

    #[test]
    fn merge_config_primary_api_supplement() {
        let config = vec!["a".into(), "b".into()];
        let api = vec!["b".into(), "c".into()];
        assert_eq!(
            merge_model_candidates(&config, &api),
            vec!["a".to_string(), "b".to_string(), "c".to_string()]
        );
    }

    #[test]
    fn merge_api_only_and_empty() {
        assert_eq!(
            merge_model_candidates(&[], &["x".into(), "y".into()]),
            vec!["x".to_string(), "y".to_string()]
        );
        assert!(merge_model_candidates(&[], &[]).is_empty());
        assert_eq!(
            merge_model_candidates(&["a".into()], &[]),
            vec!["a".to_string()]
        );
    }
}
