use tact_agent_api::RunId;
use uuid::Uuid;

/// Allocates a unique identity for every bounded activation.
pub trait RunIdSource: Send + Sync {
    fn next_run_id(&self) -> RunId;
}

/// Process-unique UUID v4 run identities.
#[derive(Debug, Clone, Copy, Default)]
pub struct UuidRunIdSource;

impl RunIdSource for UuidRunIdSource {
    fn next_run_id(&self) -> RunId {
        RunId::new(Uuid::new_v4().to_string())
    }
}
