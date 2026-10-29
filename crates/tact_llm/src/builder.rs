//! Stable provider builder and provider facade.

use std::sync::Arc;

use async_trait::async_trait;
use futures_util::{StreamExt, future::BoxFuture};
use secrecy::SecretString;
use tact_agent_api::{
    CancelToken, ModelCapabilities, ModelCapabilitiesError, ModelCursor, ModelError,
    ModelErrorKind, ModelEvent, ModelPort, ModelRequest, ModelResponse, ModelStream,
};

#[cfg(feature = "openai-chat")]
use crate::ChatDialect;
use crate::assembly::{AssemblyError, AssemblySpec, assemble};
use crate::auth::{ApiKeyProvider, CredentialKind, CredentialProvider, LegacyCredential};
use crate::client::LlmProvider;
use crate::error::LlmError;
use crate::provider_model::ProviderModel;
use crate::provider_state::ProviderConversationState;
use crate::{
    Authentication, CreateMessageParams, Credential, CredentialError, CredentialSource, Endpoint,
    EndpointError, ModelLimits, ProviderKind, ProviderMetadata, SessionId, WireProtocol,
    neutral::{create_message_params, provider_state},
};

/// Stable provider construction failures.
#[derive(Debug, thiserror::Error)]
pub enum BuildError {
    #[error("invalid model field: {field}")]
    InvalidModel { field: &'static str },
    #[error("invalid endpoint: {0}")]
    InvalidEndpoint(EndpointError),
    #[error("invalid limits: {0}")]
    InvalidLimits(crate::limits::LimitError),
    #[error("invalid credential: {0}")]
    InvalidCredential(CredentialError),
    #[error("invalid session: {0}")]
    InvalidSession(crate::limits::SessionIdError),
    #[error("model limits are required")]
    MissingLimits,
    #[error("an endpoint is required")]
    MissingEndpoint,
    #[error("authentication is required for {provider}")]
    AuthenticationRequired { provider: ProviderKind },
    #[error("provider feature is disabled for {provider} ({protocol:?})")]
    ProviderFeatureDisabled {
        provider: ProviderKind,
        protocol: WireProtocol,
    },
    #[error("option {option} is not supported by {protocol:?}")]
    UnsupportedOption {
        option: &'static str,
        protocol: WireProtocol,
    },
    #[error("invalid capabilities: {0}")]
    InvalidCapabilities(ModelCapabilitiesError),
    #[error("transport setup failed at {stage:?}")]
    TransportSetup { stage: TransportSetupStage },
}

/// Transport construction stages exposed without provider error text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportSetupStage {
    ReqwestClient,
}

/// Collects the immutable facts needed to build a stable provider.
pub struct ProviderBuilder {
    kind: ProviderKind,
    protocol: WireProtocol,
    model: String,
    base_url: Option<String>,
    auth: Option<Authentication>,
    session_id: Option<SessionId>,
    limits: Option<ModelLimits>,
    #[cfg(feature = "openai-chat")]
    chat_dialect: Option<ChatDialect>,
    native_compaction: Option<bool>,
    responses_compact_threshold: Option<u32>,
    supports_vision: Option<bool>,
    replay_prior_reasoning: Option<bool>,
}

impl ProviderBuilder {
    fn new(kind: ProviderKind, protocol: WireProtocol, model: impl Into<String>) -> Self {
        Self {
            kind,
            protocol,
            model: model.into(),
            base_url: None,
            auth: None,
            session_id: None,
            limits: None,
            #[cfg(feature = "openai-chat")]
            chat_dialect: None,
            native_compaction: None,
            responses_compact_threshold: None,
            supports_vision: None,
            replay_prior_reasoning: None,
        }
    }

    pub fn authentication(mut self, auth: Authentication) -> Self {
        self.auth = Some(auth);
        self
    }

    pub fn api_key(self, value: impl Into<String>) -> Result<Self, CredentialError> {
        Ok(self.authentication(Authentication::Static(Credential::api_key(value)?)))
    }

    pub fn bearer(self, value: impl Into<String>) -> Result<Self, CredentialError> {
        Ok(self.authentication(Authentication::Static(Credential::bearer(value)?)))
    }

    pub fn refreshable(mut self, source: Arc<dyn CredentialSource>) -> Self {
        self.auth = Some(Authentication::Refreshable(source));
        self
    }

    pub fn anonymous(mut self) -> Self {
        self.auth = Some(Authentication::None);
        self
    }

    pub fn base_url(mut self, value: impl Into<String>) -> Self {
        self.base_url = Some(value.into());
        self
    }

    pub fn session_id(mut self, session_id: SessionId) -> Self {
        self.session_id = Some(session_id);
        self
    }

    pub fn limits(mut self, limits: ModelLimits) -> Self {
        self.limits = Some(limits);
        self
    }

    #[cfg(feature = "openai-chat")]
    pub fn chat_dialect(mut self, dialect: ChatDialect) -> Self {
        self.chat_dialect = Some(dialect);
        self
    }

    pub fn native_compaction(mut self, enabled: bool) -> Self {
        self.native_compaction = Some(enabled);
        self
    }

    pub fn responses_compact_threshold(mut self, tokens: u32) -> Self {
        self.responses_compact_threshold = Some(tokens);
        self
    }

    pub fn supports_vision(mut self, enabled: bool) -> Self {
        self.supports_vision = Some(enabled);
        self
    }

    pub fn replay_prior_reasoning(mut self, enabled: bool) -> Self {
        self.replay_prior_reasoning = Some(enabled);
        self
    }

    pub fn build(self) -> Result<Provider, BuildError> {
        self.build_with_capture(false)
    }

    #[cfg(feature = "product-runtime")]
    pub fn build_product(self) -> Result<Provider, BuildError> {
        self.build_with_capture(true)
    }

    fn build_with_capture(self, product_capture: bool) -> Result<Provider, BuildError> {
        if self.model.trim().is_empty() {
            return Err(BuildError::InvalidModel { field: "model" });
        }
        let limits = self.limits.ok_or(BuildError::MissingLimits)?;
        let policy = crate::policy::ProviderPolicy::for_target(&self.kind, self.protocol);
        let raw_endpoint = self
            .base_url
            .or_else(|| policy.default_endpoint().map(str::to_string))
            .ok_or(BuildError::MissingEndpoint)?;
        let endpoint = Endpoint::parse(&raw_endpoint).map_err(BuildError::InvalidEndpoint)?;
        let auth = match self.auth {
            Some(auth) => auth,
            None if is_builtin(&self.kind) => {
                return Err(BuildError::AuthenticationRequired {
                    provider: self.kind,
                });
            }
            None => Authentication::None,
        };
        if is_builtin(&self.kind) && matches!(auth, Authentication::None) {
            return Err(BuildError::AuthenticationRequired {
                provider: self.kind,
            });
        }
        if matches!(&auth, Authentication::Static(credential) if credential.kind() == CredentialKind::None)
            && is_builtin(&self.kind)
        {
            return Err(BuildError::AuthenticationRequired {
                provider: self.kind,
            });
        }
        #[cfg(feature = "openai-chat")]
        if self.chat_dialect.is_some() && self.protocol != WireProtocol::OpenAiChatCompletions {
            return Err(BuildError::UnsupportedOption {
                option: "chat_dialect",
                protocol: self.protocol,
            });
        }
        if self.responses_compact_threshold.is_some()
            && self.protocol != WireProtocol::OpenAiResponses
        {
            return Err(BuildError::UnsupportedOption {
                option: "responses_compact_threshold",
                protocol: self.protocol,
            });
        }
        if self.responses_compact_threshold == Some(0) {
            return Err(BuildError::InvalidModel {
                field: "responses_compact_threshold",
            });
        }
        if self.replay_prior_reasoning.is_some() && self.protocol != WireProtocol::OpenAiResponses {
            return Err(BuildError::UnsupportedOption {
                option: "replay_prior_reasoning",
                protocol: self.protocol,
            });
        }
        let default_native = policy.native_compaction();
        let native_compaction = self.native_compaction.unwrap_or(default_native);
        if native_compaction && self.protocol != WireProtocol::OpenAiResponses {
            return Err(BuildError::UnsupportedOption {
                option: "native_compaction",
                protocol: self.protocol,
            });
        }
        let capabilities = ModelCapabilities::new(
            self.model.clone(),
            limits.context_window(),
            limits.max_output_tokens(),
            true,
            native_compaction,
        )
        .map_err(BuildError::InvalidCapabilities)?;
        let supports_vision = self
            .supports_vision
            .unwrap_or_else(|| policy.supports_vision());
        let spec = ProviderSpec {
            kind: self.kind,
            protocol: self.protocol,
            model: self.model,
            endpoint,
            auth,
            session_id: self.session_id,
            limits,
            #[cfg(feature = "openai-chat")]
            chat_dialect: self.chat_dialect,
            native_compaction,
            responses_compact_threshold: self.responses_compact_threshold,
            supports_vision,
            replay_prior_reasoning: self.replay_prior_reasoning,
            product_capture,
            capabilities,
        };
        Provider::from_spec(spec)
    }
}

/// Build an Anthropic provider.
#[cfg(feature = "anthropic")]
pub fn anthropic(model: impl Into<String>) -> ProviderBuilder {
    ProviderBuilder::new(
        ProviderKind::Anthropic,
        WireProtocol::AnthropicMessages,
        model,
    )
}

/// Build an OpenAI Chat Completions provider.
#[cfg(feature = "openai-chat")]
pub fn openai_chat(model: impl Into<String>) -> ProviderBuilder {
    ProviderBuilder::new(
        ProviderKind::OpenAi,
        WireProtocol::OpenAiChatCompletions,
        model,
    )
}

/// Build an OpenAI Responses provider.
#[cfg(feature = "openai-responses")]
pub fn openai_responses(model: impl Into<String>) -> ProviderBuilder {
    ProviderBuilder::new(ProviderKind::OpenAi, WireProtocol::OpenAiResponses, model)
}

#[cfg(feature = "openai-chat")]
pub fn deepseek_chat(model: impl Into<String>) -> ProviderBuilder {
    ProviderBuilder::new(
        ProviderKind::DeepSeek,
        WireProtocol::OpenAiChatCompletions,
        model,
    )
}

#[cfg(feature = "openai-responses")]
pub fn deepseek_responses(model: impl Into<String>) -> ProviderBuilder {
    ProviderBuilder::new(ProviderKind::DeepSeek, WireProtocol::OpenAiResponses, model)
}

#[cfg(feature = "openai-chat")]
pub fn kimi_chat(model: impl Into<String>) -> ProviderBuilder {
    ProviderBuilder::new(
        ProviderKind::Kimi,
        WireProtocol::OpenAiChatCompletions,
        model,
    )
}

#[cfg(feature = "openai-responses")]
pub fn kimi_responses(model: impl Into<String>) -> ProviderBuilder {
    ProviderBuilder::new(ProviderKind::Kimi, WireProtocol::OpenAiResponses, model)
}

#[cfg(feature = "openai-chat")]
pub fn custom_chat(
    name: impl Into<String>,
    model: impl Into<String>,
    base_url: impl Into<String>,
) -> ProviderBuilder {
    ProviderBuilder::new(
        ProviderKind::Custom(name.into()),
        WireProtocol::OpenAiChatCompletions,
        model,
    )
    .base_url(base_url)
}

#[cfg(feature = "openai-responses")]
pub fn custom_responses(
    name: impl Into<String>,
    model: impl Into<String>,
    base_url: impl Into<String>,
) -> ProviderBuilder {
    ProviderBuilder::new(
        ProviderKind::Custom(name.into()),
        WireProtocol::OpenAiResponses,
        model,
    )
    .base_url(base_url)
}

fn is_builtin(kind: &ProviderKind) -> bool {
    !matches!(kind, ProviderKind::Custom(_))
}

#[derive(Clone)]
struct ProviderSpec {
    kind: ProviderKind,
    protocol: WireProtocol,
    model: String,
    endpoint: Endpoint,
    auth: Authentication,
    session_id: Option<SessionId>,
    limits: ModelLimits,
    #[cfg(feature = "openai-chat")]
    chat_dialect: Option<ChatDialect>,
    native_compaction: bool,
    responses_compact_threshold: Option<u32>,
    supports_vision: bool,
    replay_prior_reasoning: Option<bool>,
    product_capture: bool,
    capabilities: ModelCapabilities,
}

struct ProviderInner {
    spec: ProviderSpec,
    model: ProviderModel,
    metadata: ProviderMetadata,
}

/// A built provider that implements the neutral model port directly.
#[derive(Clone)]
pub struct Provider {
    inner: Arc<ProviderInner>,
}

impl std::fmt::Debug for Provider {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Provider")
            .field("metadata", &self.inner.metadata)
            .finish()
    }
}

impl Provider {
    fn from_spec(spec: ProviderSpec) -> Result<Self, BuildError> {
        let client = build_client(&spec)?;
        let model = ProviderModel::new(client, spec.capabilities.clone());
        let metadata = ProviderMetadata::new(
            spec.kind.clone(),
            spec.protocol,
            spec.model.clone(),
            spec.endpoint.clone(),
            spec.capabilities.clone(),
            spec.supports_vision,
        );
        Ok(Self {
            inner: Arc::new(ProviderInner {
                spec,
                model,
                metadata,
            }),
        })
    }

    pub fn metadata(&self) -> &ProviderMetadata {
        &self.inner.metadata
    }

    pub fn into_model(self) -> Arc<dyn ModelPort> {
        Arc::new(self)
    }

    /// Return a new provider with the same policy and a different session.
    pub fn with_session_id(self, session_id: SessionId) -> Self {
        let mut spec = self.inner.spec.clone();
        spec.session_id = Some(session_id);
        Self::from_spec(spec).expect("a validated provider spec can be rebuilt")
    }

    fn validate(
        &self,
        request: ModelRequest,
        cursor: Option<&ModelCursor>,
    ) -> Result<(CreateMessageParams, Option<ProviderConversationState>), ModelError> {
        if request.model != self.inner.metadata.model() {
            return Err(ModelError::new(
                ModelErrorKind::Permanent,
                "request model does not match the built provider model",
            ));
        }
        if request.max_output_tokens == 0
            || request.max_output_tokens > self.inner.spec.limits.max_output_tokens()
        {
            return Err(ModelError::new(
                ModelErrorKind::Permanent,
                "request max_output_tokens is outside the built limit",
            ));
        }
        if self
            .inner
            .metadata
            .capabilities()
            .known_context_window()
            .is_some_and(|window| request.max_output_tokens as usize >= window)
        {
            return Err(ModelError::new(
                ModelErrorKind::Permanent,
                "request max_output_tokens exceeds the known context window",
            ));
        }
        let state = match (self.inner.metadata.protocol(), cursor) {
            (WireProtocol::OpenAiResponses, Some(cursor)) => {
                let state = provider_state(cursor)?;
                let ProviderConversationState::OpenAiResponses(inner) = &state;
                if inner.provider != "openai_responses" {
                    return Err(ModelError::new(
                        ModelErrorKind::Permanent,
                        "cursor provider does not match the Responses protocol",
                    ));
                }
                let endpoint = Endpoint::parse(&inner.base_url).map_err(|error| {
                    ModelError::new(
                        ModelErrorKind::Permanent,
                        format!("invalid provider cursor endpoint: {error}"),
                    )
                })?;
                if endpoint != *self.inner.metadata.endpoint() {
                    return Err(ModelError::new(
                        ModelErrorKind::Permanent,
                        "cursor endpoint does not match the built provider",
                    ));
                }
                Some(state)
            }
            (WireProtocol::OpenAiResponses, None) => None,
            (_, Some(_)) => {
                return Err(ModelError::new(
                    ModelErrorKind::Permanent,
                    "this provider does not accept a provider cursor",
                ));
            }
            (_, None) => None,
        };
        let params = create_message_params(&request)?;
        Ok((params, state))
    }

    fn strip_body(response: &mut ModelResponse) {
        response.request_body = None;
    }

    fn stable_error(mut error: ModelError) -> ModelError {
        error.message = match error.kind {
            ModelErrorKind::ContextOverflow => "model context overflow",
            ModelErrorKind::Transient => "provider temporarily unavailable",
            ModelErrorKind::Permanent => "provider request failed",
            ModelErrorKind::Cancelled => "model call cancelled",
            ModelErrorKind::Unsupported => "provider operation unsupported",
        }
        .to_string();
        error
    }
}

#[async_trait]
impl ModelPort for Provider {
    fn capabilities(&self) -> ModelCapabilities {
        self.inner.metadata.capabilities().clone()
    }

    async fn stream(
        &self,
        request: ModelRequest,
        cursor: Option<&ModelCursor>,
        cancel: CancelToken,
    ) -> Result<ModelStream, ModelError> {
        let mut span = crate::telemetry::call_span("stream", &self.inner.metadata);
        let (params, state) = self.validate(request, cursor)?;
        let stream = self.inner.model.stream_lazy(params, state, cancel).await?;
        let capture = self.inner.spec.product_capture;
        Ok(Box::pin(stream.map(move |event| {
            crate::telemetry::record_event(&mut span, &event);
            match event {
                Ok(ModelEvent::Finished(mut response)) => {
                    if !capture {
                        Self::strip_body(&mut response);
                    }
                    Ok(ModelEvent::Finished(response))
                }
                Err(error) => Err(Self::stable_error(error)),
                other => other,
            }
        })))
    }

    async fn complete(
        &self,
        request: ModelRequest,
        cursor: Option<&ModelCursor>,
        cancel: CancelToken,
    ) -> Result<ModelResponse, ModelError> {
        let span = crate::telemetry::call_span("complete", &self.inner.metadata);
        let (params, state) = self.validate(request, cursor)?;
        let result = self
            .inner
            .model
            .complete_params(params, state, cancel)
            .await
            .map_err(Self::stable_error);
        crate::telemetry::record_result(&span, &result);
        let mut response = result?;
        if !self.inner.spec.product_capture {
            Self::strip_body(&mut response);
        }
        Ok(response)
    }

    async fn compact(
        &self,
        request: ModelRequest,
        cursor: Option<&ModelCursor>,
        cancel: CancelToken,
    ) -> Result<ModelResponse, ModelError> {
        let span = crate::telemetry::call_span("compact", &self.inner.metadata);
        if !self.inner.spec.native_compaction {
            return Err(ModelError::unsupported("native compaction"));
        }
        let (params, state) = self.validate(request, cursor)?;
        let result = self
            .inner
            .model
            .compact_params(params, state, cancel)
            .await
            .map_err(Self::stable_error);
        crate::telemetry::record_result(&span, &result);
        let mut response = result?;
        if !self.inner.spec.product_capture {
            Self::strip_body(&mut response);
        }
        Ok(response)
    }
}

fn build_client(spec: &ProviderSpec) -> Result<LlmProvider, BuildError> {
    assemble(AssemblySpec {
        kind: spec.kind.clone(),
        protocol: spec.protocol,
        model: spec.model.clone(),
        base_url: spec.endpoint.dispatch_base().to_string(),
        credentials: credentials_for(&spec.auth),
        compact_threshold: spec.responses_compact_threshold,
        replay_prior_reasoning: spec.replay_prior_reasoning,
        chat_dialect: {
            #[cfg(feature = "openai-chat")]
            {
                spec.chat_dialect
            }
            #[cfg(not(feature = "openai-chat"))]
            {
                None
            }
        },
        session_id: spec.session_id.as_ref().map(|id| id.as_str().to_string()),
    })
    .map_err(|error| match error {
        AssemblyError::FeatureDisabled => BuildError::ProviderFeatureDisabled {
            provider: spec.kind.clone(),
            protocol: spec.protocol,
        },
        AssemblyError::Transport(stage) => BuildError::TransportSetup { stage },
    })
}

fn credentials_for(auth: &Authentication) -> Arc<dyn CredentialProvider> {
    match auth {
        Authentication::Static(credential) => Arc::new(StaticCredentialProvider {
            credential: credential.clone(),
        }),
        Authentication::Refreshable(source) => Arc::new(SourceCredentialProvider {
            source: source.clone(),
        }),
        Authentication::None => Arc::new(ApiKeyProvider::new("")),
    }
}

#[derive(Debug)]
struct StaticCredentialProvider {
    credential: Credential,
}

impl CredentialProvider for StaticCredentialProvider {
    fn resolve(&self) -> BoxFuture<'_, Result<SecretString, LlmError>> {
        let secret = self.credential.secret().unwrap_or_default().to_string();
        Box::pin(async move { Ok(SecretString::from(secret)) })
    }

    fn resolve_with_kind(&self) -> BoxFuture<'_, Result<LegacyCredential, LlmError>> {
        let credential = legacy_credential(&self.credential);
        Box::pin(async move { Ok(credential) })
    }
}

#[derive(Debug)]
struct SourceCredentialProvider {
    source: Arc<dyn CredentialSource>,
}

impl CredentialProvider for SourceCredentialProvider {
    fn resolve(&self) -> BoxFuture<'_, Result<SecretString, LlmError>> {
        Box::pin(async move {
            let credential = self.source.resolve().await.map_err(credential_error)?;
            Ok(SecretString::from(
                credential.secret().unwrap_or_default().to_string(),
            ))
        })
    }

    fn resolve_with_kind(&self) -> BoxFuture<'_, Result<LegacyCredential, LlmError>> {
        Box::pin(async move {
            let credential = self.source.resolve().await.map_err(credential_error)?;
            Ok(legacy_credential(&credential))
        })
    }
}

fn credential_error(error: CredentialError) -> LlmError {
    LlmError::Auth(error.to_string())
}

fn legacy_credential(credential: &Credential) -> LegacyCredential {
    match credential.kind() {
        CredentialKind::ApiKey => LegacyCredential::ApiKey(SecretString::from(
            credential.secret().unwrap_or_default().to_string(),
        )),
        CredentialKind::Bearer => LegacyCredential::Bearer(SecretString::from(
            credential.secret().unwrap_or_default().to_string(),
        )),
        CredentialKind::None => LegacyCredential::None,
    }
}

#[cfg(all(
    test,
    any(
        feature = "anthropic",
        feature = "openai-chat",
        feature = "openai-responses"
    )
))]
mod tests {
    use super::*;
    #[cfg(feature = "openai-responses")]
    use futures_util::StreamExt;

    fn limits() -> ModelLimits {
        ModelLimits::new(Some(1_000), 100).unwrap()
    }

    #[cfg(feature = "anthropic")]
    #[test]
    fn anthropic_has_a_default_endpoint_and_redacted_debug() {
        let provider = anthropic("claude-test")
            .api_key("sk-secret")
            .unwrap()
            .limits(limits())
            .build()
            .unwrap();
        assert_eq!(
            provider.metadata().endpoint().as_str(),
            "https://api.anthropic.com/"
        );
        let debug = format!("{provider:?}");
        assert!(debug.contains("Provider"));
        assert!(!debug.contains("sk-secret"));
    }

    #[cfg(feature = "openai-chat")]
    #[test]
    fn custom_anonymous_provider_is_a_model_port() {
        let provider = custom_chat("local", "model", "http://127.0.0.1:8000")
            .anonymous()
            .limits(limits())
            .build()
            .unwrap();
        let model: Arc<dyn ModelPort> = provider.into_model();
        assert_eq!(model.capabilities().context_window, Some(1_000));
    }

    #[cfg(feature = "openai-chat")]
    #[tokio::test]
    async fn stable_request_model_mismatch_fails_before_dispatch() {
        let provider = custom_chat("local", "model", "http://127.0.0.1:8000")
            .anonymous()
            .limits(limits())
            .build()
            .unwrap();
        let request = ModelRequest {
            model: "other".into(),
            system: String::new(),
            messages: Vec::new(),
            tools: Vec::new(),
            max_output_tokens: 10,
            options: Default::default(),
        };
        let error = provider
            .complete(request, None, CancelToken::new())
            .await
            .unwrap_err();
        assert_eq!(error.kind, ModelErrorKind::Permanent);
    }

    #[cfg(feature = "anthropic")]
    #[test]
    fn required_builtin_auth_is_rejected_at_build() {
        let error = anthropic("claude-test")
            .limits(limits())
            .build()
            .unwrap_err();
        assert!(matches!(error, BuildError::AuthenticationRequired { .. }));
    }

    #[cfg(feature = "openai-chat")]
    #[test]
    fn provider_session_rebind_preserves_metadata() {
        let provider = custom_chat("local", "model", "http://127.0.0.1:8000")
            .anonymous()
            .session_id(SessionId::new("parent").unwrap())
            .limits(limits())
            .build()
            .unwrap();
        let rebounded = provider.with_session_id(SessionId::new("child").unwrap());
        assert_eq!(rebounded.metadata().model(), "model");
        assert_eq!(
            rebounded.metadata().endpoint().as_str(),
            "http://127.0.0.1:8000/"
        );
    }

    #[test]
    fn root_endpoint_has_no_double_slash() {
        let endpoint = Endpoint::parse("https://api.example.com/").unwrap();
        assert_eq!(
            format!("{}/messages", endpoint.dispatch_base()),
            "https://api.example.com/messages"
        );
    }

    #[cfg(feature = "openai-responses")]
    #[tokio::test]
    async fn cancellation_interrupts_a_refreshable_credential_wait() {
        #[derive(Debug)]
        struct NeverResolves;

        impl CredentialSource for NeverResolves {
            fn resolve(
                &self,
            ) -> futures_util::future::BoxFuture<'_, Result<Credential, CredentialError>>
            {
                Box::pin(std::future::pending())
            }
        }

        let provider = custom_responses("local", "model", "http://127.0.0.1:1")
            .refreshable(Arc::new(NeverResolves))
            .limits(limits())
            .build()
            .unwrap();
        let cancel = CancelToken::new();
        let mut stream = provider
            .stream(
                ModelRequest {
                    model: "model".into(),
                    system: String::new(),
                    messages: Vec::new(),
                    tools: Vec::new(),
                    max_output_tokens: 10,
                    options: Default::default(),
                },
                None,
                cancel.clone(),
            )
            .await
            .unwrap();
        cancel.cancel();
        let error = stream.next().await.unwrap().unwrap_err();
        assert_eq!(error.kind, ModelErrorKind::Cancelled);
        assert!(stream.next().await.is_none());
    }
}
