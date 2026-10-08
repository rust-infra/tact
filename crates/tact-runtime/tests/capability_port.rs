use anyhow::Result;
use async_trait::async_trait;
use tact_contracts::capability::{
    ToolCallResult, ToolDescriptor, ToolInvocation, ToolOrigin, ToolResources,
};
use tact_runtime::capability::ToolExecutor;

struct Echo;

#[async_trait]
impl ToolExecutor for Echo {
    fn describe(&self, name: &str) -> Option<ToolDescriptor> {
        Some(ToolDescriptor {
            name: name.into(),
            origin: ToolOrigin::Native,
            schema: serde_json::json!({}),
            description: "echo".into(),
        })
    }

    fn resources(&self, _call: &ToolInvocation) -> Result<ToolResources> {
        Ok(ToolResources::independent())
    }

    async fn execute(&self, call: &ToolInvocation) -> Result<ToolCallResult> {
        Ok(ToolCallResult::text(call.input.to_string()))
    }
}

#[tokio::test]
async fn executor_port_preserves_descriptor_resources_and_result() {
    let executor = Echo;
    let call = ToolInvocation {
        tool_id: "1".into(),
        name: "echo".into(),
        input: serde_json::json!("ok"),
    };
    assert_eq!(executor.describe("echo").expect("descriptor").name, "echo");
    assert_eq!(
        executor.resources(&call).expect("resources"),
        ToolResources::independent()
    );
    assert_eq!(
        executor.execute(&call).await.expect("result").content,
        "\"ok\""
    );
}
