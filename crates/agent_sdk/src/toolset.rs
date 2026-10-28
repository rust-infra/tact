use std::{
    collections::HashMap,
    fmt,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use async_trait::async_trait;
use tact_agent_api::{
    CancelToken, ResolvedToolCall, ResourceAccess, ToolCall, ToolDescriptor, ToolError,
    ToolErrorKind, ToolExecutionContext, ToolExecutionResult, ToolPort, ToolResolver, ToolSnapshot,
    ToolSpec,
};

use crate::ToolSetError;

/// One statically injected tool implementation.
#[async_trait]
pub trait ToolHandler: Send + Sync {
    fn spec(&self) -> ToolSpec;

    fn metadata(&self) -> serde_json::Value {
        serde_json::Value::Null
    }

    /// Conservative by default: omitted access is a global scheduling barrier.
    fn access(&self, _input: &serde_json::Value) -> ResourceAccess {
        ResourceAccess::barrier()
    }

    async fn execute(
        &self,
        input: serde_json::Value,
        context: ToolExecutionContext,
    ) -> Result<ToolExecutionResult, ToolError>;
}

/// Static group of tools contributed as one composition unit.
pub trait ToolProvider: Send + Sync {
    fn tools(&self) -> Vec<Arc<dyn ToolHandler>>;
}

struct ToolSetEntry {
    spec: ToolSpec,
    metadata: serde_json::Value,
    handler: Arc<dyn ToolHandler>,
}

/// Immutable static tool composition implementing the kernel tool ports.
pub struct ToolSet {
    entries: Vec<ToolSetEntry>,
    by_name: HashMap<String, usize>,
    generation: AtomicU64,
}

impl fmt::Debug for ToolSet {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ToolSet")
            .field(
                "tools",
                &self
                    .entries
                    .iter()
                    .map(|entry| entry.spec.name.as_str())
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}

impl Default for ToolSet {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolSet {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            by_name: HashMap::new(),
            generation: AtomicU64::new(0),
        }
    }

    /// Add one tool, validating before any mutation.
    #[allow(clippy::should_implement_trait)]
    pub fn add(mut self, tool: impl ToolHandler + 'static) -> Result<Self, ToolSetError> {
        let handler: Arc<dyn ToolHandler> = Arc::new(tool);
        let entry = Self::snapshot(handler);
        Self::validate_name(&self.by_name, &entry.spec.name)?;
        self.commit(entry);
        Ok(self)
    }

    /// Add a provider batch atomically.
    pub fn add_provider(
        mut self,
        provider: impl ToolProvider + 'static,
    ) -> Result<Self, ToolSetError> {
        let mut candidates = Vec::new();
        let mut batch_names = HashMap::new();
        for handler in provider.tools() {
            let entry = Self::snapshot(handler);
            Self::validate_name(&self.by_name, &entry.spec.name)?;
            Self::validate_name(&batch_names, &entry.spec.name)?;
            batch_names.insert(entry.spec.name.clone(), candidates.len());
            candidates.push(entry);
        }
        for entry in candidates {
            self.commit(entry);
        }
        Ok(self)
    }

    fn snapshot(handler: Arc<dyn ToolHandler>) -> ToolSetEntry {
        let spec = handler.spec();
        let metadata = handler.metadata();
        ToolSetEntry {
            spec,
            metadata,
            handler,
        }
    }

    fn validate_name(names: &HashMap<String, usize>, name: &str) -> Result<(), ToolSetError> {
        if names.contains_key(name) {
            return Err(ToolSetError::DuplicateTool(name.to_owned()));
        }
        Ok(())
    }

    fn commit(&mut self, entry: ToolSetEntry) {
        self.by_name
            .insert(entry.spec.name.clone(), self.entries.len());
        self.entries.push(entry);
    }

    fn unknown(name: &str) -> ToolError {
        ToolError::new(ToolErrorKind::UnknownTool, format!("unknown tool {name}"))
    }
}

#[async_trait]
impl ToolPort for ToolSet {
    async fn refresh(&self, cancel: CancelToken) -> Result<ToolSnapshot, ToolError> {
        if cancel.is_cancelled() {
            return Err(ToolError::new(
                ToolErrorKind::Adapter,
                "tool refresh cancelled",
            ));
        }
        Ok(ToolSnapshot {
            specs: self
                .entries
                .iter()
                .map(|entry| entry.spec.clone())
                .collect(),
            generation: self.generation.fetch_add(1, Ordering::Relaxed) + 1,
        })
    }

    async fn execute(
        &self,
        call: ResolvedToolCall,
        context: ToolExecutionContext,
    ) -> Result<ToolExecutionResult, ToolError> {
        let entry = self
            .by_name
            .get(&call.name)
            .and_then(|index| self.entries.get(*index))
            .ok_or_else(|| Self::unknown(&call.name))?;
        entry.handler.execute(call.input, context).await
    }
}

impl ToolResolver for ToolSet {
    fn resolve(&self, call: &ToolCall) -> Result<ResolvedToolCall, ToolError> {
        let entry = self
            .by_name
            .get(&call.name)
            .and_then(|index| self.entries.get(*index))
            .ok_or_else(|| Self::unknown(&call.name))?;
        Ok(ResolvedToolCall {
            call_id: call.id.clone(),
            name: entry.spec.name.clone(),
            input: call.input.clone(),
            access: entry.handler.access(&call.input),
            descriptor: ToolDescriptor {
                name: entry.spec.name.clone(),
                description: entry.spec.description.clone(),
                metadata: entry.metadata.clone(),
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use async_trait::async_trait;
    use tact_agent_api::{
        ResourceAccess, ToolCall, ToolError, ToolExecutionContext, ToolExecutionResult,
        ToolOutcome, ToolPort, ToolResolver, ToolSpec,
    };

    use super::*;

    struct TestTool {
        name: String,
        spec_calls: Arc<AtomicUsize>,
        metadata_calls: Arc<AtomicUsize>,
        access: ResourceAccess,
    }

    impl TestTool {
        fn new(name: &str) -> Self {
            Self {
                name: name.into(),
                spec_calls: Arc::new(AtomicUsize::new(0)),
                metadata_calls: Arc::new(AtomicUsize::new(0)),
                access: ResourceAccess::barrier(),
            }
        }

        fn independent(name: &str) -> Self {
            Self {
                access: ResourceAccess::independent(),
                ..Self::new(name)
            }
        }
    }

    #[async_trait]
    impl ToolHandler for TestTool {
        fn spec(&self) -> ToolSpec {
            self.spec_calls.fetch_add(1, Ordering::SeqCst);
            ToolSpec {
                name: self.name.clone(),
                description: format!("{} description", self.name),
                input_schema: serde_json::json!({"type": "object"}),
            }
        }

        fn metadata(&self) -> serde_json::Value {
            self.metadata_calls.fetch_add(1, Ordering::SeqCst);
            serde_json::json!({"stable": true})
        }

        fn access(&self, _input: &serde_json::Value) -> ResourceAccess {
            self.access.clone()
        }

        async fn execute(
            &self,
            _input: serde_json::Value,
            _context: ToolExecutionContext,
        ) -> Result<ToolExecutionResult, ToolError> {
            Ok(ToolExecutionResult::success(ToolOutcome::text("ok")))
        }
    }

    struct Provider(Vec<Arc<dyn ToolHandler>>);

    impl ToolProvider for Provider {
        fn tools(&self) -> Vec<Arc<dyn ToolHandler>> {
            self.0.clone()
        }
    }

    fn call(name: &str) -> ToolCall {
        ToolCall {
            id: "call-1".into(),
            name: name.into(),
            input: serde_json::json!({}),
        }
    }

    #[tokio::test]
    async fn tool_provider_composes_without_registry() {
        let tools = ToolSet::new()
            .add_provider(Provider(vec![Arc::new(TestTool::new("a"))]))
            .unwrap();
        let snapshot = tools
            .refresh(tact_agent_api::CancelToken::new())
            .await
            .unwrap();
        assert_eq!(snapshot.specs[0].name, "a");
    }

    #[tokio::test]
    async fn duplicate_tool_names_are_rejected() {
        assert!(matches!(
            ToolSet::new()
                .add(TestTool::new("same"))
                .unwrap()
                .add(TestTool::new("same")),
            Err(ToolSetError::DuplicateTool(name)) if name == "same"
        ));
        assert!(matches!(
            ToolSet::new()
                .add(TestTool::new("same"))
                .unwrap()
                .add_provider(Provider(vec![Arc::new(TestTool::new("same"))])),
            Err(ToolSetError::DuplicateTool(name)) if name == "same"
        ));
    }

    #[test]
    fn duplicate_names_inside_one_provider_are_rejected() {
        let set = ToolSet::new()
            .add(TestTool::new("existing"))
            .unwrap()
            .add_provider(Provider(vec![
                Arc::new(TestTool::new("new")),
                Arc::new(TestTool::new("new")),
            ]));
        assert_eq!(set.unwrap_err(), ToolSetError::DuplicateTool("new".into()));

        let set = ToolSet::new()
            .add(TestTool::new("existing"))
            .unwrap()
            .add_provider(Provider(vec![
                Arc::new(TestTool::new("existing")),
                Arc::new(TestTool::new("other")),
            ]))
            .unwrap_err();
        assert_eq!(set, ToolSetError::DuplicateTool("existing".into()));
    }

    #[tokio::test]
    async fn tool_spec_name_is_stable_after_composition() {
        let tool = TestTool::new("stable");
        let calls = tool.spec_calls.clone();
        let set = ToolSet::new().add(tool).unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let first = set
            .refresh(tact_agent_api::CancelToken::new())
            .await
            .unwrap();
        let second = set
            .refresh(tact_agent_api::CancelToken::new())
            .await
            .unwrap();
        assert_eq!(first.specs, second.specs);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(first.generation, 1);
        assert_eq!(second.generation, 2);
    }

    #[tokio::test]
    async fn tool_metadata_is_stable_after_composition() {
        let tool = TestTool::new("metadata");
        let calls = tool.metadata_calls.clone();
        let set = ToolSet::new().add(tool).unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        let first = set.resolve(&call("metadata")).unwrap();
        set.refresh(tact_agent_api::CancelToken::new())
            .await
            .unwrap();
        let second = set.resolve(&call("metadata")).unwrap();
        assert_eq!(first.descriptor.metadata, second.descriptor.metadata);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn tool_access_defaults_to_barrier() {
        struct WithoutAccess;

        #[async_trait]
        impl ToolHandler for WithoutAccess {
            fn spec(&self) -> ToolSpec {
                ToolSpec {
                    name: "safe".into(),
                    description: "safe".into(),
                    input_schema: serde_json::json!({}),
                }
            }

            async fn execute(
                &self,
                _input: serde_json::Value,
                _context: ToolExecutionContext,
            ) -> Result<ToolExecutionResult, ToolError> {
                Ok(ToolExecutionResult::success(ToolOutcome::text("ok")))
            }
        }

        let set = ToolSet::new().add(WithoutAccess).unwrap();
        assert!(set.resolve(&call("safe")).unwrap().access.barrier);
    }

    #[test]
    fn resolve_uses_handler_access_and_cached_descriptor() {
        let set = ToolSet::new()
            .add(TestTool::independent("independent"))
            .unwrap();
        let resolved = set.resolve(&call("independent")).unwrap();
        assert!(!resolved.access.barrier);
        assert_eq!(resolved.descriptor.name, "independent");
        assert_eq!(
            resolved.descriptor.metadata,
            serde_json::json!({"stable": true})
        );
    }

    #[tokio::test]
    async fn empty_tools_has_defined_behavior() {
        let tools = crate::EmptyTools;
        assert!(tools.resolve(&call("missing")).unwrap_err().kind == ToolErrorKind::UnknownTool);
        assert_eq!(
            tools
                .refresh(tact_agent_api::CancelToken::new())
                .await
                .unwrap(),
            tact_agent_api::ToolSnapshot {
                specs: Vec::new(),
                generation: 1,
            }
        );
    }
}
