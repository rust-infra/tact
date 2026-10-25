use crate::tool::descriptor::ToolDescriptor;

pub trait Tool: Send + Sync {
    fn descriptor(&self) -> ToolDescriptor;
}
