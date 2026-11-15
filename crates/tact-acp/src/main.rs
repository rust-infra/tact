#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tact_acp::run().await
}
