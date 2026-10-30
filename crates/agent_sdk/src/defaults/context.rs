use async_trait::async_trait;
use tact_agent_api::{
    CancelToken, CompactionOutcome, CompactionRequest, ContextError, ContextPolicy,
    ContextPreparation, ContextView, ModelCallOptions, ModelMessage, ModelPort,
    PreparationDecision, PreparedRequest, RequestPlan, RunInput,
};

/// Literal system-prompt and transcript context policy with no compaction.
#[derive(Clone)]
pub struct BasicContextPolicy {
    system_prompt: String,
    options: ModelCallOptions,
}

impl BasicContextPolicy {
    pub fn new(system_prompt: impl Into<String>, options: ModelCallOptions) -> Self {
        Self {
            system_prompt: system_prompt.into(),
            options,
        }
    }
}

#[async_trait]
impl ContextPolicy for BasicContextPolicy {
    async fn prepare_run(
        &self,
        input: &RunInput,
        _state: &ContextView<'_>,
        _model: &dyn ModelPort,
        _cancel: CancelToken,
    ) -> Result<PreparationDecision<ContextPreparation>, ContextError> {
        Ok(PreparationDecision::Continue(ContextPreparation {
            inputs: input.inputs.clone(),
            appended_messages: Vec::new(),
        }))
    }

    async fn prepare_request(
        &self,
        state: &ContextView<'_>,
        _model: &dyn ModelPort,
        _cancel: CancelToken,
    ) -> Result<PreparationDecision<RequestPlan>, ContextError> {
        let messages = state.transcript.iter().map(ModelMessage::from).collect();
        Ok(PreparationDecision::Continue(RequestPlan {
            request: PreparedRequest {
                system: self.system_prompt.clone(),
                messages,
                cursor: None,
                options: self.options.clone(),
            },
            appended_messages: Vec::new(),
        }))
    }

    async fn compact(
        &self,
        _state: &ContextView<'_>,
        _request: CompactionRequest,
        _model: &dyn ModelPort,
        _cancel: CancelToken,
    ) -> Result<CompactionOutcome, ContextError> {
        Ok(CompactionOutcome::Skipped {
            reason: "basic context policy has no compaction".into(),
        })
    }
}
