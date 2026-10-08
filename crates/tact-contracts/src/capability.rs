//! Capability descriptors and invocation values.

use serde_json::Value;
use std::path::PathBuf;
use strum_macros::Display;

/// Workspace resources touched by one invocation.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ToolResources {
    pub reads: Vec<PathBuf>,
    pub writes: Vec<PathBuf>,
    pub barrier: bool,
}

/// Coarse risk classification shared by permission and capability owners.
#[derive(Clone, Copy, Debug, Display, Eq, PartialEq)]
#[strum(serialize_all = "snake_case")]
pub enum CapabilityRisk {
    Read,
    Write,
    High,
}

/// Side effects returned by a capability invocation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ToolEffect {
    CompactHistory { focus: Option<String> },
}

/// Complete result shape shared by native and future external backends.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolCallResult {
    pub content: String,
    pub effects: Vec<ToolEffect>,
    pub image: Option<ToolImage>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolImage {
    pub type_: String,
    pub media_type: String,
    pub data: String,
}

impl ToolCallResult {
    pub fn text(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            effects: Vec::new(),
            image: None,
        }
    }

    pub fn text_image(content: impl Into<String>, image: ToolImage) -> Self {
        Self {
            content: content.into(),
            effects: Vec::new(),
            image: Some(image),
        }
    }
}

impl ToolResources {
    pub fn barrier() -> Self {
        Self {
            barrier: true,
            ..Self::default()
        }
    }

    pub fn independent() -> Self {
        Self::default()
    }
}

/// A concrete invocation passed from the runtime to an owner backend.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolInvocation {
    pub tool_id: String,
    pub name: String,
    pub input: Value,
}

/// Where a capability is implemented.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ToolOrigin {
    Native,
    Mcp { server: String },
    McpResource,
    McpPrompt,
}

/// Model-visible metadata resolved from an owner registry.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolDescriptor {
    pub name: String,
    pub origin: ToolOrigin,
    pub schema: Value,
    pub description: String,
}
