use crate::{PluginId, RequestId, RunId, SessionId, TrajectoryId};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtocolVersion {
    pub major: u16,
    pub minor: u16,
}

impl ProtocolVersion {
    pub const CURRENT: Self = Self { major: 1, minor: 0 };
    pub const fn new(major: u16, minor: u16) -> Self {
        Self { major, minor }
    }
    pub fn compatible_with(self, other: Self) -> bool {
        self.major == other.major && self.minor <= other.minor
    }
}

impl Default for ProtocolVersion {
    fn default() -> Self {
        Self::CURRENT
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequestEnvelope {
    pub protocol_version: ProtocolVersion,
    pub request_id: RequestId,
    pub plugin_id: PluginId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<SessionId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<RunId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trajectory_id: Option<TrajectoryId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deadline: Option<String>,
    pub request: crate::PluginRequest,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResponseEnvelope {
    pub protocol_version: ProtocolVersion,
    pub request_id: RequestId,
    pub plugin_id: PluginId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<SessionId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<RunId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trajectory_id: Option<TrajectoryId>,
    pub response: crate::PluginResponse,
}

pub type PluginRequestEnvelope = RequestEnvelope;
pub type PluginResponseEnvelope = ResponseEnvelope;
