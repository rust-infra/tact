//! The SDK must not require the host to install a `tracing` subscriber.
//!
//! This lives in its own test binary because the assertion is about process
//! state. `tracing::dispatcher::has_been_set()` reads a process-global flag
//! that any `tracing::subscriber::set_default` sets for the rest of the run,
//! and the sibling tests in `telemetry/tests.rs` all install a scoped
//! subscriber. In one binary the flag was already true before this test's
//! first assertion ran, so the test passed or failed on the test runner's
//! scheduling. A separate binary starts with the flag clear.

use tact_agent_api::{RunStop, StopReason};
use tact_agent_core::test_support::{ScriptedModel, events, text_response};
use tact_agent_sdk::AgentBuilder;

#[tokio::test]
async fn no_subscriber_is_required_or_installed() {
    assert!(
        !tracing::dispatcher::has_been_set(),
        "the test binary starts with no dispatcher, so the check below can mean something"
    );

    let model = ScriptedModel::new(vec![events(text_response(
        "assistant",
        "ok",
        StopReason::EndTurn,
    ))]);
    let mut session = AgentBuilder::new(model)
        .build_session("no-subscriber")
        .unwrap();
    let outcome = session.run_text("go").await.unwrap();

    assert_eq!(outcome.stop, RunStop::Completed);
    assert!(
        !tracing::dispatcher::has_been_set(),
        "running a session must not install a subscriber behind the host's back"
    );
}
