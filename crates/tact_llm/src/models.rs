//! OpenAI-compatible `GET {base_url}/models` for `/model` picker supplement.

use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use crate::provider::read_provider;
use crate::types::ProviderKind;

static CLIENT: LazyLock<reqwest::Client> = LazyLock::new(reqwest::Client::new);

struct ModelsCache {
    base_url: String,
    api_key: String,
    ids: Vec<String>,
}

static CACHE: Mutex<Option<ModelsCache>> = Mutex::new(None);

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

pub fn is_models_query_supported() -> bool {
    read_provider(|p| {
        matches!(
            p.provider,
            ProviderKind::OpenAi | ProviderKind::DeepSeek | ProviderKind::Kimi
        )
    })
}

/// Session-cached API model ids for the active provider.
/// Soft-fails to empty. Skips HTTP when unsupported.
pub async fn ensure_api_model_ids() -> Vec<String> {
    if !is_models_query_supported() {
        return Vec::new();
    }
    let (base_url, api_key) = read_provider(|p| (p.base_url.clone(), p.api_key.clone()));
    {
        let guard = CACHE.lock().expect("models cache poisoned");
        if let Some(c) = guard.as_ref()
            && c.base_url == base_url
            && c.api_key == api_key
        {
            return c.ids.clone();
        }
    }
    let ids = fetch_model_ids(&base_url, &api_key).await;
    let mut guard = CACHE.lock().expect("models cache poisoned");
    *guard = Some(ModelsCache {
        base_url,
        api_key,
        ids: ids.clone(),
    });
    ids
}

/// Clear the process models cache (tests / harnesses).
pub fn clear_models_cache_for_tests() {
    *CACHE.lock().expect("models cache poisoned") = None;
}

/// Seed the process models cache (tests / harnesses).
pub fn seed_models_cache_for_tests(base_url: &str, api_key: &str, ids: Vec<String>) {
    *CACHE.lock().expect("models cache poisoned") = Some(ModelsCache {
        base_url: base_url.to_string(),
        api_key: api_key.to_string(),
        ids,
    });
}

async fn fetch_model_ids(base_url: &str, api_key: &str) -> Vec<String> {
    let url = models_url_from_base_url(base_url);
    let resp = match CLIENT
        .get(&url)
        .header("Authorization", format!("Bearer {api_key}"))
        .timeout(Duration::from_millis(5000))
        .send()
        .await
    {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };
    if !resp.status().is_success() {
        return Vec::new();
    }
    let body = match resp.text().await {
        Ok(b) => b,
        Err(_) => return Vec::new(),
    };
    parse_models_response(&body).unwrap_or_default()
}

#[cfg(test)]
fn cached_api_model_ids_for_tests(base_url: &str, api_key: &str) -> Option<Vec<String>> {
    let guard = CACHE.lock().expect("models cache poisoned");
    guard.as_ref().and_then(|c| {
        if c.base_url == base_url && c.api_key == api_key {
            Some(c.ids.clone())
        } else {
            None
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::lock_provider_for_tests;
    use crate::{ProviderInfo, ProviderKind};

    fn init_provider_for_test(kind: ProviderKind, base_url: &str) {
        crate::init_provider(ProviderInfo {
            provider: kind,
            protocol: crate::OpenAiProtocol::default(),
            reasoning_effort: None,
            api_key: "sk-test".into(),
            base_url: base_url.into(),
            model: "test-model".into(),
        });
    }

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

    #[test]
    fn models_query_supported_for_openai_compat_not_anthropic() {
        let _guard = lock_provider_for_tests();
        init_provider_for_test(ProviderKind::OpenAi, "https://api.openai.com/v1");
        assert!(is_models_query_supported());
        init_provider_for_test(ProviderKind::DeepSeek, "https://api.deepseek.com/v1");
        assert!(is_models_query_supported());
        init_provider_for_test(ProviderKind::Kimi, "https://api.moonshot.cn/v1");
        assert!(is_models_query_supported());
        init_provider_for_test(ProviderKind::Anthropic, "https://api.anthropic.com");
        assert!(!is_models_query_supported());
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)] // test mutex serializes provider+cache only
    async fn ensure_api_model_ids_soft_fails_and_caches_empty() {
        let _guard = lock_provider_for_tests();
        clear_models_cache_for_tests();
        // Closed port → connection error; soft-fail to empty, then cached.
        init_provider_for_test(ProviderKind::OpenAi, "http://127.0.0.1:1/v1");
        let first = ensure_api_model_ids().await;
        assert!(first.is_empty());
        let second = ensure_api_model_ids().await;
        assert!(second.is_empty());
        // Same key still cached (no panic / still empty).
        assert_eq!(
            cached_api_model_ids_for_tests("http://127.0.0.1:1/v1", "sk-test").as_deref(),
            Some(&[][..])
        );
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)] // test mutex serializes provider+cache only
    async fn ensure_refetches_when_base_url_changes() {
        let _guard = lock_provider_for_tests();
        clear_models_cache_for_tests();
        seed_models_cache_for_tests("https://a.example/v1", "sk-test", vec!["from-a".into()]);
        init_provider_for_test(ProviderKind::OpenAi, "https://a.example/v1");
        assert_eq!(ensure_api_model_ids().await, vec!["from-a".to_string()]);

        // Different base_url → cache miss → soft-fail empty (unreachable host).
        init_provider_for_test(ProviderKind::OpenAi, "http://127.0.0.1:1/v1");
        assert!(ensure_api_model_ids().await.is_empty());
    }
}
