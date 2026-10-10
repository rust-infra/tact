use tact_protocol::{PluginId, PluginRequest, PluginRequestEnvelope, ProtocolVersion, RequestId};

pub fn make_envelope(
    protocol: ProtocolVersion,
    plugin_id: &PluginId,
    request_id: RequestId,
    request: PluginRequest,
) -> PluginRequestEnvelope {
    PluginRequestEnvelope {
        protocol_version: protocol,
        request_id,
        plugin_id: plugin_id.clone(),
        session_id: None,
        run_id: None,
        trajectory_id: None,
        deadline: None,
        request,
    }
}
