//! Protocol version negotiation.

use tact_protocol::ProtocolVersion;

use crate::KernelError;

/// Selects a version both sides can speak.
///
/// Major versions must match; a requested minor may be no higher than the
/// supported minor, and the negotiation answers with the requested version so
/// a client learns the highest minor the host accepted.
pub fn negotiate(
    requested: ProtocolVersion,
    supported: ProtocolVersion,
) -> Result<ProtocolVersion, KernelError> {
    if requested.major != supported.major || requested.minor > supported.minor {
        return Err(KernelError::new(
            tact_protocol::ErrorCategory::ProtocolMismatch,
            format!(
                "unsupported protocol version {}.{} (host supports {}.{})",
                requested.major, requested.minor, supported.major, supported.minor
            ),
            "protocol",
            false,
        ));
    }
    Ok(requested)
}
