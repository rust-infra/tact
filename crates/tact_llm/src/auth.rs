//! Credential types and the async credential resolution seam.
//!
//! Provider configuration ([`crate::ProviderProfile`]) deliberately carries no
//! secrets. A [`CredentialProvider`] resolves credentials lazily at request
//! time, which leaves room for future browser-OAuth flows that must refresh
//! expiring tokens.

use std::sync::Arc;

use futures_util::future::BoxFuture;
use secrecy::{ExposeSecret, SecretString};

use crate::error::LlmError;

/// Legacy credential shape retained for the product compatibility surface.
#[derive(Debug, Clone)]
pub enum LegacyCredential {
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

    /// Resolve the credential while preserving whether it is an API key, a
    /// bearer token, or an explicit anonymous value. Legacy providers default
    /// to an API key because that was their only supported shape.
    fn resolve_with_kind(&self) -> BoxFuture<'_, Result<LegacyCredential, LlmError>> {
        Box::pin(async move { self.resolve().await.map(LegacyCredential::ApiKey) })
    }

    /// Return a static API key when this provider is explicitly static.
    fn static_api_key(&self) -> Option<&str> {
        None
    }

    /// Short human-readable label used in diagnostics.
    fn describe(&self) -> &'static str {
        "credential"
    }
}

/// How a resolved stable credential should be encoded by a provider adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialKind {
    ApiKey,
    Bearer,
    None,
}

/// Stable, opaque credential value. Its debug output never contains the secret.
#[derive(Clone)]
pub struct Credential {
    kind: CredentialKind,
    value: String,
}

impl std::fmt::Debug for Credential {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Credential")
            .field("kind", &self.kind)
            .field("value", &"<redacted>")
            .finish()
    }
}

impl Credential {
    pub fn api_key(value: impl Into<String>) -> Result<Self, CredentialError> {
        Self::validated(CredentialKind::ApiKey, value.into())
    }

    pub fn bearer(value: impl Into<String>) -> Result<Self, CredentialError> {
        Self::validated(CredentialKind::Bearer, value.into())
    }

    pub const fn none() -> Self {
        Self {
            kind: CredentialKind::None,
            value: String::new(),
        }
    }

    pub(crate) fn kind(&self) -> CredentialKind {
        self.kind
    }

    pub(crate) fn secret(&self) -> Option<&str> {
        (self.kind != CredentialKind::None).then_some(self.value.as_str())
    }

    fn validated(kind: CredentialKind, value: String) -> Result<Self, CredentialError> {
        if value.is_empty() {
            return Err(CredentialError::Empty);
        }
        if value.chars().any(|character| character.is_control()) {
            return Err(CredentialError::ControlCharacter);
        }
        if !value.bytes().all(|byte| (0x21..=0x7e).contains(&byte)) {
            return Err(CredentialError::InvalidHeaderValue);
        }
        Ok(Self { kind, value })
    }
}

/// Resolves a stable credential at dispatch time.
pub trait CredentialSource: Send + Sync + std::fmt::Debug {
    fn resolve(&self) -> BoxFuture<'_, Result<Credential, CredentialError>>;
}

/// Adapt the legacy credential provider to the stable source contract.
#[derive(Debug)]
pub struct LegacyCredentialSource {
    provider: Arc<dyn CredentialProvider>,
}

impl CredentialSource for LegacyCredentialSource {
    fn resolve(&self) -> BoxFuture<'_, Result<Credential, CredentialError>> {
        Box::pin(async move {
            let credential = self
                .provider
                .resolve_with_kind()
                .await
                .map_err(|_| CredentialError::Unavailable)?;
            match credential {
                LegacyCredential::ApiKey(secret) => {
                    Credential::api_key(secret.expose_secret().clone())
                }
                LegacyCredential::Bearer(secret) => {
                    Credential::bearer(secret.expose_secret().clone())
                }
                LegacyCredential::None => Ok(Credential::none()),
            }
        })
    }
}

pub fn legacy_credential_source(
    provider: Arc<dyn CredentialProvider>,
) -> Arc<dyn CredentialSource> {
    Arc::new(LegacyCredentialSource { provider })
}

/// Stable authentication configuration.
#[derive(Clone)]
pub enum Authentication {
    Static(Credential),
    Refreshable(Arc<dyn CredentialSource>),
    None,
}

/// Stable credential validation and refresh failures.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CredentialError {
    #[error("credential is empty")]
    Empty,
    #[error("credential contains a control character")]
    ControlCharacter,
    #[error("credential is not a valid HTTP header value")]
    InvalidHeaderValue,
    #[error("credential is unavailable")]
    Unavailable,
    #[error("credential refresh failed")]
    RefreshFailed,
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

    fn static_api_key(&self) -> Option<&str> {
        Some(self.api_key.expose_secret())
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
