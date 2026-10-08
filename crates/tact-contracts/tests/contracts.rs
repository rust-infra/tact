use serde_json::json;
use tact_contracts::{
    capability::{CapabilityRisk, ToolDescriptor, ToolInvocation, ToolOrigin, ToolResources},
    run::{RunId, RunOutcome, RunProgress},
};

#[test]
fn contracts_keep_run_identity_and_invocation_data() {
    let id = RunId(7);
    assert_eq!(RunProgress { id, model_turns: 2 }.id, id);
    assert_eq!(RunOutcome::Completed, RunOutcome::Completed);

    let call = ToolInvocation {
        tool_id: "call-1".into(),
        name: "read_file".into(),
        input: json!({"path": "src/lib.rs"}),
    };
    let descriptor = ToolDescriptor {
        name: call.name.clone(),
        origin: ToolOrigin::Native,
        schema: json!({"type": "object"}),
        description: "Read a file".into(),
    };
    assert_eq!(descriptor.name, call.name);
}

#[test]
fn resources_keep_barrier_and_independent_semantics() {
    assert!(ToolResources::barrier().barrier);
    assert_eq!(ToolResources::independent(), ToolResources::default());
}

#[test]
fn capability_risk_has_stable_wire_names() {
    assert_eq!(CapabilityRisk::Read.to_string(), "read");
    assert_eq!(CapabilityRisk::Write.to_string(), "write");
    assert_eq!(CapabilityRisk::High.to_string(), "high");
}
