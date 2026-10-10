//! Protocol-neutral streaming facts a provider emits while a model call runs.
//!
//! A provider adapter never knows which run it is serving — it only sees a
//! request — so every event is built with `run_id: None`. The emitter that owns
//! the run stamps the identity before the event reaches a View.

use tact_protocol::{ModelCallParams, RuntimeEvent, ThinkingChunk, TokenUsageInfo};

pub(crate) fn text(content: String) -> RuntimeEvent {
    RuntimeEvent::Text {
        run_id: None,
        role: "assistant".into(),
        content,
    }
}

pub(crate) fn thinking(chunk: ThinkingChunk) -> RuntimeEvent {
    RuntimeEvent::Thinking {
        run_id: None,
        chunk,
    }
}

pub(crate) fn token_usage(usage: TokenUsageInfo) -> RuntimeEvent {
    RuntimeEvent::TokenUsage {
        run_id: None,
        usage,
    }
}

pub(crate) fn model_info(params: ModelCallParams) -> RuntimeEvent {
    RuntimeEvent::ModelInfo {
        run_id: None,
        params,
    }
}
