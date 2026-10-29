//! Private construction of the concrete adapter behind any public entry point.

use std::sync::Arc;

#[cfg(feature = "openai-chat")]
use crate::OpenAiProtocol;
#[cfg(feature = "anthropic")]
use crate::anthropic::AnthropicAdapter;
use crate::auth::CredentialProvider;
use crate::client::LlmProvider;
#[cfg(feature = "openai-chat")]
use crate::openai::compatible::{ChatCompletionsAdapter, OpenAiAdapter};
#[cfg(feature = "openai-responses")]
use crate::openai::responses::OpenAiResponsesAdapter;
#[cfg(feature = "openai-chat")]
use crate::profile::ProviderProfile;
#[cfg(any(feature = "openai-chat", feature = "openai-responses"))]
use crate::transport::SharedHttpClient;
use crate::{ChatDialect, ProviderKind, TransportSetupStage, WireProtocol};

pub(crate) struct AssemblySpec {
    pub kind: ProviderKind,
    pub protocol: WireProtocol,
    pub model: String,
    pub base_url: String,
    pub credentials: Arc<dyn CredentialProvider>,
    pub compact_threshold: Option<u32>,
    pub replay_prior_reasoning: Option<bool>,
    pub chat_dialect: Option<ChatDialect>,
    pub session_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AssemblyError {
    #[allow(dead_code)] // constructed only when a protocol feature is disabled
    FeatureDisabled,
    Transport(TransportSetupStage),
}

pub(crate) fn assemble(spec: AssemblySpec) -> Result<LlmProvider, AssemblyError> {
    let mut client = match spec.protocol {
        WireProtocol::AnthropicMessages => assemble_anthropic(spec.credentials, &spec.base_url)?,
        WireProtocol::OpenAiChatCompletions => assemble_chat(&spec)?,
        WireProtocol::OpenAiResponses => assemble_responses(&spec)?,
    };
    if let Some(session_id) = &spec.session_id {
        client.set_user_id(session_id);
    }
    Ok(client)
}

#[cfg(feature = "anthropic")]
fn assemble_anthropic(
    credentials: Arc<dyn CredentialProvider>,
    base_url: &str,
) -> Result<LlmProvider, AssemblyError> {
    Ok(LlmProvider::Anthropic(AnthropicAdapter::new_with_auth(
        credentials,
        base_url,
    )))
}

#[cfg(not(feature = "anthropic"))]
fn assemble_anthropic(
    _credentials: Arc<dyn CredentialProvider>,
    _base_url: &str,
) -> Result<LlmProvider, AssemblyError> {
    Err(AssemblyError::FeatureDisabled)
}

#[cfg(feature = "openai-chat")]
fn assemble_chat(spec: &AssemblySpec) -> Result<LlmProvider, AssemblyError> {
    let http = SharedHttpClient::try_new()
        .map_err(|_| AssemblyError::Transport(TransportSetupStage::ReqwestClient))?;
    let adapter = OpenAiAdapter::with_auth(&spec.base_url, http, spec.credentials.clone());
    let profile = ProviderProfile {
        provider: spec.kind.clone(),
        base_url: spec.base_url.clone(),
        model: spec.model.clone(),
        protocol: OpenAiProtocol::ChatCompletions,
        responses_compact_threshold: None,
    };
    let adapter = ChatCompletionsAdapter::new(profile, adapter);
    let adapter = match spec.chat_dialect {
        Some(dialect) => adapter.with_dialect(dialect),
        None => adapter,
    };
    Ok(LlmProvider::ChatCompletions(adapter))
}

#[cfg(not(feature = "openai-chat"))]
fn assemble_chat(_spec: &AssemblySpec) -> Result<LlmProvider, AssemblyError> {
    Err(AssemblyError::FeatureDisabled)
}

#[cfg(feature = "openai-responses")]
fn assemble_responses(spec: &AssemblySpec) -> Result<LlmProvider, AssemblyError> {
    let http = SharedHttpClient::try_new()
        .map_err(|_| AssemblyError::Transport(TransportSetupStage::ReqwestClient))?;
    let adapter = OpenAiResponsesAdapter::new_with_auth(
        spec.credentials.clone(),
        &spec.base_url,
        spec.compact_threshold,
        http,
    );
    let adapter = match spec.replay_prior_reasoning {
        Some(replay) => adapter.with_replay_prior_reasoning(replay),
        None => adapter,
    };
    Ok(LlmProvider::OpenAiResponses(adapter))
}

#[cfg(not(feature = "openai-responses"))]
fn assemble_responses(_spec: &AssemblySpec) -> Result<LlmProvider, AssemblyError> {
    Err(AssemblyError::FeatureDisabled)
}
