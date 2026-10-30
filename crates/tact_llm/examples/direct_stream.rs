use futures_util::StreamExt;
use tact_llm::{CancelToken, ModelEvent, ModelLimits, ModelPort, ModelRequest, openai_responses};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let provider = openai_responses("gpt-5.6")
        .api_key(std::env::var("OPENAI_API_KEY")?)?
        .limits(ModelLimits::new(Some(400_000), 16_384)?)
        .build()?;
    let mut stream = provider
        .stream(
            ModelRequest {
                model: "gpt-5.6".into(),
                system: "Be concise.".into(),
                messages: Vec::new(),
                tools: Vec::new(),
                max_output_tokens: 1_000,
                options: Default::default(),
            },
            None,
            CancelToken::new(),
        )
        .await?;
    while let Some(event) = stream.next().await {
        if matches!(event?, ModelEvent::Finished(_)) {
            break;
        }
    }
    Ok(())
}
