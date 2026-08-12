//! Credential types and the async credential resolution seam.
//!
//! Provider configuration ([`crate::ProviderProfile`]) deliberately carries no
//! secrets. A [`CredentialProvider`] resolves credentials lazily at request
//! time, which leaves room for future browser-OAuth flows that must refresh
//! expiring tokens.

use std::sync::Arc;

use futures_util::future::BoxFuture;
use secrecy::SecretString;

use crate::LlmError;

/// A resolved credential, kept opaque so HTTP layers decide how to encode it.
#[derive(Debug, Clone)]
pub enum Credential {
    /// API key sent as `Authorization: Bearer <key>`.
    ApiKey(SecretString),
    /// Arbitrary bearer token (e.g. an OAuth access token).
    Bearer(SecretString),
    /// Explicit no-credential state for anonymous / local endpoints.
    None,
}

/// Resolves credentials for a provider at request time.
///
/// Implementations may cache, refresh, or open a browser to authorize;
/// adapters only ever call [`resolve`](CredentialProvider::resolve) and use
/// the returned secret.
pub trait CredentialProvider: Send + Sync + std::fmt::Debug {
    /// Resolve the credential that should be sent with the next request.
    ///
    /// # Errors
    ///
    /// Returns [`LlmError::Auth`] when no credential is available or the
    /// refresh / authorization flow fails. The error must not contain the
    /// secret itself.
    fn resolve(&self) -> BoxFuture<'_, Result<SecretString, LlmError>>;

    /// Short human-readable label used in diagnostics.
    fn describe(&self) -> &'static str {
        "credential"
    }
}

/// Static API-key credential provider, the current default implementation.
#[derive(Debug, Clone)]
pub struct ApiKeyProvider {
    api_key: Arc<SecretString>,
}

impl ApiKeyProvider {
    /// Wrap a static API key in a shared [`SecretString`].
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: Arc::new(SecretString::from(api_key.into())),
        }
    }
}

impl CredentialProvider for ApiKeyProvider {
    fn resolve(&self) -> BoxFuture<'_, Result<SecretString, LlmError>> {
        Box::pin(async move { Ok((*self.api_key).clone()) })
    }

    fn describe(&self) -> &'static str {
        "api key"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use secrecy::ExposeSecret;

    #[tokio::test]
    async fn api_key_provider_resolves_the_configured_key() {
        let provider = ApiKeyProvider::new("sk-test");
        let key = provider.resolve().await.expect("resolve must succeed");
        assert_eq!(key.expose_secret().as_str(), "sk-test");
        assert_eq!(provider.describe(), "api key");
    }
}
