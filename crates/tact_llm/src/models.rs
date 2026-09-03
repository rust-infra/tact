//! OpenAI-compatible `GET {base_url}/models` for `/model` picker supplement.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use secrecy::{ExposeSecret, SecretString};

use crate::provider::{ProviderInfo, get_provider, try_active_credential_provider};
use crate::types::ProviderKind;
use crate::{ApiKeyProvider, CredentialProvider, ProviderProfile, SharedHttpClient};

struct ModelsCache {
    base_url: String,
    api_key: SecretString,
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

fn provider_supports_models_query(provider: &ProviderKind) -> bool {
    matches!(
        provider,
        ProviderKind::OpenAi
            | ProviderKind::DeepSeek
            | ProviderKind::Kimi
            | ProviderKind::Custom(_)
    )
}

pub fn is_models_query_supported() -> bool {
    provider_supports_models_query(&get_provider().provider)
}

/// Session-cached API model ids for the given provider.
/// Soft-fails to empty. Skips HTTP when unsupported.
pub async fn ensure_api_model_ids_for_provider(provider: &ProviderInfo) -> Vec<String> {
    let profile = provider.to_profile();
    let credentials = credentials_for_provider(provider);
    ensure_api_model_ids_for(&profile, credentials.as_ref(), &SharedHttpClient::default()).await
}

/// Uses the provider's explicit API key when present, otherwise falls back to
/// the process-global credential provider (e.g. browser-OAuth in the future).
fn credentials_for_provider(provider: &ProviderInfo) -> Arc<dyn CredentialProvider> {
    if provider.api_key.is_empty() {
        try_active_credential_provider()
            .unwrap_or_else(|| Arc::new(ApiKeyProvider::new(String::new())))
    } else {
        Arc::new(ApiKeyProvider::new(provider.api_key.clone()))
    }
}

/// Session-cached API model ids for the active provider.
/// Soft-fails to empty. Skips HTTP when unsupported.
pub async fn ensure_api_model_ids() -> Vec<String> {
    let provider = get_provider();
    ensure_api_model_ids_for_provider(&provider).await
}

/// Session-cached API model ids for an explicit profile, credential, and
/// transport. Soft-fails to empty. Skips HTTP when unsupported.
pub async fn ensure_api_model_ids_for(
    profile: &ProviderProfile,
    credentials: &dyn CredentialProvider,
    http: &SharedHttpClient,
) -> Vec<String> {
    if !provider_supports_models_query(&profile.provider) {
        return Vec::new();
    }
    let Ok(secret) = credentials.resolve().await else {
        return Vec::new();
    };
    ensure_api_model_ids_for_credentials(&profile.base_url, &secret, http).await
}

async fn ensure_api_model_ids_for_credentials(
    base_url: &str,
    secret: &SecretString,
    http: &SharedHttpClient,
) -> Vec<String> {
    {
        let guard = CACHE.lock().expect("models cache poisoned");
        if let Some(c) = guard.as_ref()
            && c.base_url == base_url
            && c.api_key.expose_secret().as_str() == secret.expose_secret().as_str()
        {
            return c.ids.clone();
        }
    }
    let ids = fetch_model_ids(base_url, secret.expose_secret(), http).await;
    let mut guard = CACHE.lock().expect("models cache poisoned");
    *guard = Some(ModelsCache {
        base_url: base_url.to_string(),
        api_key: secret.clone(),
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
        api_key: SecretString::from(api_key.to_string()),
        ids,
    });
}

async fn fetch_model_ids(base_url: &str, api_key: &str, http: &SharedHttpClient) -> Vec<String> {
    let url = models_url_from_base_url(base_url);
    let resp = match http
        .inner()
        .get(&url)
        .header("Authorization", format!("Bearer {api_key}"))
        .headers(crate::opencode::endpoint_headers(base_url, None))
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
        if c.base_url == base_url && c.api_key.expose_secret().as_str() == api_key {
            Some(c.ids.clone())
        } else {
            None
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use futures_util::future::BoxFuture;

    use crate::provider::lock_provider_for_tests;
    use crate::{LlmError, ProviderInfo, ProviderKind};

    #[derive(Debug)]
    struct CountingCredential {
        key: Arc<SecretString>,
        calls: Arc<AtomicUsize>,
    }

    impl CountingCredential {
        fn new(key: &str) -> Self {
            Self {
                key: Arc::new(SecretString::from(key.to_string())),
                calls: Arc::new(AtomicUsize::new(0)),
            }
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::Relaxed)
        }
    }

    impl CredentialProvider for CountingCredential {
        fn resolve(&self) -> BoxFuture<'_, Result<SecretString, LlmError>> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            let key = self.key.clone();
            Box::pin(async move { Ok((*key).clone()) })
        }
    }

    #[derive(Debug)]
    struct FailingCredential;

    impl CredentialProvider for FailingCredential {
        fn resolve(&self) -> BoxFuture<'_, Result<SecretString, LlmError>> {
            Box::pin(async { Err(LlmError::Auth("test credential unavailable".to_string())) })
        }
    }

    fn openai_profile(base_url: &str) -> ProviderProfile {
        ProviderProfile {
            provider: ProviderKind::OpenAi,
            base_url: base_url.to_string(),
            model: "test-model".to_string(),
            protocol: crate::OpenAiProtocol::default(),
            responses_compact_threshold: None,
        }
    }

    fn init_provider_for_test(kind: ProviderKind, base_url: &str) {
        crate::init_provider(ProviderInfo {
            provider: kind,
            protocol: crate::OpenAiProtocol::default(),
            responses_compact_threshold: None,
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
    #[allow(clippy::await_holding_lock)]
    async fn explicit_provider_model_query_uses_subagent_credentials() {
        let _guard = lock_provider_for_tests();
        clear_models_cache_for_tests();
        init_provider_for_test(ProviderKind::OpenAi, "https://main.example/v1");
        seed_models_cache_for_tests(
            "https://subagent.example/v1",
            "sk-subagent",
            vec!["subagent-model".into()],
        );

        let subagent = ProviderInfo {
            api_key: "sk-subagent".into(),
            base_url: "https://subagent.example/v1".into(),
            model: "subagent-model".into(),
            provider: ProviderKind::DeepSeek,
            protocol: crate::OpenAiProtocol::default(),
            responses_compact_threshold: None,
        };

        assert_eq!(
            ensure_api_model_ids_for_provider(&subagent).await,
            vec!["subagent-model".to_string()]
        );
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)] // test mutex serializes provider+cache only
    async fn provider_without_key_uses_global_credentials() {
        let _guard = lock_provider_for_tests();
        clear_models_cache_for_tests();
        crate::init_provider_with_credentials(
            ProviderInfo {
                api_key: String::new(),
                base_url: "https://main.example/v1".into(),
                model: "test-model".into(),
                provider: ProviderKind::OpenAi,
                protocol: crate::OpenAiProtocol::default(),
                responses_compact_threshold: None,
            },
            Arc::new(CountingCredential::new("sk-global")),
        );
        seed_models_cache_for_tests(
            "https://main.example/v1",
            "sk-global",
            vec!["global-model".into()],
        );

        assert_eq!(
            ensure_api_model_ids().await,
            vec!["global-model".to_string()]
        );
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

    #[tokio::test]
    #[allow(clippy::await_holding_lock)] // test mutex serializes provider+cache only
    async fn explicit_credentials_resolve_per_request_and_hit_cache() {
        let _guard = lock_provider_for_tests();
        clear_models_cache_for_tests();
        let credentials = CountingCredential::new("sk-cache-hit");
        let profile = openai_profile("https://main.example/v1");
        seed_models_cache_for_tests(
            "https://main.example/v1",
            "sk-cache-hit",
            vec!["cached-model".into()],
        );

        assert_eq!(
            ensure_api_model_ids_for(&profile, &credentials, &SharedHttpClient::default()).await,
            vec!["cached-model".to_string()]
        );
        assert_eq!(credentials.calls(), 1);
        assert_eq!(
            ensure_api_model_ids_for(&profile, &credentials, &SharedHttpClient::default()).await,
            vec!["cached-model".to_string()]
        );
        assert_eq!(credentials.calls(), 2);
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)] // test mutex serializes provider+cache only
    async fn models_cache_distinguishes_credentials() {
        let _guard = lock_provider_for_tests();
        clear_models_cache_for_tests();
        let first = CountingCredential::new("sk-first");
        let second = CountingCredential::new("sk-second");
        let profile = openai_profile("https://same.example/v1");
        seed_models_cache_for_tests(
            "https://same.example/v1",
            "sk-first",
            vec!["first-model".into()],
        );

        assert_eq!(
            ensure_api_model_ids_for(&profile, &first, &SharedHttpClient::default()).await,
            vec!["first-model".to_string()]
        );
        // Different secret misses the cache and soft-fails on the closed port.
        assert!(
            ensure_api_model_ids_for(&profile, &second, &SharedHttpClient::default())
                .await
                .is_empty()
        );
        assert_eq!(
            cached_api_model_ids_for_tests("https://same.example/v1", "sk-second").as_deref(),
            Some(&[][..])
        );
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)] // test mutex serializes provider+cache only
    async fn models_query_soft_fails_when_credential_resolve_fails() {
        let _guard = lock_provider_for_tests();
        clear_models_cache_for_tests();
        let profile = openai_profile("https://main.example/v1");

        assert!(
            ensure_api_model_ids_for(&profile, &FailingCredential, &SharedHttpClient::default())
                .await
                .is_empty()
        );
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)] // test mutex serializes provider+cache only
    async fn models_query_skips_unsupported_provider_without_resolving() {
        let _guard = lock_provider_for_tests();
        clear_models_cache_for_tests();
        let credentials = CountingCredential::new("sk-never-resolved");
        let profile = ProviderProfile {
            provider: ProviderKind::Anthropic,
            base_url: "https://api.anthropic.com".to_string(),
            model: "claude-sonnet".to_string(),
            protocol: crate::OpenAiProtocol::default(),
            responses_compact_threshold: None,
        };

        assert!(
            ensure_api_model_ids_for(&profile, &credentials, &SharedHttpClient::default())
                .await
                .is_empty()
        );
        assert_eq!(credentials.calls(), 0);
    }
}
