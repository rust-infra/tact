use std::sync::Arc;

use futures_util::future::BoxFuture;
use tact_llm::{Credential, CredentialError, CredentialSource, ModelLimits, openai_responses};

#[derive(Debug)]
struct TokenStore;

impl CredentialSource for TokenStore {
    fn resolve(&self) -> BoxFuture<'_, Result<Credential, CredentialError>> {
        Box::pin(async { Credential::bearer("refreshed-token") })
    }
}

fn main() -> Result<(), tact_llm::BuildError> {
    let _provider = openai_responses("gpt-5.6")
        .refreshable(Arc::new(TokenStore))
        .limits(ModelLimits::new(Some(400_000), 16_384).expect("limits"))
        .build()?;
    Ok(())
}
