use async_trait::async_trait;
use tact_agent_api::{
    CancelToken, GateDecision, ResolvedToolCall, ToolCall, ToolExecutionResult, ToolGate,
    ToolGateContext, ToolGateError, ToolResolver,
};

/// Deny every tool call.
#[derive(Debug, Clone, Copy, Default)]
pub struct DenyAllGate;

#[async_trait]
impl ToolGate for DenyAllGate {
    async fn before(
        &self,
        _call: ToolCall,
        _resolver: &dyn ToolResolver,
        _context: &ToolGateContext,
        _cancel: CancelToken,
    ) -> Result<GateDecision, ToolGateError> {
        Ok(GateDecision::Deny {
            reason: "no tool gate configured".into(),
        })
    }

    async fn after(
        &self,
        _call: &ResolvedToolCall,
        result: ToolExecutionResult,
        _context: &ToolGateContext,
        _cancel: CancelToken,
    ) -> Result<ToolExecutionResult, ToolGateError> {
        Ok(result)
    }
}

/// Explicitly allow every tool call; hosts must opt in.
#[derive(Debug, Clone, Copy, Default)]
pub struct AllowAllGate;

#[async_trait]
impl ToolGate for AllowAllGate {
    async fn before(
        &self,
        call: ToolCall,
        resolver: &dyn ToolResolver,
        _context: &ToolGateContext,
        _cancel: CancelToken,
    ) -> Result<GateDecision, ToolGateError> {
        let call = resolver
            .resolve(&call)
            .map_err(|error| ToolGateError::Failed(error.to_string()))?;
        Ok(GateDecision::Allow {
            call: Box::new(call),
            label: None,
        })
    }

    async fn after(
        &self,
        _call: &ResolvedToolCall,
        result: ToolExecutionResult,
        _context: &ToolGateContext,
        _cancel: CancelToken,
    ) -> Result<ToolExecutionResult, ToolGateError> {
        Ok(result)
    }
}
