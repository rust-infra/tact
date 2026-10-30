use tact_llm::{ModelLimits, SessionId, openai_responses};

pub fn build_provider(api_key: String) -> Result<tact_llm::Provider, tact_llm::BuildError> {
    openai_responses("gpt-5.6")
        .api_key(api_key)
        .map_err(tact_llm::BuildError::InvalidCredential)?
        .session_id(SessionId::new("fixture-session").expect("fixture session id"))
        .limits(ModelLimits::new(Some(400_000), 16_384).expect("fixture limits"))
        .build()
}
