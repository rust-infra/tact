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
        Ok(Self::from_parts(spec, client))
    }

    fn from_parts(spec: ProviderSpec, client: LlmProvider) -> Self {
        let model = ProviderModel::new(client, spec.capabilities.clone());
        let metadata = ProviderMetadata::new(
            spec.kind.clone(),
            spec.protocol,
            spec.model.clone(),
            spec.endpoint.clone(),
            spec.capabilities.clone(),
            spec.supports_vision,
        );
        Self {
            inner: Arc::new(ProviderInner {
                spec,
                model,
                metadata,
            }),
        }
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

/// Records an abandoned live stream, so a dispatched call that never reaches
/// a terminal event still reports an outcome.
struct GuardedStream {
    inner: ModelStream,
    span: crate::telemetry::SpanHandle,
    started: bool,
    terminated: bool,
}

impl futures_util::Stream for GuardedStream {
    type Item = Result<ModelEvent, ModelError>;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        context: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        self.started = true;
        let item = self.inner.as_mut().poll_next(context);
        if matches!(
            &item,
            std::task::Poll::Ready(None)
                | std::task::Poll::Ready(Some(Err(_)))
                | std::task::Poll::Ready(Some(Ok(ModelEvent::Finished(_))))
        ) {
            self.terminated = true;
        }
        item
    }
}

impl Drop for GuardedStream {
    fn drop(&mut self) {
        if self.started && !self.terminated {
            crate::telemetry::record_dropped(&self.span);
        }
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
        let (params, state) = match self.validate(request, cursor) {
            Ok(validated) => validated,
            Err(error) => {
                crate::telemetry::record_failure(&span, &error);
                return Err(error);
            }
        };
        let handle = span.handle();
        let stream = self
            .inner
            .model
            .stream_lazy(params, state, cancel, handle.clone())
            .await;
        let capture = self.inner.spec.product_capture;
        let guarded = GuardedStream {
            inner: Box::pin(stream.map(move |event| {
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
            })),
            span: handle,
            started: false,
            terminated: false,
        };
        Ok(Box::pin(guarded))
    }

    async fn complete(
        &self,
        request: ModelRequest,
        cursor: Option<&ModelCursor>,
        cancel: CancelToken,
    ) -> Result<ModelResponse, ModelError> {
        let span = crate::telemetry::call_span("complete", &self.inner.metadata);
        let (params, state) = match self.validate(request, cursor) {
            Ok(validated) => validated,
            Err(error) => {
                crate::telemetry::record_failure(&span, &error);
                return Err(error);
            }
        };
        let result = crate::telemetry::instrument(
            span.handle(),
            self.inner
                .model
                .complete_params_spanned(params, state, cancel, span.handle()),
        )
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
            let error = ModelError::unsupported("native compaction");
            crate::telemetry::record_failure(&span, &error);
            return Err(error);
        }
        let (params, state) = match self.validate(request, cursor) {
            Ok(validated) => validated,
            Err(error) => {
                crate::telemetry::record_failure(&span, &error);
                return Err(error);
            }
        };
        let result = crate::telemetry::instrument(
            span.handle(),
            self.inner
                .model
                .compact_params_spanned(params, state, cancel, span.handle()),
        )
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

/// Telemetry contract tests that go through the real `Provider` path.
#[cfg(all(test, feature = "telemetry"))]
mod telemetry_tests {
    use futures_util::StreamExt;
    use tact_agent_api::{
        AgentContentBlock, AgentRole, CancelToken, ModelMessage, ModelPort, ModelRequest,
    };

    use super::*;
    use crate::mock::MockClient;
    use crate::{ContentBlock, StopReason, telemetry::testing::Capture};

    fn spec(protocol: WireProtocol) -> ProviderSpec {
        spec_with_compaction(protocol, false)
    }

    fn spec_with_compaction(protocol: WireProtocol, native_compaction: bool) -> ProviderSpec {
        ProviderSpec {
            kind: ProviderKind::Custom("telemetry-test".into()),
            protocol,
            model: "mock".into(),
            endpoint: Endpoint::parse("http://127.0.0.1:1").unwrap(),
            auth: Authentication::None,
            session_id: None,
            limits: ModelLimits::new(Some(1_000), 100).unwrap(),
            #[cfg(feature = "openai-chat")]
            chat_dialect: None,
            native_compaction,
            responses_compact_threshold: None,
            supports_vision: false,
            replay_prior_reasoning: None,
            product_capture: false,
            capabilities: ModelCapabilities::new("mock", Some(1_000), 100, true, false).unwrap(),
        }
    }

    fn provider(client: MockClient) -> Provider {
        provider_with(spec(WireProtocol::OpenAiChatCompletions), client)
    }

    fn provider_with(spec: ProviderSpec, client: MockClient) -> Provider {
        Provider::from_parts(spec, LlmProvider::Mock(client))
    }

    fn turn() -> (Vec<ContentBlock>, Option<StopReason>) {
        (
            vec![ContentBlock::Text {
                text: "hello".into(),
            }],
            Some(StopReason::EndTurn),
        )
    }

    fn request() -> ModelRequest {
        ModelRequest {
            model: "mock".into(),
            system: String::new(),
            messages: vec![ModelMessage {
                role: AgentRole::User,
                content: vec![AgentContentBlock::Text { text: "hi".into() }],
            }],
            tools: Vec::new(),
            max_output_tokens: 10,
            options: Default::default(),
        }
    }

    // `set_default` only covers this thread, so the task must stay on it.
    #[tokio::test(flavor = "current_thread")]
    async fn complete_runs_the_model_future_inside_the_call_span() {
        let (capture, _guard) = Capture::install(tracing::Level::INFO);
        // The responder runs inside the awaited model future, so its event
        // only carries the call span if `complete` instruments that future.
        let provider = provider(MockClient::with_responder(|_request, _index| {
            tracing::info!("inside complete future");
            Ok((turn().0, turn().1, None))
        }));

        provider
            .complete(request(), None, CancelToken::new())
            .await
            .unwrap();

        let output = capture.text();
        assert!(
            output.lines().any(|line| line.contains("tact_llm.call{")
                && line.contains("request_kind=\"complete\"")
                && line.contains("inside complete future")),
            "complete did not record under its call span:\n{output}"
        );
    }

    // `set_default` only covers this thread, so the task must stay on it.
    #[tokio::test(flavor = "current_thread")]
    async fn compact_runs_the_model_future_inside_the_call_span() {
        let (capture, _guard) = Capture::install(tracing::Level::INFO);
        let provider = provider_with(
            spec_with_compaction(WireProtocol::OpenAiChatCompletions, true),
            MockClient::with_responder(|_request, _index| {
                tracing::info!("inside compact future");
                Ok((turn().0, turn().1, None))
            }),
        );

        provider
            .compact(request(), None, CancelToken::new())
            .await
            .unwrap();

        let output = capture.text();
        assert!(
            output.lines().any(|line| line.contains("tact_llm.call{")
                && line.contains("request_kind=\"compact\"")
                && line.contains("inside compact future")),
            "compact did not run inside its call span:\n{output}"
        );
    }

    // `set_default` only covers this thread, so the task must stay on it.
    #[tokio::test(flavor = "current_thread")]
    async fn lazy_stream_producer_runs_inside_the_call_span() {
        let (capture, _guard) = Capture::install(tracing::Level::INFO);
        // The responder runs inside the spawned producer task, so its event
        // only carries the call span if the spawn is instrumented.
        let provider = provider(MockClient::with_responder(|_request, _index| {
            tracing::info!("inside lazy stream producer");
            Ok((turn().0, turn().1, None))
        }));

        let mut stream = provider
            .stream(request(), None, CancelToken::new())
            .await
            .unwrap();
        while stream.next().await.is_some() {}

        let output = capture.text();
        assert!(
            output.lines().any(|line| line.contains("tact_llm.call{")
                && line.contains("request_kind=\"stream\"")
                && line.contains("inside lazy stream producer")),
            "lazy stream producer lost the call span:\n{output}"
        );
        // Matching text is not enough: a second span with the same fields
        // formats identically. Exactly one call span may exist.
        assert_eq!(
            capture.call_span_ids().len(),
            1,
            "stream opened more than one call span, so the producer and the \
             stream events are not on the same span:\n{output}"
        );
        assert!(
            output.contains("stream first event"),
            "stream events did not reach the call span:\n{output}"
        );
    }

    // `set_default` only covers this thread, so the task must stay on it.
    #[tokio::test(flavor = "current_thread")]
    async fn stream_http_failure_records_its_status_class() {
        let (capture, _guard) = Capture::install(tracing::Level::INFO);
        // `with_error` flattens to `LlmError::Mock`, which drops the class, so
        // return the typed error from a responder instead.
        let provider = provider(MockClient::with_responder(|_request, _index| {
            Err(crate::error::LlmError::HttpError {
                status: 401,
                class: crate::error::HttpErrorClass::Authentication,
                retry_after: None,
            })
        }));

        let mut stream = provider
            .stream(request(), None, CancelToken::new())
            .await
            .unwrap();
        while stream.next().await.is_some() {}

        let output = capture.text();
        assert!(
            output.lines().any(|line| line.contains("tact_llm.call{")
                && line.contains("request_kind=\"stream\"")
                && line.contains("status_class=\"authentication\"")
                && line.contains("outcome=\"failed\"")),
            "the stream producer lost its HTTP class:\n{output}"
        );
    }

    // Public helper path used by legacy callers must not invent a span.
    #[tokio::test(flavor = "current_thread")]
    async fn unspanned_stream_params_do_not_emit_a_call_span() {
        let (capture, _guard) = Capture::install(tracing::Level::INFO);
        let provider = ProviderModel::new(
            LlmProvider::Mock(MockClient::with_responder(|_request, _index| {
                tracing::info!("inside unspanned producer");
                Ok((turn().0, turn().1, None))
            })),
            spec(WireProtocol::OpenAiChatCompletions).capabilities,
        );
        let params =
            crate::neutral::create_message_params(&request()).expect("request converts to params");
        let mut stream = provider
            .stream_params(params, None, CancelToken::new())
            .await
            .unwrap();
        while stream.next().await.is_some() {}

        let output = capture.text();
        assert!(
            output.contains("inside unspanned producer"),
            "producer did not run:\n{output}"
        );
        assert!(
            !output.contains("tact_llm.call"),
            "unspanned path emitted a call span:\n{output}"
        );
    }

    // `set_default` only covers this thread, so the task must stay on it.
    #[tokio::test(flavor = "current_thread")]
    async fn validation_failures_are_recorded_before_dispatch() {
        let (capture, _guard) = Capture::install(tracing::Level::INFO);
        // Native compaction on, so `compact` reaches validation instead of
        // short-circuiting at the unsupported check.
        // The responder panics if reached, which proves validation rejects
        // the request before anything is dispatched.
        let provider = provider_with(
            spec_with_compaction(WireProtocol::OpenAiChatCompletions, true),
            MockClient::with_responder(|_request, _index| {
                panic!("validation must reject before dispatch");
            }),
        );
        let mut mismatched = request();
        mismatched.model = "other".into();

        let complete = provider
            .complete(mismatched.clone(), None, CancelToken::new())
            .await
            .unwrap_err();
        assert_eq!(complete.kind, ModelErrorKind::Permanent);
        let Err(stream) = provider
            .stream(mismatched.clone(), None, CancelToken::new())
            .await
        else {
            panic!("stream must reject a mismatched model");
        };
        assert_eq!(stream.kind, ModelErrorKind::Permanent);
        let compact = provider
            .compact(mismatched, None, CancelToken::new())
            .await
            .unwrap_err();
        assert_eq!(compact.kind, ModelErrorKind::Permanent);

        let output = capture.text();
        for kind in ["complete", "stream", "compact"] {
            assert!(
                output.lines().any(|line| line.contains("tact_llm.call{")
                    && line.contains(&format!("request_kind=\"{kind}\""))
                    && line.contains("outcome=\"failed\"")),
                "{kind} validation failure left no failure event:\n{output}"
            );
        }
    }

    // `set_default` only covers this thread, so the task must stay on it.
    #[tokio::test(flavor = "current_thread")]
    async fn native_compaction_validation_failure_is_recorded() {
        let (capture, _guard) = Capture::install(tracing::Level::INFO);
        let provider = provider_with(
            spec_with_compaction(WireProtocol::OpenAiChatCompletions, true),
            MockClient::new(vec![turn()]),
        );
        let mut mismatched = request();
        mismatched.model = "other".into();

        provider
            .compact(mismatched, None, CancelToken::new())
            .await
            .unwrap_err();

        let output = capture.text();
        assert!(
            output.lines().any(|line| line.contains("tact_llm.call{")
                && line.contains("request_kind=\"compact\"")
                && line.contains("outcome=\"failed\"")),
            "compact validation failure left no failure event:\n{output}"
        );
    }

    // `set_default` only covers this thread, so the task must stay on it.
    #[tokio::test(flavor = "current_thread")]
    async fn draining_a_stream_to_terminal_records_no_drop() {
        let (capture, _guard) = Capture::install(tracing::Level::INFO);
        let provider = provider(MockClient::new(vec![turn()]));

        let mut stream = provider
            .stream(request(), None, CancelToken::new())
            .await
            .unwrap();
        while stream.next().await.is_some() {}
        drop(stream);

        let output = capture.text();
        assert!(
            output.lines().any(|line| line.contains("tact_llm.call{")
                && line.contains("request_kind=\"stream\"")
                && line.contains("outcome=\"finished\"")),
            "stream did not reach a terminal event:\n{output}"
        );
        assert!(
            !output.contains("outcome=\"dropped\""),
            "a completed stream must not report a drop:\n{output}"
        );
    }

    // `set_default` only covers this thread, so the task must stay on it.
    #[tokio::test(flavor = "current_thread")]
    async fn unsupported_compaction_is_recorded() {
        let (capture, _guard) = Capture::install(tracing::Level::INFO);
        let provider = provider(MockClient::new(vec![turn()]));

        let error = provider
            .compact(request(), None, CancelToken::new())
            .await
            .unwrap_err();
        assert_eq!(error.kind, ModelErrorKind::Unsupported);

        let output = capture.text();
        assert!(
            output.lines().any(|line| line.contains("tact_llm.call{")
                && line.contains("request_kind=\"compact\"")
                && line.contains("error_kind=Unsupported")
                && line.contains("outcome=\"failed\"")),
            "unsupported compaction left no failure event:\n{output}"
        );
    }

    // `set_default` only covers this thread, so the task must stay on it.
    #[tokio::test(flavor = "current_thread")]
    async fn dropping_a_live_stream_records_an_outcome() {
        let (capture, _guard) = Capture::install(tracing::Level::INFO);
        let provider = provider(MockClient::new(vec![turn()]));

        let mut stream = provider
            .stream(request(), None, CancelToken::new())
            .await
            .unwrap();
        // Dispatch, then abandon without reading a terminal event.
        assert!(stream.next().await.is_some());
        drop(stream);

        let output = capture.text();
        assert!(
            output.lines().any(|line| line.contains("tact_llm.call{")
                && line.contains("request_kind=\"stream\"")
                && line.contains("outcome=\"dropped\"")),
            "dropping a live stream left no outcome:\n{output}"
        );
    }

    // `set_default` only covers this thread, so the task must stay on it.
    #[tokio::test(flavor = "current_thread")]
    async fn dropping_an_unpolled_stream_records_nothing() {
        let (capture, _guard) = Capture::install(tracing::Level::INFO);
        let provider = provider(MockClient::new(vec![turn()]));

        drop(
            provider
                .stream(request(), None, CancelToken::new())
                .await
                .unwrap(),
        );

        let output = capture.text();
        assert!(
            !output.contains("outcome=\"dropped\""),
            "an unpolled stream must dispatch nothing:\n{output}"
        );
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
