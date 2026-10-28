use std::{future::Future, sync::Arc};

use tact_agent_api::{EventObserver, ModelPort};
use tact_agent_core::KernelDeps;
use tracing::{Instrument, Span};

mod context;
mod events;
mod gate;
mod model;
mod store;
mod tools;
mod usage;

pub(crate) use events::TelemetryEventObserver;
pub(crate) use model::InstrumentedModelPort;

/// Host-selectable tracing instrumentation settings.
#[derive(Debug, Clone)]
pub struct TelemetryConfig {
    enabled: bool,
    record_content: bool,
}

impl Default for TelemetryConfig {
    fn default() -> Self {
        Self::enabled()
    }
}

impl TelemetryConfig {
    pub fn enabled() -> Self {
        Self {
            enabled: true,
            record_content: false,
        }
    }

    pub fn disabled() -> Self {
        Self {
            enabled: false,
            record_content: false,
        }
    }

    pub fn record_content(mut self, enabled: bool) -> Self {
        self.record_content = enabled;
        self
    }

    pub(crate) fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub(crate) fn record_content_enabled(&self) -> bool {
        self.record_content
    }
}

pub(crate) fn instrument_graph(
    mut deps: KernelDeps,
    mut observers: Vec<Arc<dyn EventObserver>>,
    config: &TelemetryConfig,
) -> (KernelDeps, Vec<Arc<dyn EventObserver>>) {
    let model: Arc<dyn ModelPort> = Arc::new(InstrumentedModelPort::new(
        deps.model.clone(),
        config.clone(),
    ));
    deps.model = model;
    deps.tools = Arc::new(tools::InstrumentedToolPort::new(deps.tools.clone()));
    deps.gate = Arc::new(gate::InstrumentedToolGate::new(deps.gate.clone()));
    deps.context = Arc::new(context::InstrumentedContextPolicy::new(
        deps.context.clone(),
        config.clone(),
    ));
    deps.transcript = Arc::new(store::InstrumentedTranscriptStore::new(
        deps.transcript.clone(),
    ));
    deps.usage = Arc::new(usage::InstrumentedUsageSink::new(deps.usage.clone()));
    observers.push(Arc::new(TelemetryEventObserver::new(config.clone())));
    (deps, observers)
}

pub(crate) async fn timed<F, T>(span: Span, future: F) -> T
where
    F: Future<Output = T>,
{
    let started = std::time::Instant::now();
    let span_name = span
        .metadata()
        .map_or("unknown", |metadata| metadata.name());
    let output = future.instrument(span.clone()).await;
    let elapsed = started.elapsed().as_millis();
    span.record("duration_ms", elapsed);
    span.in_scope(|| {
        tracing::debug!(
            target: "tact_agent_sdk",
            metric = "agent.span.finished",
            instrumented_span = span_name,
            duration_ms = elapsed,
            "agent span finished"
        );
    });
    output
}

pub(crate) fn truncate_content(value: &str) -> String {
    value.chars().take(512).collect()
}

pub(crate) fn debug_content(kind: &'static str, field: &'static str, value: &str) {
    let value = truncate_content(value);
    tracing::debug!(
        target: "tact_agent_sdk",
        content_kind = kind,
        content_field = field,
        content = %value,
        "agent content"
    );
}

#[cfg(test)]
mod integration_tests {
    include!("tests.rs");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_is_bounded_by_unicode_scalars() {
        let value = "界".repeat(600);
        assert_eq!(truncate_content(&value).chars().count(), 512);
    }

    #[test]
    fn telemetry_defaults_to_enabled_without_content() {
        let config = TelemetryConfig::default();
        assert!(config.is_enabled());
        assert!(!config.record_content_enabled());
        assert!(!TelemetryConfig::disabled().is_enabled());
    }
}
