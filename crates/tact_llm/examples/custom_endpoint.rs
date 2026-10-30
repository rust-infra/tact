use tact_llm::{ChatDialect, ModelLimits, custom_chat};

fn main() -> Result<(), tact_llm::BuildError> {
    let _provider = custom_chat(
        "local",
        "Qwen/Qwen3-Coder-30B-A3B-Instruct",
        "http://127.0.0.1:8000/v1",
    )
    .anonymous()
    .chat_dialect(ChatDialect::Standard)
    .limits(ModelLimits::new(Some(128_000), 8_192).expect("limits"))
    .build()?;
    Ok(())
}
