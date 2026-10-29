use tact_llm::{ModelLimits, openai_responses};

fn main() -> Result<(), tact_llm::BuildError> {
    tracing::info!("building a provider; install the host's subscriber separately");
    let _provider = openai_responses("gpt-5.6")
        .api_key("example-key")
        .map_err(tact_llm::BuildError::InvalidCredential)?
        .limits(ModelLimits::new(Some(400_000), 16_384).expect("limits"))
        .build()?;
    Ok(())
}
