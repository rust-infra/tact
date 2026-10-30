use tact_agent_sdk::AgentBuilder;
use tact_llm::{ModelLimits, openai_responses};

pub fn build_agent(api_key: String) -> Result<tact_agent_sdk::Agent, Box<dyn std::error::Error>> {
    let provider = openai_responses("gpt-5.6")
        .api_key(api_key)?
        .limits(ModelLimits::new(Some(400_000), 16_384)?)
        .build()?;
    Ok(AgentBuilder::new(provider.into_model())
        .system_prompt("You are a concise fixture agent.")
        .build()?)
}
