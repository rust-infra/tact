use tact_agent_sdk::AgentBuilder;
use tact_llm::{ModelLimits, openai_responses};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let provider = openai_responses("gpt-5.6")
        .api_key(std::env::var("OPENAI_API_KEY")?)?
        .limits(ModelLimits::new(Some(400_000), 16_384)?)
        .build()?;
    let mut session = AgentBuilder::new(provider.into_model())
        .system_prompt("You are a concise agent.")
        .build_session("sdk-example")?;
    let _ = session.run_text("Hello.").await?;
    Ok(())
}
