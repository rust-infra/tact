use tact::permission::{CapabilityRisk, PermissionPolicyExt};
use tact::tool::{PermissionPolicy, ToolCallResult, ToolImage, ToolMetadata, ToolResources};

#[test]
fn moved_contracts_keep_their_public_paths() {
    assert_eq!(
        ToolMetadata::read_json("echo", "Echo", "Echo").permission,
        PermissionPolicy::Read
    );
    assert_eq!(CapabilityRisk::Read, CapabilityRisk::Read);
    assert_eq!(ToolCallResult::text("ok").content, "ok");
    let _ = ToolImage {
        mime_type: "image/png".into(),
        data: "AA==".into(),
    };
    assert_eq!(
        PermissionPolicy::Read.resolve(&serde_json::json!({})),
        CapabilityRisk::Read
    );
    assert!(!ToolResources::independent().barrier);
}
