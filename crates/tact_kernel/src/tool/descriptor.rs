//! Owned, model-facing tool descriptors.

use serde_json::Value;

use crate::tool::metadata::{ToolMetadata, ToolPolicy};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolDescriptor {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    pub policy: ToolPolicy,
}

impl ToolDescriptor {
    /// Build an owned descriptor from a native static declaration.
    #[must_use]
    pub fn from_native(
        name: impl Into<String>,
        description: impl Into<String>,
        input_schema: Value,
        metadata: &ToolMetadata,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            input_schema,
            policy: metadata.policy(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::metadata::{PermissionPolicy, ToolMetadata};
    use serde_json::json;

    #[test]
    fn native_descriptor_carries_owned_policy_without_static_strings() {
        let metadata = ToolMetadata::path_read("read_file", "Read", "Read File", "path");
        let descriptor = ToolDescriptor::from_native(
            "mcp__docs__read",
            "Read a document",
            json!({"type": "object"}),
            &metadata,
        );
        assert_eq!(descriptor.name, "mcp__docs__read");
        assert_eq!(descriptor.description, "Read a document");
        assert_eq!(
            descriptor.policy.permission,
            PermissionPolicy::ReadPath { path_field: "path" }
        );
        assert_eq!(descriptor.policy.presentation.display_name, "Read File");
    }

    #[test]
    fn dynamic_descriptor_can_be_built_without_any_static_metadata() {
        let policy = ToolMetadata::read_json("native", "native", "Native").policy();
        let descriptor = ToolDescriptor {
            name: "mcp__docs__search".into(),
            description: "Search docs".into(),
            input_schema: json!({"type": "object"}),
            policy,
        };
        assert_eq!(descriptor.name, "mcp__docs__search");
        assert_eq!(descriptor.policy.presentation.display_name, "Native");
    }
}
